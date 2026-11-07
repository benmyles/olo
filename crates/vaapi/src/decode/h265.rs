//! HEVC's pictures and slices for VA-API, as ChromeOS's decoder fills
//! them (cros-codecs' `h265/vaapi.rs`, on our own declarations): Main and
//! Main 10. The picture's references are the decoded picture buffer's,
//! each marked with the set it is in this picture (before, after, long
//! term); a slice's reference lists are indices into them; the last
//! slice of a picture says so (they're sent a slice behind for that).

use cros_codecs::{
    Resolution,
    codec::h265::{
        dpb::Dpb,
        parser::{NaluType, Pps, Profile, Slice, Sps},
        picture::{PictureData, Reference},
    },
    decoder::{
        DecodedHandle,
        stateless::{
            NewPictureResult, StatelessBackendResult, StatelessDecoderBackend, StatelessDecoderBackendPicture,
            h265::{H265, RefPicListEntry, RefPicSet, StatelessH265DecoderBackend},
        },
    },
};
use va_sys as va;

use super::{
    bits,
    stateless::{Backend, Handle, Look, Picture, other},
};

/// A picture being decoded, with what HEVC needs between its slices.
pub(super) struct HevcPicture {
    picture: Picture,
    /// The references as VA-API was told them (the slices' lists index
    /// into them).
    references: [va::VAPictureHEVC; 15],
    /// The slice before, sent with the next or as the last.
    last: Option<(va::VASliceParameterBufferHEVC, Vec<u8>)>,
}

impl StatelessDecoderBackendPicture<H265> for Backend {
    type Picture = HevcPicture;
}

/// The VA-API profile for `sps`: Main (8-bit 4:2:0) or Main 10.
fn profile(sps: &Sps) -> Result<(va::VAProfile, bool), String> {
    let depth = (sps.bit_depth_luma_minus8 + 8).max(sps.bit_depth_chroma_minus8 + 8);
    let profile = Profile::try_from(sps.profile_tier_level.general_profile_idc);
    match (profile, depth, sps.chroma_format_idc) {
        (Ok(Profile::Main | Profile::MainStill | Profile::Main10), 8, 0 | 1) => Ok((va::VAProfileHEVCMain, false)),
        (Ok(Profile::Main | Profile::MainStill | Profile::Main10), 10, 0 | 1) => Ok((va::VAProfileHEVCMain10, true)),
        _ => Err(format!("HEVC profile {} at {depth} bits, chroma format {}, isn't decoded on the GPU", sps.profile_tier_level.general_profile_idc, sps.chroma_format_idc)),
    }
}

/// Equation 5-8.
fn clip3(x: i32, y: i32, z: i32) -> i32 {
    z.clamp(x, y)
}

/// 6.5.3's up-right diagonal scan of an N×N block, as raster positions.
const fn up_right_diagonal<const N: usize, const ROWS: usize>() -> [usize; N] {
    let (mut i, mut x, mut y) = (0, 0i32, 0i32);
    let mut out = [0; N];
    loop {
        while y >= 0 {
            if x < ROWS as i32 && y < ROWS as i32 {
                out[i] = (x + ROWS as i32 * y) as usize;
                i += 1;
            }
            y -= 1;
            x += 1;
        }
        y = x;
        x = 0;
        if i >= N {
            break;
        }
    }
    out
}

const DIAGONAL_4X4: [usize; 16] = up_right_diagonal::<16, 4>();
const DIAGONAL_8X8: [usize; 64] = up_right_diagonal::<64, 8>();

fn raster<const N: usize>(scan: &[usize; N], src: &[u8; N], dst: &mut [u8; N]) {
    for i in 0..N {
        dst[scan[i]] = src[i];
    }
}

/// The set `pic` is in for the picture being decoded.
fn rps_flag(pic: &PictureData, rps: &RefPicSet<Handle>) -> u32 {
    let has = |set: &[Option<cros_codecs::codec::h265::dpb::DpbEntry<Handle>>]| set.iter().flatten().any(|e| *e.0.borrow() == *pic);
    if has(&rps.ref_pic_set_st_curr_before) {
        va::VA_PICTURE_HEVC_RPS_ST_CURR_BEFORE
    } else if has(&rps.ref_pic_set_st_curr_after) {
        va::VA_PICTURE_HEVC_RPS_ST_CURR_AFTER
    } else if has(&rps.ref_pic_set_lt_curr) {
        va::VA_PICTURE_HEVC_RPS_LT_CURR
    } else {
        0
    }
}

