//! H.264's pictures and slices for VA-API, as ChromeOS's decoder fills
//! them (cros-codecs' `h264/vaapi.rs`, on our own declarations): the
//! picture's parameters with every reference the decoded picture buffer
//! holds, the scaling lists, and each slice's header, reference lists
//! and weights.

use std::rc::Rc;

use cros_codecs::{
    Resolution,
    codec::h264::{
        dpb::{Dpb, DpbEntry},
        parser::{Level, Pps, Profile, Slice, SliceHeader, Sps},
        picture::{Field, PictureData, Reference},
    },
    decoder::{
        DecodedHandle,
        stateless::{
            NewPictureResult, StatelessBackendResult, StatelessDecoderBackend, StatelessDecoderBackendPicture,
            h264::{H264, StatelessH264DecoderBackend, get_raster_from_zigzag_4x4, get_raster_from_zigzag_8x8},
        },
    },
};
use va_sys as va;

use super::{
    bits,
    stateless::{Backend, Handle, Look, Picture, other},
};

impl StatelessDecoderBackendPicture<H264> for Backend {
    type Picture = Picture;
}

/// The VA-API profile for `sps`, if the GPU's is one that decodes it
/// (8-bit 4:2:0: High 10 and the 4:2:2 and 4:4:4 profiles aren't).
fn profile(sps: &Sps) -> Result<va::VAProfile, String> {
    if sps.bit_depth_luma_minus8 != 0 || sps.bit_depth_chroma_minus8 != 0 || sps.chroma_format_idc > 1 {
        return Err(format!("H.264 at {} bits, chroma format {}, isn't decoded on the GPU", sps.bit_depth_luma_minus8 + 8, sps.chroma_format_idc));
    }
    match Profile::try_from(sps.profile_idc) {
        Ok(Profile::Baseline) if sps.constraint_set0_flag => Ok(va::VAProfileH264ConstrainedBaseline),
        Ok(Profile::Main) => Ok(va::VAProfileH264Main),
        Ok(Profile::Extended) if sps.constraint_set1_flag => Ok(va::VAProfileH264Main),
        Ok(Profile::High | Profile::High422P | Profile::High10) => Ok(va::VAProfileH264High),
        _ => Err(format!("H.264 profile {} isn't decoded on the GPU", sps.profile_idc)),
    }
}

/// A picture as VA-API names one: its surface, frame number (or long
/// term index), whether a reference, which field, and its order counts.
fn va_picture(pic: &PictureData, surface: va::VASurfaceID, merge_other_field: bool) -> va::VAPictureH264 {
    let mut flags = 0;
    let frame_idx = if matches!(pic.reference(), Reference::LongTerm) {
        flags |= va::VA_PICTURE_H264_LONG_TERM_REFERENCE;
        pic.long_term_frame_idx
    } else {
        if matches!(pic.reference(), Reference::ShortTerm) {
            flags |= va::VA_PICTURE_H264_SHORT_TERM_REFERENCE;
        }
        pic.frame_num
    };
    let (top, bottom) = match pic.field {
        Field::Frame => (pic.top_field_order_cnt, pic.bottom_field_order_cnt),
        Field::Top => match (merge_other_field, pic.other_field()) {
            (true, Some(other)) => (pic.top_field_order_cnt, other.borrow().bottom_field_order_cnt),
            _ => {
                flags |= va::VA_PICTURE_H264_TOP_FIELD;
                (pic.top_field_order_cnt, 0)
            }
        },
        Field::Bottom => match (merge_other_field, pic.other_field()) {
            (true, Some(other)) => (other.borrow().top_field_order_cnt, pic.bottom_field_order_cnt),
            _ => {
                flags |= va::VA_PICTURE_H264_BOTTOM_FIELD;
                (0, pic.bottom_field_order_cnt)
            }
        },
    };
    va::VAPictureH264 { picture_id: surface, frame_idx, flags, TopFieldOrderCnt: top, BottomFieldOrderCnt: bottom, ..Default::default() }
}

fn no_picture() -> va::VAPictureH264 {
    va::VAPictureH264 { picture_id: va::VA_INVALID_SURFACE, flags: va::VA_PICTURE_H264_INVALID, ..Default::default() }
}

fn surface_of(reference: &Option<Handle>) -> va::VASurfaceID {
    reference.as_ref().map_or(va::VA_INVALID_SURFACE, Handle::id)
}

