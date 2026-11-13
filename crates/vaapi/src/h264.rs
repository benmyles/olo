//! H.264 through VA-API, where the driver encodes it: RPM Fusion's iHD
//! on Intel's GPUs (Fedora's own leaves H.264 out), Mesa's freeworld
//! drivers on AMD's. It's what AirPlay's mirroring takes, and pictures in
//! the GPU's memory go in without a copy through the CPU
//! ([`H264::encode_prime`]).
//!
//! It works as FFmpeg's `h264_vaapi` and Chrome's encoder do: High
//! profile (Main or Constrained Baseline where that's all the driver
//! has), a key (IDR) frame every two seconds, and P frames each predicted
//! from the one before (no B frames: nothing is reordered, so each frame
//! is shown as it comes), on two reconstructed surfaces taken in turn.
//! The sequence and picture parameter sets are written here and handed
//! over as packed headers (BT.709 in limited range, and a VUI that says
//! no frame waits for a later one); the driver writes the slices. The
//! rate: a quality with a cap where the driver has one (QVBR, as NVENC's
//! `targetQuality` with `maxBitRate`), else that quality's quantizer for
//! every frame (CQP).

use std::path::Path;

use super::{Display, Input, Made, Prime, Writer, nvidia, render_nodes, va};

/// How finely pictures are coded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rate {
    /// Quality `q` (1–51, as a quantizer: lower is finer), at most `cap`
    /// bits a second.
    Quality(u8, u32),
    /// Quantizer `qp` (1–51) for every frame, as many bits as it takes.
    Constant(u8),
}

/// The quality a screen is coded at ([`Rate::Quality`]): measured on a
/// 1280 × 800 screen typed into (a Terminal on a photograph,
/// `vaapi_h264_sizes`, three seconds at 30), 46.7 dB in 199 KB, where
/// NVENC's H.264 at its quality 20 keeps 46.4 dB in 322 KB (and a
/// constant quantizer of 18 here, 46.5 dB in 238 KB).
pub const QUALITY: u8 = 18;

/// frame_num's bits (a key frame every two seconds is 120 frames at 60).
const LOG2_MAX_FRAME_NUM: u32 = 8;

/// A profile the driver may encode, and what its parameter sets say.
#[derive(Debug, Clone, Copy)]
struct Profile {
    va: va::VAProfile,
    idc: u8,
    /// constraint_set0_flag to constraint_set5_flag, and two zero bits.
    constraints: u8,
    cabac: bool,
    transform_8x8: bool,
}

/// High, else Main, else Constrained Baseline (what Main and Baseline
/// decoders can both play).
const PROFILES: [Profile; 3] = [
    Profile { va: va::VAProfileH264High, idc: 100, constraints: 0, cabac: true, transform_8x8: true },
    Profile { va: va::VAProfileH264Main, idc: 77, constraints: 0b0100_0000, cabac: true, transform_8x8: false },
    Profile { va: va::VAProfileH264ConstrainedBaseline, idc: 66, constraints: 0b1100_0000, cabac: false, transform_8x8: false },
];

impl Display {
    /// The profile H.264 is encoded as, and the entrypoint (the
    /// low-power one first: Intel's from Gen12 on have only that).
    fn h264(&self) -> Option<(Profile, va::VAEntrypoint)> {
        PROFILES.iter().find_map(|p| {
            let list = self.entrypoints(p.va);
            [va::VAEntrypointEncSliceLP, va::VAEntrypointEncSlice].into_iter().find(|e| list.contains(e)).map(|e| (*p, e))
        })
    }
}

/// The largest picture `node`'s driver encodes as H.264, if it does.
pub(crate) fn most(d: &Display) -> Option<(u32, u32)> {
    let (p, e) = d.h264()?;
    let v = d.attributes(p.va, e, &[va::VAConfigAttribMaxPictureWidth, va::VAConfigAttribMaxPictureHeight]).ok()?;
    Some((v[0], v[1]))
}

/// The first GPU that encodes H.264 through VA-API (never NVIDIA's).
pub fn node() -> Option<std::path::PathBuf> {
    render_nodes().into_iter().filter(|n| !nvidia(n)).find(|n| Display::open(n).is_ok_and(|d| d.h264().is_some()))
}