fn va_picture(pic: &PictureData, surface: va::VASurfaceID, rps: &RefPicSet<Handle>) -> va::VAPictureHEVC {
    let mut flags = rps_flag(pic, rps);
    if matches!(pic.reference(), Reference::LongTerm) {
        flags |= va::VA_PICTURE_HEVC_LONG_TERM_REFERENCE;
    }
    va::VAPictureHEVC { picture_id: surface, pic_order_cnt: pic.pic_order_cnt_val, flags, ..Default::default() }
}

fn no_picture() -> va::VAPictureHEVC {
    va::VAPictureHEVC { picture_id: va::VA_INVALID_SURFACE, pic_order_cnt: 0, flags: va::VA_PICTURE_HEVC_INVALID, ..Default::default() }
}

fn picture_parameters(
    current: &PictureData,
    surface: va::VASurfaceID,
    dpb: &Dpb<Handle>,
    rps: &RefPicSet<Handle>,
    sps: &Sps,
    pps: &Pps,
) -> (va::VAPictureParameterBufferHEVC, [va::VAPictureHEVC; 15]) {
    let mut references = [no_picture(); 15];
    for (to, e) in references.iter_mut().zip(dpb.get_all_references()) {
        *to = va_picture(&e.0.borrow(), e.1.id(), rps);
    }
    let b = |v: bool| v as u32;
    let mut p = va::VAPictureParameterBufferHEVC {
        CurrPic: va_picture(current, surface, rps),
        ReferenceFrames: references,
        pic_width_in_luma_samples: sps.pic_width_in_luma_samples,
        pic_height_in_luma_samples: sps.pic_height_in_luma_samples,
        sps_max_dec_pic_buffering_minus1: sps.max_dec_pic_buffering_minus1[usize::from(sps.max_sub_layers_minus1)],
        bit_depth_luma_minus8: sps.bit_depth_luma_minus8,
        bit_depth_chroma_minus8: sps.bit_depth_chroma_minus8,
        pcm_sample_bit_depth_luma_minus1: sps.pcm_sample_bit_depth_luma_minus1,
        pcm_sample_bit_depth_chroma_minus1: sps.pcm_sample_bit_depth_chroma_minus1,
        log2_min_luma_coding_block_size_minus3: sps.log2_min_luma_coding_block_size_minus3,
        log2_diff_max_min_luma_coding_block_size: sps.log2_diff_max_min_luma_coding_block_size,
        log2_min_transform_block_size_minus2: sps.log2_min_luma_transform_block_size_minus2,
        log2_diff_max_min_transform_block_size: sps.log2_diff_max_min_luma_transform_block_size,
        log2_min_pcm_luma_coding_block_size_minus3: sps.log2_min_pcm_luma_coding_block_size_minus3,
        log2_diff_max_min_pcm_luma_coding_block_size: sps.log2_diff_max_min_pcm_luma_coding_block_size,
        max_transform_hierarchy_depth_intra: sps.max_transform_hierarchy_depth_intra,
        max_transform_hierarchy_depth_inter: sps.max_transform_hierarchy_depth_inter,
        init_qp_minus26: pps.init_qp_minus26,
        diff_cu_qp_delta_depth: pps.diff_cu_qp_delta_depth,
        pps_cb_qp_offset: pps.cb_qp_offset,
        pps_cr_qp_offset: pps.cr_qp_offset,
        log2_parallel_merge_level_minus2: pps.log2_parallel_merge_level_minus2,
        num_tile_columns_minus1: pps.num_tile_columns_minus1,
        num_tile_rows_minus1: pps.num_tile_rows_minus1,
        log2_max_pic_order_cnt_lsb_minus4: sps.log2_max_pic_order_cnt_lsb_minus4,
        num_short_term_ref_pic_sets: sps.num_short_term_ref_pic_sets,
        num_long_term_ref_pic_sps: sps.num_long_term_ref_pics_sps,
        num_ref_idx_l0_default_active_minus1: pps.num_ref_idx_l0_default_active_minus1,
        num_ref_idx_l1_default_active_minus1: pps.num_ref_idx_l1_default_active_minus1,
        pps_beta_offset_div2: pps.beta_offset_div2,
        pps_tc_offset_div2: pps.tc_offset_div2,
        num_extra_slice_header_bits: pps.num_extra_slice_header_bits,
        st_rps_bits: current.short_term_ref_pic_set_size_bits,
        ..Default::default()
    };
    for (to, from) in p.column_width_minus1.iter_mut().zip(pps.column_width_minus1.iter()) {
        *to = *from as u16;
    }
    for (to, from) in p.row_height_minus1.iter_mut().zip(pps.row_height_minus1.iter()) {
        *to = *from as u16;
    }
    // (NoPicReorderingFlag and NoBiPredFlag left unset, as FFmpeg and
    // GStreamer leave them.)
    p.pic_fields.value = bits(u32::from(sps.chroma_format_idc), 0, 2)
        | bits(b(sps.separate_colour_plane_flag), 2, 1)
        | bits(b(sps.pcm_enabled_flag), 3, 1)
        | bits(b(sps.scaling_list_enabled_flag), 4, 1)
        | bits(b(pps.transform_skip_enabled_flag), 5, 1)
        | bits(b(sps.amp_enabled_flag), 6, 1)
        | bits(b(sps.strong_intra_smoothing_enabled_flag), 7, 1)
        | bits(b(pps.sign_data_hiding_enabled_flag), 8, 1)
        | bits(b(pps.constrained_intra_pred_flag), 9, 1)
        | bits(b(pps.cu_qp_delta_enabled_flag), 10, 1)
        | bits(b(pps.weighted_pred_flag), 11, 1)
        | bits(b(pps.weighted_bipred_flag), 12, 1)
        | bits(b(pps.transquant_bypass_enabled_flag), 13, 1)
        | bits(b(pps.tiles_enabled_flag), 14, 1)
        | bits(b(pps.entropy_coding_sync_enabled_flag), 15, 1)
        | bits(b(pps.loop_filter_across_slices_enabled_flag), 16, 1)
        | bits(b(pps.loop_filter_across_tiles_enabled_flag), 17, 1)
        | bits(b(sps.pcm_loop_filter_disabled_flag), 18, 1);
    let nal = current.nalu_type as u32;
    let rap = nal >= NaluType::BlaWLp as u32 && nal <= NaluType::CraNut as u32;
    p.slice_parsing_fields.value = bits(b(pps.lists_modification_present_flag), 0, 1)
        | bits(b(sps.long_term_ref_pics_present_flag), 1, 1)
        | bits(b(sps.temporal_mvp_enabled_flag), 2, 1)
        | bits(b(pps.cabac_init_present_flag), 3, 1)
        | bits(b(pps.output_flag_present_flag), 4, 1)
        | bits(b(pps.dependent_slice_segments_enabled_flag), 5, 1)
        | bits(b(pps.slice_chroma_qp_offsets_present_flag), 6, 1)
        | bits(b(sps.sample_adaptive_offset_enabled_flag), 7, 1)
        | bits(b(pps.deblocking_filter_override_enabled_flag), 8, 1)
        | bits(b(pps.deblocking_filter_disabled_flag), 9, 1)
        | bits(b(pps.slice_segment_header_extension_present_flag), 10, 1)
        | bits(b(rap), 11, 1)
        | bits(b(current.nalu_type.is_idr()), 12, 1)
        | bits(b(current.nalu_type.is_irap()), 13, 1);
    (p, references)
}

