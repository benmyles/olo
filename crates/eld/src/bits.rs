//! Reading bits, most significant first.

pub struct Bits<'a> {
    data: &'a [u8],
    /// The next bit's position.
    at: usize,
}

impl<'a> Bits<'a> {
    pub fn new(data: &'a [u8]) -> Bits<'a> {
        Bits { data, at: 0 }
    }

    /// The next `n` bits (up to 32) as a number; None past the end.
    pub fn read(&mut self, n: u32) -> Option<u32> {
        if n == 0 {
            return Some(0);
        }
        if self.at + n as usize > self.data.len() * 8 {
            return None;
        }
        let mut v: u64 = 0;
        let mut left = n;
        while left > 0 {
            let byte = self.data[self.at / 8];
            let used = (self.at % 8) as u32;
            let take = (8 - used).min(left);
            let bits = (byte >> (8 - used - take)) & ((1u16 << take) - 1) as u8;
            v = (v << take) | bits as u64;
            self.at += take as usize;
            left -= take;
        }
        Some(v as u32)
    }

    pub fn bit(&mut self) -> Option<bool> {
        self.read(1).map(|b| b == 1)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_across_bytes() {
        let mut b = super::Bits::new(&[0b1010_1100, 0b0101_0011]);
        assert_eq!(b.read(3), Some(0b101));
        assert_eq!(b.read(7), Some(0b011_0001));
        assert_eq!(b.read(6), Some(0b01_0011));
        assert_eq!(b.read(1), None);
    }
}
