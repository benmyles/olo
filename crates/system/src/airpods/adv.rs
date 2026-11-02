//! What AirPods broadcast over Bluetooth LE: Apple's "proximity pairing"
//! message, in Apple's manufacturer data (company 0x004C). They send it
//! from a private address that changes, while they're out of their case
//! or its lid is open, connected or not: their model, where each bud is
//! and their levels, coarsely in the clear and to the percent in a block
//! made with a key only their owner has ([`super::proto::Keys`]).
//!
//! The manufacturer data (after the company): 07 (the kind), its length,
//! 01, the model (2 bytes, little-endian), status, the buds' levels (a
//! nibble each), charging and the case's level, the lid, the colour,
//! whether they're connected, then 16 encrypted bytes.

use aes::cipher::{BlockCipherDecrypt, BlockCipherEncrypt, KeyInit};

/// The kind of message AirPods broadcast.
pub const PROXIMITY: u8 = 0x07;

/// A bud or the case, as broadcast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Charge {
    /// 0–100; None when it isn't said (a bud away, the case closed).
    pub percent: Option<u8>,
    pub charging: bool,
}

/// One proximity message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proximity {
    /// Apple's product id (0x2024: AirPods Pro, 2nd generation, USB-C).
    pub model: u16,
    pub left: Charge,
    pub right: Charge,
    pub case: Charge,
    pub left_in_ear: bool,
    pub right_in_ear: bool,
    /// The bud broadcasting is in the case (and its lid is open, or it
    /// wouldn't be broadcasting).
    pub in_case: bool,
    /// Connected to a phone or computer (any, not only this one).
    pub connected: bool,
    /// The left bud is the one broadcasting: whose level comes first.
    left_primary: bool,
    /// The 16 encrypted bytes, for [`decrypt`].
    sealed: [u8; 16],
}

fn nibble(n: u8) -> Option<u8> {
    (n <= 10).then_some(n * 10)
}

/// Read Apple's manufacturer data (without the company id); None if it
/// isn't a proximity message.
pub fn parse(data: &[u8]) -> Option<Proximity> {
    if data.len() < 27 || data[0] != PROXIMITY || (data[1] as usize) < 25 {
        return None;
    }
    let status = data[5];
    // Which bud is broadcasting decides which nibble and bit is whose.
    let left_primary = status & 0x20 != 0;
    let in_case = status & 0x40 != 0;
    let swap = left_primary ^ in_case;
    let (high, low) = (data[6] >> 4, data[6] & 0x0F);
    let (left_level, right_level) = if left_primary { (low, high) } else { (high, low) };
    let charging = data[7] >> 4;
    let (left_charging, right_charging) = if left_primary { (charging & 0x01 != 0, charging & 0x02 != 0) } else { (charging & 0x02 != 0, charging & 0x01 != 0) };
    Some(Proximity {
        model: u16::from_le_bytes([data[3], data[4]]),
        left: Charge { percent: nibble(left_level), charging: left_charging },
        right: Charge { percent: nibble(right_level), charging: right_charging },
        case: Charge { percent: nibble(data[7] & 0x0F), charging: charging & 0x04 != 0 },
        left_in_ear: status & if swap { 0x02 } else { 0x08 } != 0,
        right_in_ear: status & if swap { 0x08 } else { 0x02 } != 0,
        in_case,
        connected: data[10] != 0,
        left_primary,
        sealed: data[data.len() - 16..].try_into().ok()?,
    })
}

/// Levels to the percent from the sealed block, with the encryption key
/// the AirPods gave: left, right and case.
pub fn decrypt(p: &Proximity, key: &[u8; 16]) -> [Charge; 3] {
    let cipher = aes::Aes128::new(&(*key).into());
    let mut block = p.sealed.into();
    cipher.decrypt_block(&mut block);
    let open: [u8; 16] = block.into();
    let charge = |b: u8| if b == 0xFF { Charge::default() } else { Charge { percent: Some((b & 0x7F).min(100)), charging: b & 0x80 != 0 } };
    let (l, r) = if p.left_primary { (open[1], open[2]) } else { (open[2], open[1]) };
    [charge(l), charge(r), charge(open[3])]
}