/// Where the scaling lists are (the PPS's, the SPS's, or none: flat).
fn scaling_lists(sps: &Sps, pps: &Pps) -> Option<va::VAIQMatrixBufferHEVC> {
    let lists = if pps.scaling_list_data_present_flag || (sps.scaling_list_enabled_flag && !sps.scaling_list_data_present_flag) {
        &pps.scaling_list
    } else if sps.scaling_list_enabled_flag && sps.scaling_list_data_present_flag {
        &sps.scaling_list
    } else {
        return None;
    };
    let mut m = va::VAIQMatrixBufferHEVC::default();
    for i in 0..6 {
        raster(&DIAGONAL_4X4, &lists.scaling_list_4x4[i], &mut m.ScalingList4x4[i]);
        raster(&DIAGONAL_8X8, &lists.scaling_list_8x8[i], &mut m.ScalingList8x8[i]);
        raster(&DIAGONAL_8X8, &lists.scaling_list_16x16[i], &mut m.ScalingList16x16[i]);
        m.ScalingListDC16x16[i] = (lists.scaling_list_dc_coef_minus8_16x16[i] + 8) as u8;
    }
    // (32×32: the first of each three, luma's and chroma's.)
    for i in 0..2 {
        raster(&DIAGONAL_8X8, &lists.scaling_list_32x32[i * 3], &mut m.ScalingList32x32[i]);
        m.ScalingListDC32x32[i] = (lists.scaling_list_dc_coef_minus8_32x32[i * 3] + 8) as u8;
    }
    Some(m)
}

