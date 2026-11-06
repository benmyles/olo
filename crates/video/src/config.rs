//! The decoder configuration records an MP4's sample entry carries for
//! HEVC (`hvcC`, ISO/IEC 14496-15 §8.3.3) and AV1 (`av1C`, AV1-ISOBMFF
//! §2.3), made from what the encoder gave: HEVC's parameter sets (its
//! profile, tier, level, chroma format and bit depths read from the SPS)
//! and AV1's sequence header OBU (its profile, level, tier and colour
//! layout read from it).

/// Bits read most significant first.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.at / 8)?;
        let b = (byte >> (7 - self.at % 8)) & 1;
        self.at += 1;
        Some(u32::from(b))
    }

    fn bits(&mut self, n: u32) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Some(v)
    }

    fn skip(&mut self, n: usize) -> Option<()> {
        self.at += n;
        (self.at <= self.data.len() * 8).then_some(())
    }

    /// Exp-Golomb, unsigned (H.26x's ue(v)).
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        Some((1u32 << zeros) - 1 + self.bits(zeros)?)
    }
}

/// A NAL unit's payload without its emulation prevention bytes (each
/// 0x03 after two zeros).
fn rbsp(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// HEVC's `hvcC` from its VPS, SPS and PPS (NAL units with their
/// two-byte headers), lengths four bytes.
pub fn hvcc(vps: &[u8], sps: &[u8], pps: &[u8]) -> Option<Vec<u8>> {
    let r = rbsp(sps);
    let mut b = Bits { data: r.get(2..)?, at: 0 };
    b.skip(4)?; // sps_video_parameter_set_id
    let sub_layers = b.bits(3)?; // sps_max_sub_layers_minus1
    let nested = b.bit()?;
    // profile_tier_level: the general part, 12 bytes from here.
    let start = b.at / 8;
    let ptl = b.data.get(start..start + 12)?.to_vec();
    b.skip(96)?;
    let (mut profile_present, mut level_present) = (Vec::new(), Vec::new());
    for _ in 0..sub_layers {
        profile_present.push(b.bit()?);
        level_present.push(b.bit()?);
    }
    if sub_layers > 0 {
        b.skip(2 * (8 - sub_layers as usize))?;
    }
    for i in 0..sub_layers as usize {
        if profile_present[i] == 1 {
            b.skip(88)?;
        }
        if level_present[i] == 1 {
            b.skip(8)?;
        }
    }
    b.ue()?; // sps_seq_parameter_set_id
    let chroma = b.ue()?;
    if chroma == 3 {
        b.bit()?;
    }
    b.ue()?; // width
    b.ue()?; // height
    if b.bit()? == 1 {
        for _ in 0..4 {
            b.ue()?;
        }
    }
    let depth_luma = b.ue()?;
    let depth_chroma = b.ue()?;
    let mut out = vec![1];
    out.extend_from_slice(&ptl); // profile space, tier, profile; compatibility; constraints; level
    out.extend_from_slice(&[0xf0, 0x00]); // min_spatial_segmentation_idc 0
    out.push(0xfc); // parallelismType 0
    out.push(0xfc | (chroma as u8 & 3));
    out.push(0xf8 | (depth_luma as u8 & 7));
    out.push(0xf8 | (depth_chroma as u8 & 7));
    out.extend_from_slice(&[0, 0]); // avgFrameRate unknown
    // constantFrameRate 0, numTemporalLayers, temporalIdNested,
    // lengthSizeMinusOne 3.
    out.push((((sub_layers + 1) as u8 & 7) << 3) | ((nested as u8) << 2) | 3);
    out.push(3);
    for (kind, nal) in [(32u8, vps), (33, sps), (34, pps)] {
        out.push(0x80 | kind); // array_completeness
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&(nal.len() as u16).to_be_bytes());
        out.extend_from_slice(nal);
    }
    Some(out)
}

/// An OBU's type, header length and payload length (`leb128`-sized; the
/// rest of `b` if it has no size), from the start of `b`.
pub fn obu(b: &[u8]) -> Option<(u8, usize, usize)> {
    let h = *b.first()?;
    let kind = (h >> 3) & 0xf;
    let mut head = if h & 4 != 0 { 2 } else { 1 };
    let size = if h & 2 != 0 {
        let mut v = 0usize;
        let mut i = 0;
        loop {
            let byte = *b.get(head)?;
            head += 1;
            v |= usize::from(byte & 0x7f) << (7 * i);
            i += 1;
            if byte & 0x80 == 0 {
                break;
            }
            if i == 8 {
                return None;
            }
        }
        v
    } else {
        b.len().checked_sub(head)?
    };
    (head + size <= b.len()).then_some((kind, head, size))
}

