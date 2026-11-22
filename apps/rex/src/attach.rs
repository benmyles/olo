//! The first bytes of a terminal stream may be a snapshot followed immediately
//! by ordinary terminal output. Keep its prefix only until attachment finishes.
use std::borrow::Cow;

use olo_rex_vt::{GhosttyTerminal, MAX_SNAPSHOT_BYTES, SnapshotFramer, VtError, try_decode_snapshot_prefix};

#[derive(Default)]
pub struct Attachment {
    bytes: Vec<u8>,
    framer: SnapshotFramer,
}

pub struct InitialScreen<'a> {
    pub terminal: Option<GhosttyTerminal>,
    pub snapshot_size: usize,
    pub live: Cow<'a, [u8]>,
}

impl Attachment {
    pub fn push<'a>(&mut self, chunk: &'a [u8]) -> Result<Option<InitialScreen<'a>>, VtError> {
        const MAGIC: &[u8] = b"GHOSTSNP";
        let previous = self.bytes.len();
        if previous < MAGIC.len() {
            let overlap = (MAGIC.len() - previous).min(chunk.len());
            if chunk[..overlap] != MAGIC[previous..previous + overlap] {
                let live = if previous == 0 {
                    Cow::Borrowed(chunk)
                } else {
                    let mut raw = std::mem::take(&mut self.bytes);
                    raw.extend_from_slice(chunk);
                    Cow::Owned(raw)
                };
                return Ok(Some(InitialScreen { terminal: None, snapshot_size: 0, live }));
            }
        }

        // A permitted snapshot can end right at the limit with live output in
        // the same frame. That tail need never be copied into the snapshot.
        let take = chunk.len().min(MAX_SNAPSHOT_BYTES - previous);
        let required = previous + take;
        if required > self.bytes.capacity() {
            let growth = self.bytes.capacity().saturating_mul(2).max(required).min(MAX_SNAPSHOT_BYTES);
            self.bytes.try_reserve_exact(growth - previous).map_err(|_| VtError::LimitExceeded)?;
        }
        self.bytes.extend_from_slice(&chunk[..take]);
        let Some(end) = self.framer.advance(&self.bytes)? else { return Ok(None) };
        let decoded = try_decode_snapshot_prefix(&self.bytes[..end], true, None)?.ok_or(VtError::InvalidValue)?;
        Ok(Some(InitialScreen { terminal: Some(decoded.terminal), snapshot_size: end, live: Cow::Borrowed(&chunk[end - previous..]) }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fragmented_attachment_keeps_the_live_tail() {
        let mut term = GhosttyTerminal::new(40, 12).unwrap();
        term.vt_write(b"restored");
        let bytes = term.encode_snapshot().unwrap();
        let mut attach = Attachment::default();
        for part in bytes[..bytes.len() - 1].chunks(7) {
            assert!(attach.push(part).unwrap().is_none());
        }
        let last = [*bytes.last().unwrap(), b'!', b'\n'];
        let result = attach.push(&last).unwrap().unwrap();
        assert!(result.terminal.is_some());
        assert_eq!(result.snapshot_size, bytes.len());
        assert_eq!(&*result.live, b"!\n");
    }

    #[test]
    fn ordinary_text_keeps_a_partial_magic_prefix_and_invalid_snapshots_fail() {
        let mut attach = Attachment::default();
        assert!(attach.push(b"GHO").unwrap().is_none());
        let screen = attach.push(b"ST story").unwrap().unwrap();
        assert!(screen.terminal.is_none());
        assert_eq!(&*screen.live, b"GHOST story");
        assert!(Attachment::default().push(b"GHOSTSNPxx").is_err());
    }
}