/// A slice's reference list as indices into the picture's references.
fn list_indices(list: &[Option<RefPicListEntry<Handle>>; 16], references: &[va::VAPictureHEVC; 15]) -> [u8; 15] {
    let mut out = [0xff; 15];
    for (k, entry) in list.iter().take(15).enumerate() {
        let Some(entry) = entry else { continue };
        let poc = match entry {
            RefPicListEntry::CurrentPicture(p) => p.pic_order_cnt_val,
            RefPicListEntry::DpbEntry(e) => e.0.borrow().pic_order_cnt_val,
        };
        for (i, r) in references.iter().enumerate() {
            if r.picture_id == va::VA_INVALID_SURFACE {
                break;
            }
            if r.pic_order_cnt == poc {
                out[k] = i as u8;
            }
        }
    }
    out
}

impl Backend {
    /// The slice held back, sent (`last`: as the picture's last).
    fn send_slice(&self, picture: &mut HevcPicture, last: bool) -> StatelessBackendResult<()> {
        let Some((mut s, data)) = picture.last.take() else { return Ok(()) };
        if last {
            // SAFETY: the union is one u32 either way (its bitfields, or
            // `value`, as it was written).
            let flags = unsafe { s.LongSliceFlags.value };
            // (LastSliceOfPic.)
            s.LongSliceFlags.value = flags | 1;
        }
        self.add(&mut picture.picture, va::VASliceParameterBufferType, &mut s)?;
        self.add_bytes(&mut picture.picture, &data)
    }
}

impl StatelessH265DecoderBackend for Backend {
    fn new_sequence(&mut self, sps: &Sps) -> StatelessBackendResult<()> {
        let (profile, deep) = profile(sps).map_err(other)?;
        let rect = sps.visible_rectangle();
        let vui = &sps.vui_parameters;
        let described = sps.vui_parameters_present_flag && vui.video_signal_type_present_flag;
        let look = Look {
            width: rect.max.x - rect.min.x,
            height: rect.max.y - rect.min.y,
            matrix: if described && vui.colour_description_present_flag { vui.matrix_coeffs as u8 } else { 2 },
            full_range: described && vui.video_full_range_flag,
        };
        let coded = Resolution { width: u32::from(sps.width()), height: u32::from(sps.height()) };
        self.sequence(profile, deep, coded, look, sps.max_dpb_size() + 4)
    }

    fn new_picture(&mut self, timestamp: u64, alloc_cb: &mut dyn FnMut() -> Option<<<Self as StatelessDecoderBackend>::Handle as DecodedHandle>::Frame>) -> NewPictureResult<HevcPicture> {
        let _ = alloc_cb();
        Ok(HevcPicture { picture: self.picture(timestamp)?, references: [no_picture(); 15], last: None })
    }

    fn begin_picture(&mut self, picture: &mut HevcPicture, picture_data: &PictureData, sps: &Sps, pps: &Pps, dpb: &Dpb<Handle>, rps: &RefPicSet<Handle>, _: &Slice) -> StatelessBackendResult<()> {
        if pps.scc_extension.curr_pic_ref_enabled_flag {
            return Err(other("HEVC's screen content coding isn't decoded on the GPU".into()));
        }
        let (mut p, references) = picture_parameters(picture_data, picture.picture.surface.id, dpb, rps, sps, pps);
        picture.references = references;
        self.add(&mut picture.picture, va::VAPictureParameterBufferType, &mut p)?;
        if let Some(mut m) = scaling_lists(sps, pps) {
            self.add(&mut picture.picture, va::VAIQMatrixBufferType, &mut m)?;
        }
        Ok(())
    }

