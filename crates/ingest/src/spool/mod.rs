//! Optional on-disk write-ahead spool for accepted envelopes (`F020`).
//!
//! Without the spool, envelopes accepted with `200` but still in the bounded worker channel
//! (or in an unflushed batch) are lost when the process crashes or restarts. With
//! `INGEST_SPOOL_DIR` set, every accepted envelope is appended to a CRC-checked segment file
//! *before* it is queued; the worker records a checkpoint after each flushed batch, and on
//! startup everything after the checkpoint is replayed. Delivery is at-least-once: a crash
//! between a database commit and the checkpoint write replays that batch.
//!
//! Durability: records reach the kernel on every write (survives a process crash) and the
//! active segment is `fdatasync`'d every second (bounded loss on power failure, PRD §8).
//!
//! The spool holds raw payloads **before** PII scrubbing, so the directory must be private
//! (`0700`, files `0600`) and segments are deleted as soon as they are checkpointed.

pub(crate) mod codec;

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::model::QueuedEnvelope;
use codec::FrameRead;

const SEGMENT_MAGIC: &[u8; 8] = b"MXLSPL01";
/// Rotation size; bounds disk use to the unprocessed backlog plus one segment.
const SEGMENT_MAX_BYTES: u64 = 64 * 1024 * 1024;
const SEGMENT_PREFIX: &str = "segment-";
const SEGMENT_SUFFIX: &str = ".spool";
const LOCK_FILE: &str = "spool.lock";
const CHECKPOINT_FILE: &str = "checkpoint";
const CHECKPOINT_TMP_FILE: &str = "checkpoint.tmp";
const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(1);
/// Unscrubbed payloads must not linger on disk after they are stored: a non-empty active
/// segment is rotated after this age so maintenance can delete it once checkpointed.
const SEGMENT_MAX_AGE: Duration = Duration::from_secs(30);

/// Why an envelope could not be accepted.
#[derive(Debug)]
pub enum EnqueueError {
    /// Worker channel is full: answer `429` (nothing was written).
    Full,
    /// Worker channel is closed (shutdown).
    Closed,
    /// The spool could not persist the envelope: answer `503` (nothing was queued).
    Io(io::Error),
}

/// Result of [`Spool::replay`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReplayStats {
    pub replayed: usize,
    pub undecodable: usize,
}

#[derive(Clone)]
pub struct Spool {
    inner: Arc<Inner>,
}

/// Handle the ingest worker uses to report flushed envelopes.
#[derive(Clone)]
pub struct SpoolAck {
    inner: Arc<Inner>,
}

struct Inner {
    dir: PathBuf,
    segment_max_age: Duration,
    checkpoint_at_open: u64,
    committed: AtomicU64,
    writer: Mutex<Writer>,
    /// Held for the lifetime of the spool; the OS releases the lock if the process dies.
    _lock: File,
}

struct Segment {
    last_seq: u64,
    path: PathBuf,
}

struct Writer {
    next_seq: u64,
    active: Arc<File>,
    active_path: PathBuf,
    active_first_seq: u64,
    active_bytes: u64,
    active_created: Instant,
    /// Set after a failed write: the active segment may end in a torn frame, so the next
    /// record must start a fresh segment or every record after it would be unreadable.
    needs_rotation: bool,
    closed: VecDeque<Segment>,
}

impl Spool {
    /// Open (or create) the spool directory, take its exclusive lock, drop segments that are
    /// fully checkpointed, and start a new active segment.
    pub fn open(dir: &Path) -> io::Result<Self> {
        Self::open_with_segment_max_age(dir, SEGMENT_MAX_AGE)
    }

