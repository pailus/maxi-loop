use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::mpsc;

use super::codec::tests::sample_envelope;
use super::*;

/// Private temporary directory removed on drop (no extra dev-dependency needed).
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "maxi-loop-spool-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&path);
        Self(path.join("spool"))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Some(parent) = self.0.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }
}

fn envelope(id: &str) -> QueuedEnvelope {
    let mut env = sample_envelope();
    env.envelope_id = id.to_owned();
    env
}

async fn replay_ids(dir: &Path) -> (Spool, Vec<(u64, String)>) {
    let spool = Spool::open(dir).expect("reopen spool");
    let (tx, mut rx) = mpsc::channel(64);
    spool.replay(&tx).await.expect("replay");
    drop(tx);
    let mut out = Vec::new();
    while let Some(env) = rx.recv().await {
        out.push((env.spool_seq.expect("replayed seq"), env.envelope_id));
    }
    (spool, out)
}

#[tokio::test]
async fn unflushed_envelopes_are_replayed_in_order_after_a_crash() {
    let tmp = TempDir::new();
    {
        let spool = Spool::open(tmp.path()).expect("open spool");
        let (tx, mut rx) = mpsc::channel(8);
        for id in ["a", "b", "c"] {
            spool.enqueue(envelope(id), &tx).expect("enqueue");
        }
        let first = rx.recv().await.expect("queued envelope");
        assert_eq!(first.spool_seq, Some(1));
        // Dropping without any ack simulates a crash before the worker flushed anything.
    }

    let (_spool, replayed) = replay_ids(tmp.path()).await;
    assert_eq!(
        replayed,
        vec![
            (1, "a".to_owned()),
            (2, "b".to_owned()),
            (3, "c".to_owned())
        ]
    );
}

#[tokio::test]
async fn replayed_payload_is_byte_identical() {
    let tmp = TempDir::new();
    let original = envelope("exact");
    {
        let spool = Spool::open(tmp.path()).expect("open spool");
        let (tx, _rx) = mpsc::channel(8);
        spool.enqueue(original.clone(), &tx).expect("enqueue");
    }
    let spool = Spool::open(tmp.path()).expect("reopen");
    let (tx, mut rx) = mpsc::channel(8);
    spool.replay(&tx).await.expect("replay");
    let mut replayed = rx.recv().await.expect("replayed envelope");
    assert_eq!(replayed.spool_seq, Some(1));
    replayed.spool_seq = None;
    assert_eq!(replayed, original);
}

#[tokio::test]
async fn checkpointed_records_are_skipped_and_their_segments_deleted() {
    let tmp = TempDir::new();
    {
        let spool = Spool::open(tmp.path()).expect("open spool");
        let (tx, _rx) = mpsc::channel(8);
        for id in ["a", "b", "c"] {
            spool.enqueue(envelope(id), &tx).expect("enqueue");
        }
        spool.ack().commit(2).await;
    }

    let (spool, replayed) = replay_ids(tmp.path()).await;
    assert_eq!(replayed, vec![(3, "c".to_owned())]);

    // New writes continue after the highest sequence ever written.
    let (tx, mut rx) = mpsc::channel(8);
    spool.enqueue(envelope("d"), &tx).expect("enqueue");
    assert_eq!(rx.recv().await.and_then(|e| e.spool_seq), Some(4));

    assert_eq!(spool.segment_count().expect("count"), 2);
    spool.ack().commit(3).await;
    spool.run_maintenance();
    assert_eq!(
        spool.segment_count().expect("count"),
        1,
        "the old segment is deleted once fully checkpointed; the active one stays"
    );
}