/// AV1's `av1C` from its sequence header OBU (whole, with its header and
/// size), which it carries as its configOBUs.
pub fn av1c(seq: &[u8]) -> Option<Vec<u8>> {
    let (kind, head, size) = obu(seq)?;
    if kind != 1 {
        return None;
    }
    let mut b = Bits { data: &seq[head..head + size], at: 0 };
    let profile = b.bits(3)?;
    b.bit()?; // still_picture
    let reduced = b.bit()?;
    let (level, tier);
    if reduced == 1 {
        level = b.bits(5)?;
        tier = 0;
    } else {
        let mut decoder_model = false;
        let mut delay_len = 0;
        if b.bit()? == 1 {
            // timing_info
            b.skip(64)?;
            if b.bit()? == 1 {
                // uvlc num_ticks_per_picture_minus_1
                let mut zeros = 0;
                while b.bit()? == 0 {
                    zeros += 1;
                    if zeros >= 32 {
                        return None;
                    }
                }
                b.skip(zeros)?;
            }
            if b.bit()? == 1 {
                decoder_model = true;
                delay_len = b.bits(5)? + 1;
                b.skip(32 + 5 + 5)?;
            }
        }
        let display_delay = b.bit()? == 1;
        let points = b.bits(5)? + 1;
        let (mut first_level, mut first_tier) = (0, 0);
        for i in 0..points {
            b.skip(12)?; // operating_point_idc
            let l = b.bits(5)?;
            let t = if l > 7 { b.bit()? } else { 0 };
            if decoder_model && b.bit()? == 1 {
                b.skip(2 * delay_len as usize + 1)?;
            }
            if display_delay && b.bit()? == 1 {
                b.skip(4)?;
            }
            if i == 0 {
                (first_level, first_tier) = (l, t);
            }
        }
        (level, tier) = (first_level, first_tier);
    }
    let w_bits = b.bits(4)? + 1;
    let h_bits = b.bits(4)? + 1;
    b.skip((w_bits + h_bits) as usize)?;
    if reduced == 0 && b.bit()? == 1 {
        b.skip(7)?; // frame id lengths
    }
    b.skip(3)?; // 128 superblocks, filter intra, intra edge filter
    if reduced == 0 {
        b.skip(4)?; // interintra, masked compound, warped motion, dual filter
        let order_hint = b.bit()?;
        if order_hint == 1 {
            b.skip(2)?;
        }
        let force_screen = if b.bit()? == 1 { 2 } else { b.bit()? };
        if force_screen > 0 && b.bit()? == 0 {
            b.bit()?;
        }
        if order_hint == 1 {
            b.skip(3)?;
        }
    }
    b.skip(3)?; // superres, cdef, restoration
    // color_config
    let high = b.bit()?;
    let twelve = if profile == 2 && high == 1 { b.bit()? } else { 0 };
    let mono = if profile == 1 { 0 } else { b.bit()? };
    let (mut cp, mut tc, mut mc) = (2, 2, 2);
    if b.bit()? == 1 {
        (cp, tc, mc) = (b.bits(8)?, b.bits(8)?, b.bits(8)?);
    }
    let (sx, sy, position);
    if mono == 1 {
        (sx, sy, position) = (1, 1, 0);
    } else if cp == 1 && tc == 13 && mc == 0 {
        (sx, sy, position) = (0, 0, 0);
    } else {
        b.bit()?; // color_range
        (sx, sy) = match profile {
            0 => (1, 1),
            1 => (0, 0),
            _ if twelve == 1 => {
                let x = b.bit()?;
                (x, if x == 1 { b.bit()? } else { 0 })
            }
            _ => (1, 0),
        };
        position = if sx == 1 && sy == 1 { b.bits(2)? } else { 0 };
    }
    let mut out = vec![0x81, ((profile as u8) << 5) | (level as u8 & 0x1f)];
    out.push(((tier as u8) << 7) | ((high as u8) << 6) | ((twelve as u8) << 5) | ((mono as u8) << 4) | ((sx as u8) << 3) | ((sy as u8) << 2) | (position as u8 & 3));
    out.push(0);
    out.extend_from_slice(seq);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_golomb_and_emulation_prevention() {
        // 1 → 0, 010 → 1, 011 → 2, 00100 → 3.
        let mut b = Bits { data: &[0b1010_0110, 0b0100_0000], at: 0 };
        assert_eq!((b.ue(), b.ue(), b.ue(), b.ue()), (Some(0), Some(1), Some(2), Some(3)));
        assert_eq!(rbsp(&[0, 0, 3, 1, 0, 0, 3, 0]), vec![0, 0, 1, 0, 0, 0]);
    }

    #[test]
    fn obus_are_read() {
        // A temporal delimiter (type 2) with a size of 0, then a padding
        // OBU of 3 bytes.
        assert_eq!(obu(&[0x12, 0x00]), Some((2, 2, 0)));
        assert_eq!(obu(&[0x7a, 0x03, 1, 2, 3]), Some((15, 2, 3)));
        assert_eq!(obu(&[0x7a, 0x05, 1]), None, "short");
    }

    #[test]
    fn a_reduced_sequence_header_says_its_level() {
        // Profile 0, still, reduced, level 8; 16 × 16 (4-bit sizes, 15 and
        // 15); then the tools and colour bits, all zero: 8-bit 4:2:0.
        let mut bits = String::new();
        bits += "000"; // profile
        bits += "1"; // still_picture
        bits += "1"; // reduced
        bits += "01000"; // level 8
        bits += "0011"; // width bits minus 1 = 3
        bits += "0011";
        bits += "1111"; // width minus 1 = 15
        bits += "1111";
        bits += "000"; // superblocks, filter intra, intra edge
        bits += "000"; // superres, cdef, restoration
        bits += "0"; // high bitdepth
        bits += "0"; // mono
        bits += "0"; // colour description
        bits += "0"; // colour range
        bits += "00"; // chroma sample position
        while bits.len() % 8 != 0 {
            bits += "0";
        }
        let payload: Vec<u8> = bits.as_bytes().chunks(8).map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 2).unwrap()).collect();
        let mut seq = vec![0x0a, payload.len() as u8];
        seq.extend_from_slice(&payload);
        let c = av1c(&seq).unwrap();
        assert_eq!(c[0], 0x81);
        assert_eq!(c[1], 8, "profile 0, level 8");
        assert_eq!(c[2], 0b0000_1100, "8-bit, 4:2:0");
        assert_eq!(&c[4..], &seq[..]);
    }
}