    fn open_with_segment_max_age(dir: &Path, segment_max_age: Duration) -> io::Result<Self> {
        prepare_private_dir(dir)?;
        let lock = acquire_lock(dir)?;
        let checkpoint = read_checkpoint(dir);

        let mut closed = VecDeque::new();
        let mut max_seq = checkpoint;
        for path in list_segments(dir)? {
            match scan_last_seq(&path)? {
                Some(last_seq) if last_seq > checkpoint => {
                    max_seq = max_seq.max(last_seq);
                    closed.push_back(Segment { last_seq, path });
                }
                _ => remove_segment(&path),
            }
        }

        let next_seq = max_seq + 1;
        let (active, active_path) = create_segment(dir, next_seq)?;
        if !closed.is_empty() {
            info!(
                segments = closed.len(),
                checkpoint, "ingest spool has unflushed envelopes to replay"
            );
        }

        Ok(Self {
            inner: Arc::new(Inner {
                dir: dir.to_path_buf(),
                segment_max_age,
                checkpoint_at_open: checkpoint,
                committed: AtomicU64::new(checkpoint),
                writer: Mutex::new(Writer {
                    next_seq,
                    active: Arc::new(active),
                    active_path,
                    active_first_seq: next_seq,
                    active_bytes: SEGMENT_MAGIC.len() as u64,
                    active_created: Instant::now(),
                    needs_rotation: false,
                    closed,
                }),
                _lock: lock,
            }),
        })
    }

    pub fn ack(&self) -> SpoolAck {
        SpoolAck {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Re-queue every record written before this process started and not yet checkpointed.
    /// Must run before new envelopes are accepted so the worker sees them in sequence order.
    pub async fn replay(&self, tx: &mpsc::Sender<QueuedEnvelope>) -> io::Result<ReplayStats> {
        let paths: Vec<PathBuf> = {
            let writer = self.inner.lock_writer()?;
            writer.closed.iter().map(|s| s.path.clone()).collect()
        };
        let checkpoint = self.inner.checkpoint_at_open;
        let mut stats = ReplayStats::default();
        for path in paths {
            let (envelopes, undecodable) =
                tokio::task::spawn_blocking(move || read_pending(&path, checkpoint))
                    .await
                    .map_err(io::Error::other)??;
            stats.undecodable += undecodable;
            for envelope in envelopes {
                tx.send(envelope)
                    .await
                    .map_err(|_| io::Error::other("ingest worker channel closed during replay"))?;
                stats.replayed += 1;
            }
        }
        Ok(stats)
    }

    /// Persist `envelope` and push it onto the worker channel, atomically with respect to
    /// other callers so sequence order equals channel order.
    pub fn enqueue(
        &self,
        mut envelope: QueuedEnvelope,
        tx: &mpsc::Sender<QueuedEnvelope>,
    ) -> Result<(), EnqueueError> {
        if tx.is_closed() {
            return Err(EnqueueError::Closed);
        }
        let mut frame = codec::unsealed_frame(&envelope);
        let mut writer = self.inner.lock_writer().map_err(EnqueueError::Io)?;
        // Only this method sends on `tx` while the spool is enabled, and it holds the lock,
        // so free capacity seen here is still free at `try_send` below.
        if tx.capacity() == 0 {
            return Err(EnqueueError::Full);
        }
        if writer.needs_rotation {
            self.inner.rotate(&mut writer).map_err(EnqueueError::Io)?;
        }

        let seq = writer.next_seq;
        codec::seal_frame(&mut frame, seq);
        if let Err(err) = (&*writer.active).write_all(&frame) {
            writer.needs_rotation = true;
            return Err(EnqueueError::Io(err));
        }
        writer.next_seq += 1;
        writer.active_bytes += frame.len() as u64;

        envelope.spool_seq = Some(seq);
        let sent = tx.try_send(envelope);

        if writer.active_bytes >= SEGMENT_MAX_BYTES
            && let Err(err) = self.inner.rotate(&mut writer)
        {
            warn!(error = %err, "failed to rotate ingest spool segment; retrying on next write");
            writer.needs_rotation = true;
        }
        drop(writer);

        // Unreachable `Full` (capacity was checked under the lock) still maps to 429; the
        // record is then replayed after a restart, which is safe (at-least-once).
        sent.map_err(|err| match err {
            TrySendError::Full(_) => EnqueueError::Full,
            TrySendError::Closed(_) => EnqueueError::Closed,
        })
    }

    /// Every second: `fdatasync` the active segment, rotate it once it is older than
    /// `SEGMENT_MAX_AGE`, and delete checkpointed segments.
    pub fn spawn_maintenance(&self) -> JoinHandle<()> {
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(MAINTENANCE_INTERVAL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let inner = Arc::clone(&inner);
                if let Err(err) = tokio::task::spawn_blocking(move || inner.maintain()).await {
                    warn!(error = %err, "ingest spool maintenance task failed");
                }
            }
        })
    }