    fn decode_slice(
        &mut self,
        picture: &mut HevcPicture,
        slice: &Slice,
        sps: &Sps,
        _: &Pps,
        l0: &[Option<RefPicListEntry<Handle>>; 16],
        l1: &[Option<RefPicListEntry<Handle>>; 16],
    ) -> StatelessBackendResult<()> {
        self.send_slice(picture, false)?;
        let hdr = &slice.header;
        let pwt = &hdr.pred_weight_table;
        let b = |v: bool| v as u32;
        let mut s = va::VASliceParameterBufferHEVC {
            slice_data_size: slice.nalu.size as u32,
            slice_data_offset: 0,
            slice_data_flag: va::VA_SLICE_DATA_FLAG_ALL,
            slice_data_byte_offset: (hdr.header_bit_size / 8),
            slice_segment_address: hdr.segment_address,
            RefPicList: [list_indices(l0, &picture.references), list_indices(l1, &picture.references)],
            collocated_ref_idx: if hdr.temporal_mvp_enabled_flag { hdr.collocated_ref_idx } else { 0xff },
            num_ref_idx_l0_active_minus1: hdr.num_ref_idx_l0_active_minus1,
            num_ref_idx_l1_active_minus1: hdr.num_ref_idx_l1_active_minus1,
            slice_qp_delta: hdr.qp_delta,
            slice_cb_qp_offset: hdr.cb_qp_offset,
            slice_cr_qp_offset: hdr.cr_qp_offset,
            slice_beta_offset_div2: hdr.beta_offset_div2,
            slice_tc_offset_div2: hdr.tc_offset_div2,
            luma_log2_weight_denom: pwt.luma_log2_weight_denom,
            delta_chroma_log2_weight_denom: pwt.delta_chroma_log2_weight_denom,
            five_minus_max_num_merge_cand: hdr.five_minus_max_num_merge_cand,
            num_entry_point_offsets: hdr.num_entry_point_offsets as u16,
            entry_offset_to_subset_array: 0,
            slice_data_num_emu_prevn_bytes: hdr.n_emulation_prevention_bytes as u16,
            ..Default::default()
        };
        s.LongSliceFlags.value = bits(b(hdr.dependent_slice_segment_flag), 1, 1)
            | bits(hdr.type_ as u32, 2, 2)
            | bits(u32::from(hdr.colour_plane_id), 4, 2)
            | bits(b(hdr.sao_luma_flag), 6, 1)
            | bits(b(hdr.sao_chroma_flag), 7, 1)
            | bits(b(hdr.mvd_l1_zero_flag), 8, 1)
            | bits(b(hdr.cabac_init_flag), 9, 1)
            | bits(b(hdr.temporal_mvp_enabled_flag), 10, 1)
            | bits(b(hdr.deblocking_filter_disabled_flag), 11, 1)
            | bits(b(hdr.collocated_from_l0_flag), 12, 1)
            | bits(b(hdr.loop_filter_across_slices_enabled_flag), 13, 1);
        // Weights; chroma's offsets as 7-56 and 7-57 derive them.
        let half = sps.wp_offset_half_range_c as i32;
        let offset = |delta_weight: i8, delta_offset: i16| {
            let weight = (1 << pwt.chroma_log2_weight_denom) + i32::from(delta_weight);
            clip3(-half, half - 1, half + i32::from(delta_offset) - ((half * weight) >> pwt.chroma_log2_weight_denom)) as i8
        };
        let bi = hdr.type_.is_b();
        for i in 0..15 {
            s.delta_luma_weight_l0[i] = pwt.delta_luma_weight_l0[i];
            s.luma_offset_l0[i] = pwt.luma_offset_l0[i];
            if bi {
                s.delta_luma_weight_l1[i] = pwt.delta_luma_weight_l1[i];
                s.luma_offset_l1[i] = pwt.luma_offset_l1[i];
            }
            for j in 0..2 {
                s.delta_chroma_weight_l0[i][j] = pwt.delta_chroma_weight_l0[i][j];
                s.ChromaOffsetL0[i][j] = offset(pwt.delta_chroma_weight_l0[i][j], pwt.delta_chroma_offset_l0[i][j]);
                if bi {
                    s.delta_chroma_weight_l1[i][j] = pwt.delta_chroma_weight_l1[i][j];
                    s.ChromaOffsetL1[i][j] = offset(pwt.delta_chroma_weight_l1[i][j], pwt.delta_chroma_offset_l1[i][j]);
                }
            }
        }
        picture.last = Some((s, slice.nalu.as_ref().to_vec()));
        Ok(())
    }

    fn submit_picture(&mut self, mut picture: HevcPicture) -> StatelessBackendResult<Handle> {
        self.send_slice(&mut picture, true)?;
        self.submit(picture.picture)
    }
}
