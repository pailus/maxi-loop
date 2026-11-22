//! Binary encoding of spooled envelopes and the CRC-checked record frame (`F020`).
//!
//! Frame layout (little endian): `len: u32 | crc32: u32 | seq: u64 | payload[len]`, where the
//! CRC covers `seq` and `payload`. A hand-written format keeps raw payload bytes compact
//! (JSON would inflate them 3-4x) without adding a serialization dependency.

use chrono::{DateTime, Utc};
use maxi_loop_performance::model::QueuedTransaction;

use crate::model::{QueuedEnvelope, QueuedEvent};

/// Bytes before the payload in every frame.
pub const FRAME_HEADER_BYTES: usize = 16;

const PAYLOAD_VERSION: u8 = 1;

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    UnsupportedVersion(u8),
    InvalidUtf8,
    InvalidTimestamp,
}

/// Outcome of reading one frame from a segment buffer.
#[derive(Debug, PartialEq, Eq)]
pub enum FrameRead<'a> {
    Record {
        seq: u64,
        payload: &'a [u8],
        consumed: usize,
    },
    /// Clean end of data.
    End,
    /// A torn write or corruption: everything from here on in the segment is unusable.
    Corrupt,
}

/// Allocate a frame buffer with the payload already in place; the header is filled by
/// [`seal_frame`] once the sequence number is known (inside the writer lock).
pub fn unsealed_frame(envelope: &QueuedEnvelope) -> Vec<u8> {
    let mut buf = vec![0u8; FRAME_HEADER_BYTES];
    encode_envelope(envelope, &mut buf);
    buf
}

/// Write `len`, `crc32` and `seq` into the frame header.
pub fn seal_frame(frame: &mut [u8], seq: u64) {
    let payload_len = frame.len() - FRAME_HEADER_BYTES;
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&seq.to_le_bytes());
    hasher.update(&frame[FRAME_HEADER_BYTES..]);
    let crc = hasher.finalize();
    frame[0..4].copy_from_slice(&(payload_len as u32).to_le_bytes());
    frame[4..8].copy_from_slice(&crc.to_le_bytes());
    frame[8..16].copy_from_slice(&seq.to_le_bytes());
}

/// Read the frame at the start of `buf`. The declared length is checked against the bytes
/// actually present, so a corrupt length can never cause an allocation or out-of-bounds read.
pub fn read_frame(buf: &[u8]) -> FrameRead<'_> {
    if buf.is_empty() {
        return FrameRead::End;
    }
    if buf.len() < FRAME_HEADER_BYTES {
        return FrameRead::Corrupt;
    }
    let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let crc = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
    let mut seq_bytes = [0u8; 8];
    seq_bytes.copy_from_slice(&buf[8..16]);
    let seq = u64::from_le_bytes(seq_bytes);
    if buf.len() - FRAME_HEADER_BYTES < len {
        return FrameRead::Corrupt;
    }
    let payload = &buf[FRAME_HEADER_BYTES..FRAME_HEADER_BYTES + len];
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&seq_bytes);
    hasher.update(payload);
    if hasher.finalize() != crc {
        return FrameRead::Corrupt;
    }
    FrameRead::Record {
        seq,
        payload,
        consumed: FRAME_HEADER_BYTES + len,
    }
}

fn encode_envelope(env: &QueuedEnvelope, out: &mut Vec<u8>) {
    out.push(PAYLOAD_VERSION);
    put_i64(out, env.tenant_id);
    put_i64(out, env.workspace_id);
    put_i64(out, env.project_id);
    put_bytes(out, env.envelope_id.as_bytes());
    put_ts(out, env.received_at);
    put_u32(out, env.events.len() as u32);
    for ev in &env.events {
        put_i64(out, ev.tenant_id);
        put_i64(out, ev.workspace_id);
        put_i64(out, ev.project_id);
        put_bytes(out, ev.event_id.as_bytes());
        put_ts(out, ev.received_at);
        put_bytes(out, ev.level.as_bytes());
        put_bytes(out, ev.environment.as_bytes());
        put_opt_str(out, ev.release.as_deref());
        put_bytes(out, ev.platform.as_bytes());
        put_bytes(out, &ev.raw_payload);
    }
    put_u32(out, env.transactions.len() as u32);
    for tx in &env.transactions {
        put_i64(out, tx.tenant_id);
        put_i64(out, tx.workspace_id);
        put_i64(out, tx.project_id);
        put_bytes(out, tx.event_id.as_bytes());
        put_ts(out, tx.received_at);
        put_bytes(out, tx.environment.as_bytes());
        put_opt_str(out, tx.release.as_deref());
        put_bytes(out, tx.platform.as_bytes());
        put_bytes(out, &tx.raw_payload);
    }
}