    /// One maintenance pass (exposed for tests; the server uses [`Spool::spawn_maintenance`]).
    pub fn run_maintenance(&self) {
        self.inner.maintain();
    }

    /// Number of segment files currently on disk, including the active one.
    pub fn segment_count(&self) -> io::Result<usize> {
        Ok(list_segments(&self.inner.dir)?.len())
    }
}

impl SpoolAck {
    /// Record that every envelope up to `seq` has been flushed to PostgreSQL.
    pub async fn commit(&self, seq: u64) {
        let previous = self.inner.committed.fetch_max(seq, Ordering::SeqCst);
        if seq <= previous {
            return;
        }
        let dir = self.inner.dir.clone();
        match tokio::task::spawn_blocking(move || write_checkpoint(&dir, seq)).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => warn!(error = %err, seq, "failed to persist ingest spool checkpoint"),
            Err(err) => warn!(error = %err, seq, "ingest spool checkpoint task failed"),
        }
    }
}

impl Inner {
    fn lock_writer(&self) -> io::Result<std::sync::MutexGuard<'_, Writer>> {
        self.writer
            .lock()
            .map_err(|_| io::Error::other("ingest spool writer lock poisoned"))
    }

    fn rotate(&self, writer: &mut Writer) -> io::Result<()> {
        let (file, path) = create_segment(&self.dir, writer.next_seq)?;
        if let Err(err) = writer.active.sync_data() {
            warn!(error = %err, "failed to sync finished ingest spool segment");
        }
        let old_path = std::mem::replace(&mut writer.active_path, path);
        if writer.next_seq > writer.active_first_seq {
            writer.closed.push_back(Segment {
                last_seq: writer.next_seq - 1,
                path: old_path,
            });
        } else {
            remove_segment(&old_path);
        }
        writer.active = Arc::new(file);
        writer.active_first_seq = writer.next_seq;
        writer.active_bytes = SEGMENT_MAGIC.len() as u64;
        writer.active_created = Instant::now();
        writer.needs_rotation = false;
        Ok(())
    }

    fn maintain(&self) {
        let committed = self.committed.load(Ordering::SeqCst);
        let (active, expired) = match self.lock_writer() {
            Ok(mut writer) => {
                let has_records = writer.next_seq > writer.active_first_seq;
                if has_records
                    && writer.active_created.elapsed() >= self.segment_max_age
                    && let Err(err) = self.rotate(&mut writer)
                {
                    warn!(error = %err, "failed to rotate aged ingest spool segment");
                }
                let mut expired = Vec::new();
                while writer
                    .closed
                    .front()
                    .is_some_and(|s| s.last_seq <= committed)
                {
                    if let Some(segment) = writer.closed.pop_front() {
                        expired.push(segment.path);
                    }
                }
                (Arc::clone(&writer.active), expired)
            }
            Err(err) => {
                warn!(error = %err, "skipping ingest spool maintenance");
                return;
            }
        };
        if let Err(err) = active.sync_data() {
            warn!(error = %err, "failed to sync active ingest spool segment");
        }
        for path in &expired {
            remove_segment(path);
        }
        // Persist directory entries of newly created / removed segments.
        if let Err(err) = File::open(&self.dir).and_then(|d| d.sync_all()) {
            warn!(error = %err, "failed to sync ingest spool directory");
        }
    }
}

fn private_options() -> OpenOptions {
    let mut opts = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts
}

fn prepare_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    let meta = fs::metadata(dir)?;
    if !meta.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "INGEST_SPOOL_DIR is not a directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Never chmod an existing directory (it may be shared, e.g. /tmp): refuse instead.
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "INGEST_SPOOL_DIR holds unscrubbed payloads and must not be accessible by group/other (use a dedicated directory with mode 0700)",
            ));
        }
    }
    Ok(())
}

fn acquire_lock(dir: &Path) -> io::Result<File> {
    let file = private_options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(LOCK_FILE))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "INGEST_SPOOL_DIR is already in use by another maxi-loop process",
        )),
        Err(TryLockError::Error(err)) => Err(err),
    }
}