fn picture_parameters(hdr: &SliceHeader, current: &PictureData, surface: va::VASurfaceID, dpb: &Dpb<Handle>, sps: &Sps, pps: &Pps) -> va::VAPictureParameterBufferH264 {
    let mut refs: Vec<va::VAPictureH264> = Vec::with_capacity(16);
    for e in dpb.short_term_refs_iter() {
        let pic = e.pic.borrow();
        if !pic.nonexisting && !pic.is_second_field() {
            refs.push(va_picture(&pic, surface_of(&e.reference), true));
        }
    }
    for e in dpb.long_term_refs_iter() {
        let pic = e.pic.borrow();
        if !pic.is_second_field() {
            refs.push(va_picture(&pic, surface_of(&e.reference), true));
        }
    }
    refs.truncate(16);
    let mut reference_frames = [no_picture(); 16];
    reference_frames[..refs.len()].copy_from_slice(&refs);
    let b = |v: bool| v as u32;
    let interlaced = u32::from(!sps.frame_mbs_only_flag);
    let mut p = va::VAPictureParameterBufferH264 {
        CurrPic: va_picture(current, surface, false),
        ReferenceFrames: reference_frames,
        picture_width_in_mbs_minus1: sps.pic_width_in_mbs_minus1,
        picture_height_in_mbs_minus1: (((sps.pic_height_in_map_units_minus1 + 1) << interlaced) - 1),
        bit_depth_luma_minus8: sps.bit_depth_luma_minus8,
        bit_depth_chroma_minus8: sps.bit_depth_chroma_minus8,
        num_ref_frames: sps.max_num_ref_frames,
        pic_init_qp_minus26: pps.pic_init_qp_minus26,
        pic_init_qs_minus26: pps.pic_init_qs_minus26,
        chroma_qp_index_offset: pps.chroma_qp_index_offset,
        second_chroma_qp_index_offset: pps.second_chroma_qp_index_offset,
        frame_num: hdr.frame_num,
        ..Default::default()
    };
    p.seq_fields.value = bits(u32::from(sps.chroma_format_idc), 0, 2)
        | bits(b(sps.separate_colour_plane_flag), 2, 1)
        | bits(b(sps.gaps_in_frame_num_value_allowed_flag), 3, 1)
        | bits(b(sps.frame_mbs_only_flag), 4, 1)
        | bits(b(sps.mb_adaptive_frame_field_flag), 5, 1)
        | bits(b(sps.direct_8x8_inference_flag), 6, 1)
        // (A.3.3.2: from level 3.1 on.)
        | bits(b(sps.level_idc >= Level::L3_1), 7, 1)
        | bits(u32::from(sps.log2_max_frame_num_minus4), 8, 4)
        | bits(u32::from(sps.pic_order_cnt_type), 12, 2)
        | bits(u32::from(sps.log2_max_pic_order_cnt_lsb_minus4), 14, 4)
        | bits(b(sps.delta_pic_order_always_zero_flag), 18, 1);
    p.pic_fields.value = bits(b(pps.entropy_coding_mode_flag), 0, 1)
        | bits(b(pps.weighted_pred_flag), 1, 1)
        | bits(u32::from(pps.weighted_bipred_idc), 2, 2)
        | bits(b(pps.transform_8x8_mode_flag), 4, 1)
        | bits(b(hdr.field_pic_flag), 5, 1)
        | bits(b(pps.constrained_intra_pred_flag), 6, 1)
        | bits(b(pps.bottom_field_pic_order_in_frame_present_flag), 7, 1)
        | bits(b(pps.deblocking_filter_control_present_flag), 8, 1)
        | bits(b(pps.redundant_pic_cnt_present_flag), 9, 1)
        | bits(b(current.nal_ref_idc != 0), 10, 1);
    p
}

fn scaling_lists(pps: &Pps) -> va::VAIQMatrixBufferH264 {
    let mut m = va::VAIQMatrixBufferH264::default();
    for i in 0..6 {
        get_raster_from_zigzag_4x4(pps.scaling_lists_4x4[i], &mut m.ScalingList4x4[i]);
    }
    for i in 0..2 {
        get_raster_from_zigzag_8x8(pps.scaling_lists_8x8[i], &mut m.ScalingList8x8[i]);
    }
    m
}

fn reference_list(list: &[&DpbEntry<Handle>]) -> [va::VAPictureH264; 32] {
    let mut out = [no_picture(); 32];
    for (to, e) in out.iter_mut().zip(list) {
        let pic = e.pic.borrow();
        let merge = matches!(pic.field, Field::Frame);
        *to = va_picture(&pic, surface_of(&e.reference), merge);
    }
    out
}