/// Decode a frame payload back into a queued envelope (with `spool_seq` set by the caller).
pub fn decode_envelope(payload: &[u8]) -> Result<QueuedEnvelope, DecodeError> {
    let mut r = Reader { buf: payload };
    let version = r.u8()?;
    if version != PAYLOAD_VERSION {
        return Err(DecodeError::UnsupportedVersion(version));
    }
    let tenant_id = r.i64()?;
    let workspace_id = r.i64()?;
    let project_id = r.i64()?;
    let envelope_id = r.string()?;
    let received_at = r.ts()?;

    // Counts come from CRC-checked data but are still bounded by the remaining bytes so a
    // logic bug can never trigger a huge pre-allocation.
    let event_count = r.u32()? as usize;
    let mut events = Vec::with_capacity(event_count.min(r.buf.len()));
    for _ in 0..event_count {
        events.push(QueuedEvent {
            tenant_id: r.i64()?,
            workspace_id: r.i64()?,
            project_id: r.i64()?,
            event_id: r.string()?,
            received_at: r.ts()?,
            level: r.string()?,
            environment: r.string()?,
            release: r.opt_string()?,
            platform: r.string()?,
            raw_payload: r.bytes()?.to_vec(),
        });
    }
    let tx_count = r.u32()? as usize;
    let mut transactions = Vec::with_capacity(tx_count.min(r.buf.len()));
    for _ in 0..tx_count {
        transactions.push(QueuedTransaction {
            tenant_id: r.i64()?,
            workspace_id: r.i64()?,
            project_id: r.i64()?,
            event_id: r.string()?,
            received_at: r.ts()?,
            environment: r.string()?,
            release: r.opt_string()?,
            platform: r.string()?,
            raw_payload: r.bytes()?.to_vec(),
        });
    }
    Ok(QueuedEnvelope {
        tenant_id,
        workspace_id,
        project_id,
        envelope_id,
        received_at,
        events,
        transactions,
        spool_seq: None,
    })
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_i64(out: &mut Vec<u8>, v: i64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, v: &[u8]) {
    put_u32(out, v.len() as u32);
    out.extend_from_slice(v);
}

fn put_opt_str(out: &mut Vec<u8>, v: Option<&str>) {
    match v {
        Some(s) => {
            out.push(1);
            put_bytes(out, s.as_bytes());
        }
        None => out.push(0),
    }
}

fn put_ts(out: &mut Vec<u8>, ts: DateTime<Utc>) {
    put_i64(out, ts.timestamp());
    put_u32(out, ts.timestamp_subsec_nanos());
}

struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.buf.len() < n {
            return Err(DecodeError::Truncated);
        }
        let (head, tail) = self.buf.split_at(n);
        self.buf = tail;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, DecodeError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn i64(&mut self) -> Result<i64, DecodeError> {
        let mut a = [0u8; 8];
        a.copy_from_slice(self.take(8)?);
        Ok(i64::from_le_bytes(a))
    }

    fn bytes(&mut self) -> Result<&'a [u8], DecodeError> {
        let len = self.u32()? as usize;
        self.take(len)
    }

    fn string(&mut self) -> Result<String, DecodeError> {
        let b = self.bytes()?;
        String::from_utf8(b.to_vec()).map_err(|_| DecodeError::InvalidUtf8)
    }

    fn opt_string(&mut self) -> Result<Option<String>, DecodeError> {
        match self.u8()? {
            0 => Ok(None),
            _ => self.string().map(Some),
        }
    }

    fn ts(&mut self) -> Result<DateTime<Utc>, DecodeError> {
        let secs = self.i64()?;
        let nanos = self.u32()?;
        DateTime::<Utc>::from_timestamp(secs, nanos).ok_or(DecodeError::InvalidTimestamp)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample_envelope() -> QueuedEnvelope {
        let now = Utc::now();
        QueuedEnvelope {
            tenant_id: 7,
            workspace_id: 8,
            project_id: 9,
            envelope_id: "abc123".to_owned(),
            received_at: now,
            events: vec![QueuedEvent {
                tenant_id: 7,
                workspace_id: 8,
                project_id: 9,
                event_id: "ev1".to_owned(),
                received_at: now,
                level: "error".to_owned(),
                environment: "production".to_owned(),
                release: Some("1.2.3".to_owned()),
                platform: "python".to_owned(),
                raw_payload: vec![0, 159, 255, b'{', b'}'],
            }],
            transactions: vec![QueuedTransaction {
                tenant_id: 7,
                workspace_id: 8,
                project_id: 9,
                event_id: "tx1".to_owned(),
                received_at: now,
                environment: "staging".to_owned(),
                release: None,
                platform: "go".to_owned(),
                raw_payload: b"{\"type\":\"transaction\"}".to_vec(),
            }],
            spool_seq: None,
        }
    }

    fn sealed(env: &QueuedEnvelope, seq: u64) -> Vec<u8> {
        let mut frame = unsealed_frame(env);
        seal_frame(&mut frame, seq);
        frame
    }

    #[test]
    fn envelope_round_trips_through_a_frame_exactly() {
        let env = sample_envelope();
        let frame = sealed(&env, 42);
        let FrameRead::Record {
            seq,
            payload,
            consumed,
        } = read_frame(&frame)
        else {
            panic!("expected a record");
        };
        assert_eq!(seq, 42);
        assert_eq!(consumed, frame.len());
        assert_eq!(decode_envelope(payload).expect("decode"), env);
    }

    #[test]
    fn torn_or_flipped_frames_are_reported_corrupt() {
        let frame = sealed(&sample_envelope(), 1);
        assert_eq!(read_frame(&[]), FrameRead::End);
        assert_eq!(read_frame(&frame[..10]), FrameRead::Corrupt);
        assert_eq!(read_frame(&frame[..frame.len() - 1]), FrameRead::Corrupt);
        let mut flipped = frame.clone();
        let last = flipped.len() - 1;
        flipped[last] ^= 0x01;
        assert_eq!(read_frame(&flipped), FrameRead::Corrupt);
        let mut wrong_seq = frame;
        wrong_seq[8] ^= 0x01;
        assert_eq!(read_frame(&wrong_seq), FrameRead::Corrupt);
    }

    #[test]
    fn truncated_or_unknown_payloads_fail_to_decode() {
        let frame = sealed(&sample_envelope(), 1);
        let payload = &frame[FRAME_HEADER_BYTES..];
        assert_eq!(
            decode_envelope(&payload[..payload.len() - 3]),
            Err(DecodeError::Truncated)
        );
        let mut future = payload.to_vec();
        future[0] = 99;
        assert_eq!(
            decode_envelope(&future),
            Err(DecodeError::UnsupportedVersion(99))
        );
    }
}