/// A missing or unreadable checkpoint means "replay everything": duplicates, never loss.
fn read_checkpoint(dir: &Path) -> u64 {
    match fs::read_to_string(dir.join(CHECKPOINT_FILE)) {
        Ok(text) => text.trim().parse().unwrap_or_else(|_| {
            warn!("ingest spool checkpoint is unreadable; replaying all segments");
            0
        }),
        Err(err) if err.kind() == io::ErrorKind::NotFound => 0,
        Err(err) => {
            warn!(error = %err, "failed to read ingest spool checkpoint; replaying all segments");
            0
        }
    }
}

/// Write-then-rename so a crash leaves either the old or the new checkpoint, never a torn one.
fn write_checkpoint(dir: &Path, seq: u64) -> io::Result<()> {
    let tmp = dir.join(CHECKPOINT_TMP_FILE);
    let mut file = private_options()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp)?;
    file.write_all(format!("{seq}\n").as_bytes())?;
    drop(file);
    fs::rename(&tmp, dir.join(CHECKPOINT_FILE))
}

fn segment_path(dir: &Path, first_seq: u64) -> PathBuf {
    dir.join(format!("{SEGMENT_PREFIX}{first_seq:020}{SEGMENT_SUFFIX}"))
}

fn create_segment(dir: &Path, first_seq: u64) -> io::Result<(File, PathBuf)> {
    let path = segment_path(dir, first_seq);
    let mut file = private_options()
        .create_new(true)
        .append(true)
        .open(&path)?;
    if let Err(err) = file.write_all(SEGMENT_MAGIC) {
        remove_segment(&path);
        return Err(err);
    }
    Ok((file, path))
}

fn remove_segment(path: &Path) {
    if let Err(err) = fs::remove_file(path)
        && err.kind() != io::ErrorKind::NotFound
    {
        warn!(path = %path.display(), error = %err, "failed to delete ingest spool segment");
    }
}

/// Segment files sorted by their first sequence number.
fn list_segments(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut segments = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(first_seq) = name
            .to_str()
            .and_then(|n| n.strip_prefix(SEGMENT_PREFIX))
            .and_then(|n| n.strip_suffix(SEGMENT_SUFFIX))
            .and_then(|n| n.parse::<u64>().ok())
        else {
            continue;
        };
        segments.push((first_seq, entry.path()));
    }
    segments.sort_by_key(|(first_seq, _)| *first_seq);
    Ok(segments.into_iter().map(|(_, path)| path).collect())
}

/// Visit every intact record of a segment, stopping at the first torn or corrupt frame.
fn for_each_record(path: &Path, mut visit: impl FnMut(u64, &[u8])) -> io::Result<()> {
    let data = fs::read(path)?;
    let Some(mut rest) = data.strip_prefix(SEGMENT_MAGIC.as_slice()) else {
        warn!(path = %path.display(), "ingest spool segment has no valid header; ignoring it");
        return Ok(());
    };
    loop {
        match codec::read_frame(rest) {
            FrameRead::Record {
                seq,
                payload,
                consumed,
            } => {
                visit(seq, payload);
                rest = &rest[consumed..];
            }
            FrameRead::End => return Ok(()),
            FrameRead::Corrupt => {
                warn!(
                    path = %path.display(),
                    trailing_bytes = rest.len(),
                    "ingest spool segment ends in a torn or corrupt record; ignoring the tail"
                );
                return Ok(());
            }
        }
    }
}

fn scan_last_seq(path: &Path) -> io::Result<Option<u64>> {
    let mut last = None;
    for_each_record(path, |seq, _| last = Some(seq))?;
    Ok(last)
}

fn read_pending(path: &Path, checkpoint: u64) -> io::Result<(Vec<QueuedEnvelope>, usize)> {
    let mut envelopes = Vec::new();
    let mut undecodable = 0;
    for_each_record(path, |seq, payload| {
        if seq <= checkpoint {
            return;
        }
        match codec::decode_envelope(payload) {
            Ok(mut envelope) => {
                envelope.spool_seq = Some(seq);
                envelopes.push(envelope);
            }
            Err(err) => {
                undecodable += 1;
                warn!(seq, error = ?err, "skipping undecodable ingest spool record");
            }
        }
    })?;
    Ok((envelopes, undecodable))
}

#[cfg(test)]
mod tests;