/// What the first sequence parameter set in Annex B `bytes` says its
/// pictures' colour is: H.273's matrix (2: not said), and whether they're
/// full range.
pub fn colour(bytes: &[u8]) -> Option<(u8, bool)> {
    use cros_codecs::codec::h264::parser::{Nalu, NaluType, Parser};
    let mut cursor = std::io::Cursor::new(bytes);
    let mut parser = Parser::default();
    while let Ok(nalu) = Nalu::next(&mut cursor) {
        if nalu.header.type_ == NaluType::Sps {
            let sps = parser.parse_sps(&nalu).ok()?;
            let vui = &sps.vui_parameters;
            let said = sps.vui_parameters_present_flag && vui.video_signal_type_present_flag;
            let matrix = if said && vui.colour_description_present_flag { vui.matrix_coefficients } else { 2 };
            return Some((matrix, said && vui.video_full_range_flag));
        }
    }
    None
}

/// An H.264 encoder on a GPU's VA-API driver.
pub struct H264 {
    d: Display,
    config: va::VAConfigID,
    context: va::VAContextID,
    input: Input,
    /// Two reconstructed pictures, taken in turn.
    recon: [va::VASurfaceID; 2],
    coded: va::VABufferID,
    width: u32,
    height: u32,
    /// The picture in macroblocks.
    mbs: (u32, u32),
    profile: Profile,
    level: u8,
    sps: Vec<u8>,
    pps: Vec<u8>,
    rate: Rate,
    /// The rate control asked of the driver (VA_RC_QVBR or VA_RC_CQP).
    rc: u32,
    fps: u32,
    /// The quantizer the picture parameter set starts slices at (the
    /// frames' own, at a constant quantizer).
    init_qp: u8,
    /// Frames encoded, and since the last key frame.
    frame: u64,
    since_key: u64,
    /// A key frame every this many.
    gop: u64,
    /// The next key frame's idr_pic_id (two in a row differ).
    idr_id: u16,
    /// The slot the last frame was kept in.
    slot: u8,
}

// (Its display and surfaces are used from one thread at a time, through
// `&mut`.)
unsafe impl Send for H264 {}

