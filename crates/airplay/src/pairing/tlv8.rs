//! TLV8, the form of HomeKit's pairing messages: a type, a length and a
//! value, each a byte at most 255 long; a longer value is said in
//! pieces of the same type one after another.

/// The types used here (HomeKit's numbers).
pub const METHOD: u8 = 0x00;
pub const IDENTIFIER: u8 = 0x01;
pub const SALT: u8 = 0x02;
pub const PUBLIC_KEY: u8 = 0x03;
pub const PROOF: u8 = 0x04;
pub const ENCRYPTED_DATA: u8 = 0x05;
pub const STATE: u8 = 0x06;
pub const ERROR: u8 = 0x07;
pub const BACK_OFF: u8 = 0x08;
pub const SIGNATURE: u8 = 0x0a;

/// Values by their types, in the order written.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tlv(pub Vec<(u8, Vec<u8>)>);

impl Tlv {
    pub fn with(mut self, kind: u8, value: impl Into<Vec<u8>>) -> Tlv {
        self.0.push((kind, value.into()));
        self
    }

    pub fn get(&self, kind: u8) -> Option<&[u8]> {
        self.0.iter().find(|(k, _)| *k == kind).map(|(_, v)| v.as_slice())
    }

    /// A value that is one byte (a state, an error).
    pub fn byte(&self, kind: u8) -> Option<u8> {
        self.get(kind).and_then(|v| v.first().copied())
    }

    pub fn write(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (kind, value) in &self.0 {
            // (An empty value is still said, once.)
            let mut pieces = value.chunks(255).peekable();
            if pieces.peek().is_none() {
                out.extend([*kind, 0]);
            }
            for piece in pieces {
                out.extend([*kind, piece.len() as u8]);
                out.extend(piece);
            }
        }
        out
    }

    /// What `data` says; pieces of one type that follow one another are
    /// one value. None if it's cut short.
    pub fn read(mut data: &[u8]) -> Option<Tlv> {
        let mut out: Vec<(u8, Vec<u8>)> = Vec::new();
        let mut full = false;
        while let [kind, len, rest @ ..] = data {
            let value = rest.get(..*len as usize)?;
            match out.last_mut() {
                Some((last, whole)) if last == kind && full => whole.extend(value),
                _ => out.push((*kind, value.to_vec())),
            }
            full = *len == 255;
            data = &rest[*len as usize..];
        }
        data.is_empty().then_some(Tlv(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_and_read_back_long_values_in_pieces() {
        let key: Vec<u8> = (0..384).map(|i| i as u8).collect();
        let said = Tlv::default().with(STATE, [3]).with(PUBLIC_KEY, key.clone()).with(PROOF, vec![7; 64]).with(METHOD, Vec::new());
        let bytes = said.write();
        // The key: two pieces of 255 and 129.
        assert_eq!(&bytes[..5], &[STATE, 1, 3, PUBLIC_KEY, 255]);
        assert_eq!(bytes[3 + 2 + 255], PUBLIC_KEY);
        assert_eq!(bytes[3 + 2 + 255 + 1], 129);
        let read = Tlv::read(&bytes).unwrap();
        assert_eq!(read, said);
        assert_eq!(read.get(PUBLIC_KEY), Some(key.as_slice()));
        assert_eq!((read.byte(STATE), read.byte(ERROR), read.get(METHOD)), (Some(3), None, Some(&[][..])));
    }

    #[test]
    fn two_values_of_one_type_are_two_unless_the_first_was_full() {
        // (Two short values of a type side by side stay apart: only a
        // piece of 255 is carried on.)
        let read = Tlv::read(&[IDENTIFIER, 1, b'a', IDENTIFIER, 1, b'b']).unwrap();
        assert_eq!(read.0.len(), 2);
        assert_eq!(Tlv::read(&[STATE, 2, 1]), None);
        assert_eq!(Tlv::read(&[STATE]), None);
        assert_eq!(Tlv::read(&[]), Some(Tlv::default()));
    }
}