/// Whether the private address `addr` ("AA:BB:CC:DD:EE:FF") is one made
/// by the AirPods with identity key `irk` (as the AirPods give it,
/// least significant byte first). A resolvable private address is a
/// random half and its hash under the key: ah(k, r) = e(k, 0…0 ‖ r)
/// mod 2²⁴, big-endian.
pub fn resolves(addr: &str, irk: &[u8; 16]) -> bool {
    let bytes: Vec<u8> = addr.split(':').filter_map(|b| u8::from_str_radix(b, 16).ok()).collect();
    let Ok(bytes) = <[u8; 6]>::try_from(bytes) else { return false };
    // Resolvable addresses have 01 as their top two bits.
    if bytes[0] >> 6 != 0b01 {
        return false;
    }
    let mut key = *irk;
    key.reverse();
    let mut block = [0u8; 16];
    block[13..].copy_from_slice(&bytes[..3]);
    let mut b = block.into();
    aes::Aes128::new(&key.into()).encrypt_block(&mut b);
    let out: [u8; 16] = b.into();
    out[13..] == bytes[3..]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(status: u8, levels: u8, charge: u8) -> Vec<u8> {
        let mut d = vec![0x07, 0x19, 0x01, 0x24, 0x20, status, levels, charge, 0x00, 0x00, 0x01];
        d.extend([0u8; 16]);
        d
    }

    #[test]
    fn reads_levels_and_ears() {
        // Left broadcasting, both in ears: left 80, right 90, case 50
        // charging.
        let p = parse(&message(0x20 | 0x02 | 0x08, 0x98, 0x45)).unwrap();
        assert_eq!(p.model, 0x2024);
        assert_eq!((p.left.percent, p.right.percent, p.case.percent, p.case.charging), (Some(80), Some(90), Some(50), true));
        assert!(p.left_in_ear && p.right_in_ear && p.connected && !p.in_case);
        // Right broadcasting, only the left in an ear (the second bud's
        // bit, as the broadcasting one sees it); the case unknown.
        let p = parse(&message(0x08, 0x7F, 0x2F)).unwrap();
        assert_eq!((p.left.percent, p.right.percent, p.case.percent), (Some(70), None, None));
        assert!(p.left_in_ear && !p.right_in_ear && p.left.charging);
        assert_eq!(parse(&[0x10, 0x05, 0, 0, 0]), None);
    }

    #[test]
    fn opens_the_sealed_levels() {
        let key = [7u8; 16];
        let mut open = [0u8; 16];
        open[1..4].copy_from_slice(&[0x80 | 57, 63, 0xFF]);
        let mut sealed = open.into();
        aes::Aes128::new(&key.into()).encrypt_block(&mut sealed);
        let mut d = message(0x20, 0x55, 0x05);
        let n = d.len();
        d[n - 16..].copy_from_slice(&<[u8; 16]>::from(sealed));
        let [l, r, c] = decrypt(&parse(&d).unwrap(), &key);
        assert_eq!((l.percent, l.charging, r.percent, c.percent), (Some(57), true, Some(63), None));
    }

    #[test]
    fn resolves_private_addresses() {
        // The Core Specification's sample (Vol 3, Part H, D.7): IRK
        // ec0234a357c8ad05341010a60a397d9b, prand 708194, hash 0dfbaa.
        let mut irk: [u8; 16] = (0..16).map(|i| u8::from_str_radix(&"ec0234a357c8ad05341010a60a397d9b"[i * 2..i * 2 + 2], 16).unwrap()).collect::<Vec<_>>().try_into().unwrap();
        irk.reverse();
        assert!(resolves("70:81:94:0D:FB:AA", &irk));
        assert!(!resolves("70:81:94:0D:FB:AB", &irk));
        assert!(!resolves("30:81:94:0D:FB:AA", &irk));
    }
}