impl H264 {
    /// One for `width` × `height` (even) pictures at up to `fps` a second
    /// on `node`'s GPU (None: the first that encodes H.264), as finely as
    /// `rate` says.
    pub fn new(node: Option<&Path>, width: u32, height: u32, fps: u32, rate: Rate) -> Result<H264, String> {
        if width == 0 || height == 0 || width % 2 == 1 || height % 2 == 1 {
            return Err(format!("{width}×{height} isn't an even size"));
        }
        let node = match node {
            Some(n) => n.to_owned(),
            None => self::node().ok_or("no GPU here encodes H.264 through VA-API (Intel's needs RPM Fusion's intel-media-driver)")?,
        };
        let d = Display::open(&node)?;
        let (profile, entry) = d.h264().ok_or_else(|| format!("{} ({}) doesn't encode H.264", node.display(), d.vendor))?;
        let v = d.attributes(
            profile.va,
            entry,
            &[va::VAConfigAttribRTFormat, va::VAConfigAttribRateControl, va::VAConfigAttribEncPackedHeaders, va::VAConfigAttribMaxPictureWidth, va::VAConfigAttribMaxPictureHeight],
        )?;
        let (rt, rcs, packed, max_w, max_h) = (v[0], v[1], v[2], v[3], v[4]);
        if rt & va::VA_RT_FORMAT_YUV420 == 0 {
            return Err("the driver doesn't take 4:2:0 pictures for H.264".into());
        }
        let want = va::VA_ENC_PACKED_HEADER_SEQUENCE | va::VA_ENC_PACKED_HEADER_PICTURE;
        if packed == va::VA_ATTRIB_NOT_SUPPORTED || packed & want != want {
            return Err("the driver doesn't take H.264's parameter sets written for it".into());
        }
        if max_w != va::VA_ATTRIB_NOT_SUPPORTED && max_h != va::VA_ATTRIB_NOT_SUPPORTED && (width > max_w || height > max_h) {
            return Err(format!("{width}×{height} is larger than the GPU encodes as H.264 ({max_w}×{max_h})"));
        }
        // A quality with a cap where the driver has one, else its
        // quantizer throughout.
        let rate = match rate {
            Rate::Quality(q, cap) => Rate::Quality(q.clamp(1, 51), cap.max(1)),
            Rate::Constant(qp) => Rate::Constant(qp.clamp(1, 51)),
        };
        let rc = match rate {
            Rate::Quality(..) if rcs != va::VA_ATTRIB_NOT_SUPPORTED && rcs & va::VA_RC_QVBR != 0 => va::VA_RC_QVBR,
            _ if rcs != va::VA_ATTRIB_NOT_SUPPORTED && rcs & va::VA_RC_CQP != 0 => va::VA_RC_CQP,
            _ => return Err("the driver has neither a quality with a cap nor a constant quantizer for H.264".into()),
        };
        let init_qp = match (rc, rate) {
            (va::VA_RC_CQP, Rate::Quality(q, _) | Rate::Constant(q)) => q,
            // (The driver chooses each frame's, from 26.)
            _ => 26,
        };
        let fps = fps.clamp(1, 240);
        let mbs = (width.div_ceil(16), height.div_ceil(16));
        let cap = match rate {
            Rate::Quality(_, cap) => Some(cap),
            Rate::Constant(_) => None,
        };
        let level = level(mbs, fps, cap, profile.idc == 100);
        let (sw, sh) = (mbs.0 * 16, mbs.1 * 16);
        let made = d.encoding(profile.va, entry, &mut [(va::VAConfigAttribRateControl, rc), (va::VAConfigAttribEncPackedHeaders, want)], sw, sh)?;
        let sps = sps(&profile, level, mbs, (sw - width, sh - height));
        let pps = pps(&profile, init_qp);
        let mut e = H264 {
            config: made.config,
            context: made.context,
            input: Input::new(made.surfaces[0], width, height),
            recon: [made.surfaces[1], made.surfaces[2]],
            coded: made.coded,
            width,
            height,
            mbs,
            profile,
            level,
            sps,
            pps,
            rate,
            rc,
            fps,
            init_qp,
            frame: 0,
            since_key: 0,
            gop: u64::from(fps) * 2,
            idr_id: 0,
            slot: 0,
            d,
        };
        e.input.open(&e.d, sw, sh)?;
        Ok(e)
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The driver, as it names itself.
    pub fn driver(&self) -> &str {
        &self.d.vendor
    }

    /// How it codes: "High 4.2, quality 22 at most 24.9 Mbit/s".
    pub fn describe(&self) -> String {
        let profile = match self.profile.idc {
            100 => "High",
            77 => "Main",
            _ => "Constrained Baseline",
        };
        let rate = match (self.rc, self.rate) {
            (va::VA_RC_QVBR, Rate::Quality(q, cap)) => format!("quality {q} at most {:.1} Mbit/s", f64::from(cap) / 1e6),
            (_, Rate::Quality(q, _) | Rate::Constant(q)) => format!("quantizer {q}"),
        };
        format!("{profile} {}.{}, {rate}", self.level / 10, self.level % 10)
    }

    /// The GPU it encodes on, as its render node's device number (to
    /// tell whether a dmabuf from elsewhere is in its memory, for
    /// [`encode_prime`](Self::encode_prime)).
    pub fn device(&self) -> Option<u64> {
        self.d.device()
    }

    /// The parameter sets every key frame starts with (SPS, then PPS,
    /// each with its start code).
    pub fn parameter_sets(&self) -> (&[u8], &[u8]) {
        (&self.sps, &self.pps)
    }

    /// One picture (BGRA rows, or RGBA with `rgba`; `stride` bytes a
    /// row, at least this size) encoded: its Annex B bytes (the
    /// parameter sets first, with a key frame), and whether it's a key
    /// frame.
    pub fn encode(&mut self, data: &[u8], stride: usize, rgba: bool) -> Result<(Vec<u8>, bool), String> {
        let (w, h) = (self.width as usize, self.height as usize);
        if stride < w * 4 || data.len() < stride * (h - 1) + w * 4 {
            return Err("the picture is smaller than the video".into());
        }
        self.input.upload(&self.d, data, stride, rgba)?;
        self.encode_input()
    }

    /// A picture in GPU memory (a capture's dmabuf) encoded, as
    /// [`encode`](Self::encode) does one in memory: made NV12 by the GPU's
    /// video processor straight into the input surface, never copied
    /// through the CPU. Its top left `width` × `height` are the video.
    pub fn encode_prime(&mut self, picture: &Prime) -> Result<(Vec<u8>, bool), String> {
        if picture.width < self.width || picture.height < self.height {
            return Err("the picture is smaller than the video".into());
        }
        self.input.convert(&self.d, picture)?;
        self.encode_input()
    }

    /// The next frame a key frame, whenever the gap would be: a receiver
    /// that lost a picture asks for one.
    pub fn key_next(&mut self) {
        self.since_key = 0;
    }

    /// The input surface, filled, encoded.
    fn encode_input(&mut self) -> Result<(Vec<u8>, bool), String> {
        let key = self.since_key == 0 || self.since_key >= self.gop;
        if key {
            self.since_key = 0;
        }
        let from = self.slot;
        let slot = if key { 0 } else { 1 - from };
        let mut bufs = Vec::new();
        let out = self.code(key, slot, from, &mut bufs);
        self.d.destroy(bufs);
        let out = out?;
        if key {
            self.idr_id = self.idr_id.wrapping_add(1);
        }
        self.slot = slot;
        self.frame += 1;
        self.since_key += 1;
        Ok((out, key))
    }

    /// The reference picture (or the one being made) `since` frames after
    /// the key frame, kept in `slot`.
    fn picture(&self, slot: u8, since: u64, flags: u32) -> va::VAPictureH264 {
        let poc = (since * 2) as i32;
        va::VAPictureH264 { picture_id: self.recon[slot as usize], frame_idx: (since % (1 << LOG2_MAX_FRAME_NUM)) as u32, flags, TopFieldOrderCnt: poc, BottomFieldOrderCnt: poc, ..Default::default() }
    }

    /// Encode the input surface into `slot`, a key frame or predicted from
    /// the one in `from`: the coded bytes.
    fn code(&self, key: bool, slot: u8, from: u8, bufs: &mut Vec<va::VABufferID>) -> Result<Vec<u8>, String> {
        let (d, ctx) = (&self.d, self.context);
        let (mbw, mbh) = self.mbs;
        let (crop_r, crop_b) = (mbw * 16 - self.width, mbh * 16 - self.height);
        if key {
            let mut seq = va::VAEncSequenceParameterBufferH264 {
                seq_parameter_set_id: 0,
                level_idc: self.level,
                intra_period: self.gop as u32,
                intra_idr_period: self.gop as u32,
                ip_period: 1,
                bits_per_second: match self.rate {
                    Rate::Quality(_, cap) if self.rc == va::VA_RC_QVBR => cap,
                    _ => 0,
                },
                max_num_ref_frames: 1,
                picture_width_in_mbs: mbw as u16,
                picture_height_in_mbs: mbh as u16,
                frame_cropping_flag: u8::from(crop_r > 0 || crop_b > 0),
                frame_crop_right_offset: crop_r / 2,
                frame_crop_bottom_offset: crop_b / 2,
                vui_parameters_present_flag: 1,
                aspect_ratio_idc: 1,
                ..Default::default()
            };
            // SAFETY: setting bits of the unions' bitfields.
            unsafe {
                let f = &mut seq.seq_fields.bits;
                f.set_chroma_format_idc(1);
                f.set_frame_mbs_only_flag(1);
                f.set_direct_8x8_inference_flag(1);
                f.set_log2_max_frame_num_minus4(LOG2_MAX_FRAME_NUM - 4);
                f.set_pic_order_cnt_type(2);
                let v = &mut seq.vui_fields.bits;
                v.set_aspect_ratio_info_present_flag(1);
                v.set_bitstream_restriction_flag(1);
                v.set_motion_vectors_over_pic_boundaries_flag(1);
                v.set_log2_max_mv_length_horizontal(15);
                v.set_log2_max_mv_length_vertical(15);
            }
            d.buffer(ctx, va::VAEncSequenceParameterBufferType, &seq, bufs)?;
            if let (va::VA_RC_QVBR, Rate::Quality(q, cap)) = (self.rc, self.rate) {
                // The quality, never more than the cap (over a second),
                // and never a frame skipped to keep to it.
                let mut rc = va::VAEncMiscParameterRateControl {
                    bits_per_second: cap,
                    target_percentage: 100,
                    window_size: 1000,
                    quality_factor: u32::from(q),
                    ICQ_quality_factor: u32::from(q),
                    ..Default::default()
                };
                // SAFETY: setting a bit of the union's bitfield.
                unsafe { rc.rc_flags.bits.set_disable_frame_skip(1) };
                d.misc(ctx, va::VAEncMiscParameterTypeRateControl, &rc, bufs)?;
                let hrd = va::VAEncMiscParameterHRD { buffer_size: cap, initial_buffer_fullness: cap / 4 * 3, ..Default::default() };
                d.misc(ctx, va::VAEncMiscParameterTypeHRD, &hrd, bufs)?;
                let rate = va::VAEncMiscParameterFrameRate { framerate: self.fps, ..Default::default() };
                d.misc(ctx, va::VAEncMiscParameterTypeFrameRate, &rate, bufs)?;
            }
            d.packed(ctx, va::VAEncPackedHeaderSequence, &self.sps, true, bufs)?;
            d.packed(ctx, va::VAEncPackedHeaderPicture, &self.pps, true, bufs)?;
        }
        let invalid = va::VAPictureH264 { picture_id: va::VA_INVALID_SURFACE, flags: va::VA_PICTURE_H264_INVALID, ..Default::default() };
        let previous = self.picture(from, self.since_key.saturating_sub(1), va::VA_PICTURE_H264_SHORT_TERM_REFERENCE);
        let mut pic = va::VAEncPictureParameterBufferH264 {
            CurrPic: self.picture(slot, self.since_key, 0),
            ReferenceFrames: [invalid; 16],
            coded_buf: self.coded,
            pic_parameter_set_id: 0,
            seq_parameter_set_id: 0,
            frame_num: (self.since_key % (1 << LOG2_MAX_FRAME_NUM)) as u16,
            pic_init_qp: self.init_qp,
            ..Default::default()
        };
        if !key {
            pic.ReferenceFrames[0] = previous;
        }
        // SAFETY: setting bits of the union's bitfield.
        unsafe {
            let f = &mut pic.pic_fields.bits;
            f.set_idr_pic_flag(u32::from(key));
            f.set_reference_pic_flag(1);
            f.set_entropy_coding_mode_flag(u32::from(self.profile.cabac));
            f.set_transform_8x8_mode_flag(u32::from(self.profile.transform_8x8));
            f.set_deblocking_filter_control_present_flag(1);
        }
        d.buffer(ctx, va::VAEncPictureParameterBufferType, &pic, bufs)?;
        let mut slice = va::VAEncSliceParameterBufferH264 {
            macroblock_address: 0,
            num_macroblocks: mbw * mbh,
            macroblock_info: va::VA_INVALID_ID,
            // I, else P.
            slice_type: if key { 2 } else { 0 },
            pic_parameter_set_id: 0,
            idr_pic_id: self.idr_id,
            RefPicList0: [invalid; 32],
            RefPicList1: [invalid; 32],
            ..Default::default()
        };
        if !key {
            slice.RefPicList0[0] = previous;
        }
        d.buffer(ctx, va::VAEncSliceParameterBufferType, &slice, bufs)?;
        d.render(ctx, self.input.surface, bufs)?;
        d.coded(self.coded)
    }
}

impl Drop for H264 {
    fn drop(&mut self) {
        self.input.release(&self.d);
        // SAFETY: what we made on our display, each once.
        unsafe { Made { config: self.config, context: self.context, surfaces: [self.input.surface, self.recon[0], self.recon[1]], coded: self.coded }.destroy(&self.d) };
    }
}

/// ue(v): `v` as Exp-Golomb codes it.
fn ue(w: &mut Writer, v: u32) {
    let x = u64::from(v) + 1;
    let n = 64 - x.leading_zeros();
    w.put(0, n - 1);
    w.put(x, n);
}

/// se(v): `v`, signed, as Exp-Golomb codes it.
fn se(w: &mut Writer, v: i32) {
    let v = i64::from(v);
    ue(w, if v > 0 { (2 * v - 1) as u32 } else { (-2 * v) as u32 });
}

/// A NAL unit of `kind` (nal_ref_idc 3) around `rbsp`: its start code,
/// its header, and the payload with emulation prevention bytes put in
/// (no 0, 0 followed by 0–3).
fn nal(kind: u8, rbsp: &[u8]) -> Vec<u8> {
    let mut out = vec![0, 0, 0, 1, 0x60 | kind];
    let mut zeros = 0;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// The sequence parameter set: `profile` at `level`, `mbs` macroblocks
/// across and down, the picture `crop` pixels (right, bottom: even) less;
/// one reference frame, picture order from frame_num (type 2: nothing
/// reordered), BT.709 in limited range, and a VUI that says each frame
/// can be shown as it's decoded.
fn sps(profile: &Profile, level: u8, mbs: (u32, u32), crop: (u32, u32)) -> Vec<u8> {
    let mut w = Writer::default();
    w.put(u64::from(profile.idc), 8);
    w.put(u64::from(profile.constraints), 8);
    w.put(u64::from(level), 8);
    ue(&mut w, 0); // seq_parameter_set_id
    if profile.idc == 100 {
        ue(&mut w, 1); // chroma_format_idc: 4:2:0
        ue(&mut w, 0); // bit_depth_luma_minus8
        ue(&mut w, 0); // bit_depth_chroma_minus8
        w.flag(false); // qpprime_y_zero_transform_bypass_flag
        w.flag(false); // seq_scaling_matrix_present_flag
    }
    ue(&mut w, LOG2_MAX_FRAME_NUM - 4);
    ue(&mut w, 2); // pic_order_cnt_type
    ue(&mut w, 1); // max_num_ref_frames
    w.flag(false); // gaps_in_frame_num_value_allowed_flag
    ue(&mut w, mbs.0 - 1); // pic_width_in_mbs_minus1
    ue(&mut w, mbs.1 - 1); // pic_height_in_map_units_minus1
    w.flag(true); // frame_mbs_only_flag
    w.flag(true); // direct_8x8_inference_flag
    let cropped = crop != (0, 0);
    w.flag(cropped); // frame_cropping_flag
    if cropped {
        // (In twos: 4:2:0's crop units.)
        ue(&mut w, 0);
        ue(&mut w, crop.0 / 2);
        ue(&mut w, 0);
        ue(&mut w, crop.1 / 2);
    }
    w.flag(true); // vui_parameters_present_flag
    // vui_parameters()
    w.flag(true); // aspect_ratio_info_present_flag
    w.put(1, 8); // aspect_ratio_idc: square pixels
    w.flag(false); // overscan_info_present_flag
    w.flag(true); // video_signal_type_present_flag
    w.put(5, 3); // video_format: unspecified
    w.flag(false); // video_full_range_flag: limited
    w.flag(true); // colour_description_present_flag
    w.put(1, 8); // colour_primaries: BT.709
    w.put(1, 8); // transfer_characteristics: BT.709
    w.put(1, 8); // matrix_coefficients: BT.709
    w.flag(false); // chroma_loc_info_present_flag
    w.flag(false); // timing_info_present_flag (a screen's rate varies)
    w.flag(false); // nal_hrd_parameters_present_flag
    w.flag(false); // vcl_hrd_parameters_present_flag
    w.flag(false); // pic_struct_present_flag
    w.flag(true); // bitstream_restriction_flag
    w.flag(true); // motion_vectors_over_pic_boundaries_flag
    ue(&mut w, 0); // max_bytes_per_pic_denom: no limit
    ue(&mut w, 0); // max_bits_per_mb_denom: no limit
    ue(&mut w, 15); // log2_max_mv_length_horizontal
    ue(&mut w, 15); // log2_max_mv_length_vertical
    ue(&mut w, 0); // max_num_reorder_frames: none
    ue(&mut w, 1); // max_dec_frame_buffering: the one reference
    nal(7, &w.trailing())
}

/// The picture parameter set: `profile`'s entropy coding and transform,
/// one reference, slices starting at quantizer `init_qp`.
fn pps(profile: &Profile, init_qp: u8) -> Vec<u8> {
    let mut w = Writer::default();
    ue(&mut w, 0); // pic_parameter_set_id
    ue(&mut w, 0); // seq_parameter_set_id
    w.flag(profile.cabac); // entropy_coding_mode_flag
    w.flag(false); // bottom_field_pic_order_in_frame_present_flag
    ue(&mut w, 0); // num_slice_groups_minus1
    ue(&mut w, 0); // num_ref_idx_l0_default_active_minus1
    ue(&mut w, 0); // num_ref_idx_l1_default_active_minus1
    w.flag(false); // weighted_pred_flag
    w.put(0, 2); // weighted_bipred_idc
    se(&mut w, i32::from(init_qp) - 26); // pic_init_qp_minus26
    se(&mut w, 0); // pic_init_qs_minus26
    se(&mut w, 0); // chroma_qp_index_offset
    w.flag(true); // deblocking_filter_control_present_flag
    w.flag(false); // constrained_intra_pred_flag
    w.flag(false); // redundant_pic_cnt_present_flag
    if profile.transform_8x8 {
        w.flag(true); // transform_8x8_mode_flag
        w.flag(false); // pic_scaling_matrix_present_flag
        se(&mut w, 0); // second_chroma_qp_index_offset
    }
    nal(8, &w.trailing())
}

/// H.264's levels (Table A-1): level_idc, then the most macroblocks a
/// second, in a frame, and kbit/s (Baseline's and Main's: High's are a
/// quarter more).
const LEVELS: [(u8, u64, u64, u64); 19] = [
    (10, 1_485, 99, 64),
    (11, 3_000, 396, 192),
    (12, 6_000, 396, 384),
    (13, 11_880, 396, 768),
    (20, 11_880, 396, 2_000),
    (21, 19_800, 792, 4_000),
    (22, 20_250, 1_620, 4_000),
    (30, 40_500, 1_620, 10_000),
    (31, 108_000, 3_600, 14_000),
    (32, 216_000, 5_120, 20_000),
    (40, 245_760, 8_192, 20_000),
    (41, 245_760, 8_192, 50_000),
    (42, 522_240, 8_704, 50_000),
    (50, 589_824, 22_080, 135_000),
    (51, 983_040, 36_864, 240_000),
    (52, 2_073_600, 36_864, 240_000),
    (60, 4_177_920, 139_264, 240_000),
    (61, 8_355_840, 139_264, 480_000),
    (62, 16_711_680, 139_264, 800_000),
];

/// The lowest level for `mbs` macroblocks at `fps`, at most `cap` bits a
/// second if said (6.2 if none says).
fn level(mbs: (u32, u32), fps: u32, cap: Option<u32>, high: bool) -> u8 {
    let (w, h) = (u64::from(mbs.0), u64::from(mbs.1));
    let fs = w * h;
    let bits = cap.map_or(0, u64::from);
    LEVELS
        .iter()
        .find(|&&(_, mbps, most, kbps)| fs <= most && fs * u64::from(fps) <= mbps && w * w <= most * 8 && h * h <= most * 8 && bits <= kbps * if high { 1250 } else { 1000 })
        .map_or(62, |l| l.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cros_codecs::codec::h264::parser::{Nalu, Parser};

    /// Bits as a reader takes them.
    struct Reader<'a> {
        data: &'a [u8],
        at: usize,
    }

    impl Reader<'_> {
        fn bit(&mut self) -> u32 {
            let b = u32::from((self.data[self.at / 8] >> (7 - self.at % 8)) & 1);
            self.at += 1;
            b
        }
        fn ue(&mut self) -> u32 {
            let mut zeros = 0;
            while self.bit() == 0 {
                zeros += 1;
            }
            let mut v = 1u64;
            for _ in 0..zeros {
                v = v << 1 | u64::from(self.bit());
            }
            (v - 1) as u32
        }
    }

    #[test]
    fn exp_golomb_codes_as_h264_reads() {
        let mut w = Writer::default();
        let values = [0u32, 1, 2, 3, 6, 7, 14, 15, 254, 255, 1000, 65_535];
        for &v in &values {
            ue(&mut w, v);
        }
        for v in [-3i32, -1, 0, 1, 2, 25] {
            se(&mut w, v);
        }
        let out = w.trailing();
        let mut r = Reader { data: &out, at: 0 };
        for &v in &values {
            assert_eq!(r.ue(), v);
        }
        for v in [-3i32, -1, 0, 1, 2, 25] {
            let k = r.ue() as i32;
            assert_eq!(if k % 2 == 1 { (k + 1) / 2 } else { -k / 2 }, v);
        }
        // ue(0) is one bit; ue(1) and ue(2) three.
        let mut w = Writer::default();
        ue(&mut w, 0);
        ue(&mut w, 1);
        ue(&mut w, 2);
        assert_eq!(w.bits, 7);
    }

    #[test]
    fn zeros_are_escaped() {
        assert_eq!(nal(7, &[0, 0, 1, 0, 0, 0, 0, 0, 3, 5]), vec![0, 0, 0, 1, 0x67, 0, 0, 3, 1, 0, 0, 3, 0, 0, 3, 0, 3, 5]);
        // (Two zeros then 4 and up need nothing.)
        assert_eq!(nal(8, &[0, 0, 4]), vec![0, 0, 0, 1, 0x68, 0, 0, 4]);
    }

    /// The parameter sets, read by another parser (cros-codecs'), say
    /// what was meant: the size less the crop, BT.709 in limited range,
    /// nothing reordered.
    #[test]
    fn parameter_sets_read_back() {
        for (profile, (width, height)) in [(PROFILES[0], (1728u32, 1080u32)), (PROFILES[1], (1280, 800)), (PROFILES[2], (3840, 2160)), (PROFILES[0], (642, 362))] {
            let mbs = (width.div_ceil(16), height.div_ceil(16));
            let level = level(mbs, 60, Some(24_000_000), profile.idc == 100);
            let mut both = sps(&profile, level, mbs, (mbs.0 * 16 - width, mbs.1 * 16 - height));
            both.extend(pps(&profile, 26));
            let mut parser = Parser::default();
            let mut cursor = std::io::Cursor::new(&both[..]);
            let nalu = Nalu::next(&mut cursor).unwrap();
            let sps = parser.parse_sps(&nalu).unwrap().clone();
            assert_eq!(sps.profile_idc, profile.idc);
            assert_eq!(sps.pic_order_cnt_type, 2);
            assert_eq!(sps.max_num_ref_frames, 1);
            let r = sps.visible_rectangle();
            assert_eq!((r.max.x - r.min.x, r.max.y - r.min.y), (width, height), "{width}×{height}");
            let vui = &sps.vui_parameters;
            assert!(!vui.video_full_range_flag);
            assert_eq!((vui.colour_primaries, vui.transfer_characteristics, vui.matrix_coefficients), (1, 1, 1));
            assert!(vui.bitstream_restriction_flag);
            assert_eq!((vui.max_num_reorder_frames, vui.max_dec_frame_buffering), (0, 1));
            let nalu = Nalu::next(&mut cursor).unwrap();
            let pps = parser.parse_pps(&nalu).unwrap();
            assert_eq!(pps.entropy_coding_mode_flag, profile.cabac);
            assert_eq!(pps.transform_8x8_mode_flag, profile.transform_8x8);
            assert_eq!(pps.pic_init_qp_minus26, 0);
        }
    }

    #[test]
    fn colour_is_read_from_the_sequence() {
        let ours = sps(&PROFILES[0], 40, (80, 45), (0, 0));
        assert_eq!(colour(&ours), Some((1, false)));
        // An SPS that says nothing (OpenH264's): not said, limited.
        let mut w = Writer::default();
        w.put(66, 8);
        w.put(0xc0, 8);
        w.put(30, 8);
        for v in [0, 0, 2, 1] {
            ue(&mut w, v);
        }
        w.flag(false);
        ue(&mut w, 39);
        ue(&mut w, 29);
        w.flag(true);
        w.flag(true);
        w.flag(false);
        w.flag(false);
        assert_eq!(colour(&nal(7, &w.trailing())), Some((2, false)));
        assert_eq!(colour(&[0, 0, 0, 1, 0x68, 0xce]), None);
    }

    #[test]
    fn levels_by_size_and_rate() {
        // 1080p: 4.0 at 30, 4.2 at 60; 4K: 5.1 at 30, 5.2 at 60.
        assert_eq!(level((120, 68), 30, None, true), 40);
        assert_eq!(level((120, 68), 60, None, true), 42);
        assert_eq!(level((240, 135), 30, None, true), 51);
        assert_eq!(level((240, 135), 60, None, true), 52);
        // More bits than 4.0 has: 4.1.
        assert_eq!(level((120, 68), 30, Some(30_000_000), true), 41);
        // A 1728 × 1080 receiver at 60, capped at 22.4 Mbit/s.
        assert_eq!(level((108, 68), 60, Some(22_400_000), true), 42);
    }

    /// The NAL units' kinds in Annex B `bytes`, in order.
    fn kinds(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        while i + 3 < bytes.len() {
            if bytes[i] == 0 && bytes[i + 1] == 0 && bytes[i + 2] == 1 {
                out.push(bytes[i + 3] & 0x1f);
                i += 3;
            }
            i += 1;
        }
        out
    }

    /// On this computer's GPU (where VA-API encodes H.264): a key frame
    /// is our parameter sets and an IDR slice, the rest a P slice each,
    /// at a quality with a cap and at a constant quantizer, from memory
    /// and at an odd size of macroblocks.
    #[test]
    fn frames_come_out_as_said() {
        let Some(node) = node() else {
            eprintln!("no GPU encodes H.264 through VA-API here");
            return;
        };
        for (rate, (w, h)) in [(Rate::Quality(QUALITY, 20_000_000), (640, 360)), (Rate::Constant(24), (642, 362))] {
            let mut e = H264::new(Some(&node), w, h, 60, rate).unwrap();
            eprintln!("{}: {}", e.driver(), e.describe());
            let (sps, pps) = (e.parameter_sets().0.to_vec(), e.parameter_sets().1.to_vec());
            for i in 0..5u32 {
                let px: Vec<u8> = (0..h).flat_map(|y| (0..w).flat_map(move |x| [(x + i * 4) as u8, y as u8, 128, 255])).collect();
                let (bits, key) = e.encode(&px, w as usize * 4, false).unwrap();
                assert_eq!(key, i == 0);
                if key {
                    assert_eq!(kinds(&bits), [7, 8, 5], "{rate:?}");
                    assert!(bits.starts_with(&sps), "our SPS first");
                    assert_eq!(&bits[sps.len()..sps.len() + pps.len()], &pps[..], "then our PPS");
                } else {
                    assert_eq!(kinds(&bits), [1], "{rate:?}");
                }
            }
            // Asked for: a key frame next.
            e.key_next();
            let px = vec![90u8; (w * h * 4) as usize];
            assert!(e.encode(&px, w as usize * 4, false).unwrap().1);
        }
    }
}