#[tokio::test]
async fn processed_payloads_do_not_linger_in_the_active_segment() {
    let tmp = TempDir::new();
    let spool = Spool::open_with_segment_max_age(tmp.path(), Duration::ZERO).expect("open spool");
    let (tx, _rx) = mpsc::channel(8);
    spool.enqueue(envelope("a"), &tx).expect("enqueue");
    let first_segment = list_segments(tmp.path()).expect("list")[0].clone();

    // Not yet checkpointed: the aged segment is rotated but must be kept.
    spool.run_maintenance();
    assert!(first_segment.exists());

    spool.ack().commit(1).await;
    spool.run_maintenance();
    assert!(
        !first_segment.exists(),
        "a checkpointed segment holding unscrubbed payloads is deleted"
    );
    assert_eq!(spool.segment_count().expect("count"), 1);

    // An empty active segment is not rotated again.
    spool.run_maintenance();
    assert_eq!(spool.segment_count().expect("count"), 1);
}

#[tokio::test]
async fn full_channel_is_rejected_without_writing_a_record() {
    let tmp = TempDir::new();
    {
        let spool = Spool::open(tmp.path()).expect("open spool");
        let (tx, _rx) = mpsc::channel(1);
        spool.enqueue(envelope("kept"), &tx).expect("enqueue");
        assert!(matches!(
            spool.enqueue(envelope("rejected"), &tx),
            Err(EnqueueError::Full)
        ));
    }
    let (_spool, replayed) = replay_ids(tmp.path()).await;
    assert_eq!(replayed, vec![(1, "kept".to_owned())]);
}

#[tokio::test]
async fn closed_channel_is_rejected() {
    let tmp = TempDir::new();
    let spool = Spool::open(tmp.path()).expect("open spool");
    let (tx, rx) = mpsc::channel(1);
    drop(rx);
    assert!(matches!(
        spool.enqueue(envelope("x"), &tx),
        Err(EnqueueError::Closed)
    ));
}

#[tokio::test]
async fn torn_tail_after_a_crash_is_ignored() {
    let tmp = TempDir::new();
    {
        let spool = Spool::open(tmp.path()).expect("open spool");
        let (tx, _rx) = mpsc::channel(8);
        spool.enqueue(envelope("a"), &tx).expect("enqueue");
        spool.enqueue(envelope("b"), &tx).expect("enqueue");
    }
    let segment = list_segments(tmp.path()).expect("list")[0].clone();
    let mut file = OpenOptions::new()
        .append(true)
        .open(&segment)
        .expect("open segment");
    // A partial frame header: what an interrupted write leaves behind.
    file.write_all(&[0xff, 0x00, 0x00, 0x10, 0x01])
        .expect("append torn bytes");
    drop(file);

    let (_spool, replayed) = replay_ids(tmp.path()).await;
    assert_eq!(replayed, vec![(1, "a".to_owned()), (2, "b".to_owned())]);
}

#[tokio::test]
async fn unreadable_checkpoint_replays_everything() {
    let tmp = TempDir::new();
    {
        let spool = Spool::open(tmp.path()).expect("open spool");
        let (tx, _rx) = mpsc::channel(8);
        spool.enqueue(envelope("a"), &tx).expect("enqueue");
        spool.ack().commit(1).await;
    }
    fs::write(tmp.path().join(CHECKPOINT_FILE), "garbage").expect("corrupt checkpoint");
    let (_spool, replayed) = replay_ids(tmp.path()).await;
    assert_eq!(replayed, vec![(1, "a".to_owned())]);
}

#[test]
fn second_process_cannot_open_a_locked_spool() {
    let tmp = TempDir::new();
    let _first = Spool::open(tmp.path()).expect("open spool");
    let err = Spool::open(tmp.path())
        .err()
        .expect("second open must fail");
    assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
}

#[cfg(unix)]
#[test]
fn spool_files_are_private_and_shared_dirs_are_refused() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new();
    {
        let _spool = Spool::open(tmp.path()).expect("open spool");
        let dir_mode = fs::metadata(tmp.path()).expect("meta").permissions().mode();
        assert_eq!(dir_mode & 0o777, 0o700);
        for path in list_segments(tmp.path()).expect("list") {
            let mode = fs::metadata(path).expect("meta").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o750)).expect("chmod");
    let err = Spool::open(tmp.path())
        .err()
        .expect("group-readable dir refused");
    assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
}