fn slice_parameters(hdr: &SliceHeader, size: usize, l0: &[&DpbEntry<Handle>], l1: &[&DpbEntry<Handle>], sps: &Sps, pps: &Pps) -> va::VASliceParameterBufferH264 {
    let pwt = &hdr.pred_weight_table;
    let mut s = va::VASliceParameterBufferH264 {
        slice_data_size: size as u32,
        slice_data_offset: 0,
        slice_data_flag: va::VA_SLICE_DATA_FLAG_ALL,
        slice_data_bit_offset: hdr.header_bit_size as u16,
        first_mb_in_slice: hdr.first_mb_in_slice as u16,
        slice_type: hdr.slice_type as u8,
        direct_spatial_mv_pred_flag: hdr.direct_spatial_mv_pred_flag as u8,
        num_ref_idx_l0_active_minus1: hdr.num_ref_idx_l0_active_minus1,
        num_ref_idx_l1_active_minus1: hdr.num_ref_idx_l1_active_minus1,
        cabac_init_idc: hdr.cabac_init_idc,
        slice_qp_delta: hdr.slice_qp_delta,
        disable_deblocking_filter_idc: hdr.disable_deblocking_filter_idc,
        slice_alpha_c0_offset_div2: hdr.slice_alpha_c0_offset_div2,
        slice_beta_offset_div2: hdr.slice_beta_offset_div2,
        RefPicList0: reference_list(l0),
        RefPicList1: reference_list(l1),
        luma_log2_weight_denom: pwt.luma_log2_weight_denom,
        chroma_log2_weight_denom: pwt.chroma_log2_weight_denom,
        ..Default::default()
    };
    let (fill_l0, fill_l1) = if pps.weighted_pred_flag && (hdr.slice_type.is_p() || hdr.slice_type.is_sp()) {
        (true, false)
    } else if pps.weighted_bipred_idc == 1 && hdr.slice_type.is_b() {
        (true, true)
    } else {
        (false, false)
    };
    let chroma = sps.chroma_array_type() != 0;
    if fill_l0 {
        s.luma_weight_l0_flag = 1;
        for i in 0..=hdr.num_ref_idx_l0_active_minus1 as usize {
            s.luma_weight_l0[i] = pwt.luma_weight_l0[i];
            s.luma_offset_l0[i] = i16::from(pwt.luma_offset_l0[i]);
        }
        if chroma {
            s.chroma_weight_l0_flag = 1;
            for i in 0..=hdr.num_ref_idx_l0_active_minus1 as usize {
                for j in 0..2 {
                    s.chroma_weight_l0[i][j] = pwt.chroma_weight_l0[i][j];
                    s.chroma_offset_l0[i][j] = i16::from(pwt.chroma_offset_l0[i][j]);
                }
            }
        }
    }
    if fill_l1 {
        s.luma_weight_l1_flag = 1;
        let n = hdr.num_ref_idx_l1_active_minus1 as usize + 1;
        s.luma_weight_l1[..n].copy_from_slice(&pwt.luma_weight_l1[..n]);
        for i in 0..n {
            s.luma_offset_l1[i] = pwt.luma_offset_l1[i];
        }
        if chroma {
            s.chroma_weight_l1_flag = 1;
            for i in 0..n {
                for j in 0..2 {
                    s.chroma_weight_l1[i][j] = pwt.chroma_weight_l1[i][j];
                    s.chroma_offset_l1[i][j] = i16::from(pwt.chroma_offset_l1[i][j]);
                }
            }
        }
    }
    s
}

impl StatelessH264DecoderBackend for Backend {
    fn new_sequence(&mut self, sps: &Rc<Sps>) -> StatelessBackendResult<()> {
        let profile = profile(sps).map_err(other)?;
        let rect = sps.visible_rectangle();
        let vui = &sps.vui_parameters;
        let described = sps.vui_parameters_present_flag && vui.video_signal_type_present_flag;
        let look = Look {
            width: rect.max.x - rect.min.x,
            height: rect.max.y - rect.min.y,
            matrix: if described && vui.colour_description_present_flag { vui.matrix_coefficients } else { 2 },
            full_range: described && vui.video_full_range_flag,
        };
        self.sequence(profile, false, Resolution { width: sps.width(), height: sps.height() }, look, sps.max_dpb_frames() + 4)
    }

    fn new_picture(&mut self, timestamp: u64, alloc_cb: &mut dyn FnMut() -> Option<<<Self as StatelessDecoderBackend>::Handle as DecodedHandle>::Frame>) -> NewPictureResult<Picture> {
        // (Its token says nothing we don't know: the surface is ours.)
        let _ = alloc_cb();
        self.picture(timestamp)
    }

    fn new_field_picture(&mut self, timestamp: u64, first_field: &Handle) -> NewPictureResult<Picture> {
        // The second field into the first's surface.
        Ok(Picture { surface: first_field.surface().clone(), timestamp, buffers: Vec::new() })
    }

    fn start_picture(&mut self, picture: &mut Picture, picture_data: &PictureData, sps: &Sps, pps: &Pps, dpb: &Dpb<Handle>, hdr: &SliceHeader) -> StatelessBackendResult<()> {
        let mut p = picture_parameters(hdr, picture_data, picture.surface.id, dpb, sps, pps);
        self.add(picture, va::VAPictureParameterBufferType, &mut p)?;
        let mut m = scaling_lists(pps);
        self.add(picture, va::VAIQMatrixBufferType, &mut m)
    }

    fn decode_slice(&mut self, picture: &mut Picture, slice: &Slice, sps: &Sps, pps: &Pps, l0: &[&DpbEntry<Handle>], l1: &[&DpbEntry<Handle>]) -> StatelessBackendResult<()> {
        let mut s = slice_parameters(&slice.header, slice.nalu.size, l0, l1, sps, pps);
        self.add(picture, va::VASliceParameterBufferType, &mut s)?;
        self.add_bytes(picture, slice.nalu.as_ref())
    }

    fn submit_picture(&mut self, picture: Picture) -> StatelessBackendResult<Handle> {
        self.submit(picture)
    }
}
