//! Parameter submission from the public libva encode headers (no driver implementation code).
use super::*;

#[repr(C)]
struct Misc<T> {
    kind: VAEncMiscParameterType,
    value: T,
}

pub(super) fn parameters(e: &mut Encoder, key: bool) -> Result<()> {
    if key {
        let mut rc = VAEncMiscParameterRateControl {
            bits_per_second: e.bps(),
            target_percentage: ((e.settings.bitrate_kbps.clamp(100, 1_000_000) as u64 * 100_000) / e.bps() as u64).clamp(1, 100) as u32,
            window_size: 1000,
            initial_qp: 26,
            min_qp: 1,
            ..Default::default()
        };
        // va.h rc_flags: reset, disable_frame_skip.
        rc.rc_flags.value = 3;
        if e.settings.hardware_qp.is_none() {
            e.buffer(VABufferType_VAEncMiscParameterBufferType, &Misc { kind: VAEncMiscParameterType_VAEncMiscParameterTypeRateControl, value: rc })?;
        }
        e.buffer(
            VABufferType_VAEncMiscParameterBufferType,
            &Misc {
                kind: VAEncMiscParameterType_VAEncMiscParameterTypeFrameRate,
                value: VAEncMiscParameterFrameRate { framerate: (e.rate.den as u32) << 16 | e.rate.num as u32, ..Default::default() },
            },
        )?;
    }
    match e.format {
        Format::H264 => h264(e, key),
        Format::Hevc => hevc(e, key),
        Format::Av1 => av1(e, key),
        _ => Err(error("unsupported codec")),
    }
}
fn h264(e: &mut Encoder, key: bool) -> Result<()> {
    let frame = (e.index % e.keyint()) as u16;
    let cabac = e.settings.h264_profile != H264Profile::Baseline;
    if key {
        let mut seq = VAEncSequenceParameterBufferH264 {
            level_idc: e.h264_level(),
            intra_period: e.keyint() as u32,
            intra_idr_period: e.keyint() as u32,
            ip_period: 1,
            bits_per_second: e.bps(),
            max_num_ref_frames: 1,
            picture_width_in_mbs: e.w.div_ceil(16) as u16,
            picture_height_in_mbs: e.h.div_ceil(16) as u16,
            frame_cropping_flag: u8::from(!e.w.is_multiple_of(16) || !e.h.is_multiple_of(16)),
            frame_crop_right_offset: (e.w.div_ceil(16) * 16 - e.w) / 2,
            frame_crop_bottom_offset: (e.h.div_ceil(16) * 16 - e.h) / 2,
            vui_parameters_present_flag: 1,
            num_units_in_tick: e.rate.den as u32,
            time_scale: e.rate.num as u32 * 2,
            aspect_ratio_idc: 255,
            sar_width: e.settings.pixel_aspect.unwrap_or((1, 1)).0.clamp(1, 65535),
            sar_height: e.settings.pixel_aspect.unwrap_or((1, 1)).1.clamp(1, 65535),
            ..Default::default()
        };
        // 4:2:0, progressive, direct_8x8; 16-bit frame_num and POC lsb.
        seq.seq_fields.value = 1 | (1 << 2) | (1 << 5) | (12 << 6) | (12 << 12);
        seq.vui_fields.value = 1 | 2 | (1 << 13);
        e.buffer(VABufferType_VAEncSequenceParameterBufferType, &seq)?;
    }
    let invalid = VAPictureH264 { picture_id: VA_INVALID_ID, flags: VA_PICTURE_H264_INVALID, ..Default::default() };
    let current = VAPictureH264 {
        picture_id: e.surfaces[1 + (e.index % 2) as usize],
        frame_idx: frame as u32,
        flags: VA_PICTURE_H264_SHORT_TERM_REFERENCE,
        TopFieldOrderCnt: frame as i32 * 2,
        BottomFieldOrderCnt: frame as i32 * 2,
        ..Default::default()
    };
    let reference = VAPictureH264 {
        picture_id: e.surfaces[1 + ((e.index + 1) % 2) as usize],
        frame_idx: frame.saturating_sub(1) as u32,
        flags: VA_PICTURE_H264_SHORT_TERM_REFERENCE,
        TopFieldOrderCnt: frame.saturating_sub(1) as i32 * 2,
        BottomFieldOrderCnt: frame.saturating_sub(1) as i32 * 2,
        ..Default::default()
    };
    let mut pic = VAEncPictureParameterBufferH264 {
        CurrPic: current,
        ReferenceFrames: [invalid; 16],
        coded_buf: e.coded,
        frame_num: frame,
        pic_init_qp: e.qp(),
        ..Default::default()
    };
    if !key {
        pic.ReferenceFrames[0] = reference;
    }
    pic.pic_fields.value = u32::from(key) | (1 << 1) | (u32::from(cabac) << 3) | (u32::from(e.settings.h264_profile == H264Profile::High) << 8) | (1 << 9);
    e.buffer(VABufferType_VAEncPictureParameterBufferType, &pic)?;
    let mut slice = VAEncSliceParameterBufferH264 {
        num_macroblocks: e.w.div_ceil(16) * e.h.div_ceil(16),
        macroblock_info: VA_INVALID_ID,
        slice_type: if key { 2 } else { 0 },
        idr_pic_id: ((e.index / e.keyint()) % 65536) as u16,
        pic_order_cnt_lsb: frame * 2,
        RefPicList0: [invalid; 32],
        RefPicList1: [invalid; 32],
        ..Default::default()
    };
    if !key {
        slice.RefPicList0[0] = reference;
    }
    e.buffer(VABufferType_VAEncSliceParameterBufferType, &slice)?;
    h264_headers(e, key, frame as u32, cabac)
}
fn hevc(e: &mut Encoder, key: bool) -> Result<()> {
    if !e.w.is_multiple_of(8) || !e.h.is_multiple_of(8) {
        return Err(error("HEVC driver-generated headers currently require dimensions divisible by 8"));
    }
    if key {
        let mut seq = VAEncSequenceParameterBufferHEVC {
            general_profile_idc: 1,
            general_level_idc: 153,
            intra_period: e.keyint() as u32,
            intra_idr_period: e.keyint() as u32,
            ip_period: 1,
            bits_per_second: e.bps(),
            pic_width_in_luma_samples: e.w as u16,
            pic_height_in_luma_samples: e.h as u16,
            log2_diff_max_min_luma_coding_block_size: 3,
            log2_diff_max_min_transform_block_size: 3,
            max_transform_hierarchy_depth_inter: 2,
            max_transform_hierarchy_depth_intra: 2,
            vui_parameters_present_flag: 1,
            vui_num_units_in_tick: e.rate.den as u32,
            vui_time_scale: e.rate.num as u32,
            aspect_ratio_idc: 255,
            sar_width: e.settings.pixel_aspect.unwrap_or((1, 1)).0.clamp(1, 65535),
            sar_height: e.settings.pixel_aspect.unwrap_or((1, 1)).1.clamp(1, 65535),
            ..Default::default()
        };
        seq.seq_fields.value = 1 | (1 << 10) | (1 << 16); // 420, strong intra smoothing, low delay
        seq.vui_fields.value = 1 | (1 << 3);
        e.buffer(VABufferType_VAEncSequenceParameterBufferType, &seq)?;
    }
    let poc = (e.index % e.keyint()) as i32;
    let invalid = VAPictureHEVC { picture_id: VA_INVALID_ID, flags: VA_PICTURE_HEVC_INVALID, ..Default::default() };
    let current = VAPictureHEVC { picture_id: e.surfaces[1 + (e.index % 2) as usize], pic_order_cnt: poc, ..Default::default() };
    let reference = VAPictureHEVC {
        picture_id: e.surfaces[1 + ((e.index + 1) % 2) as usize],
        pic_order_cnt: poc - 1,
        flags: VA_PICTURE_HEVC_RPS_ST_CURR_BEFORE,
        ..Default::default()
    };
    let mut pic = VAEncPictureParameterBufferHEVC {
        decoded_curr_pic: current,
        reference_frames: [invalid; 15],
        coded_buf: e.coded,
        collocated_ref_pic_index: 255,
        pic_init_qp: e.qp(),
        nal_unit_type: if key { 19 } else { 1 },
        ..Default::default()
    };
    if !key {
        pic.reference_frames[0] = reference;
    }
    pic.pic_fields.value = u32::from(key) | ((if key { 1 } else { 2 }) << 1) | (1 << 4) | (1 << 9) | (1 << 16);
    e.buffer(VABufferType_VAEncPictureParameterBufferType, &pic)?;
    let mut slice = VAEncSliceParameterBufferHEVC {
        num_ctu_in_slice: e.w.div_ceil(64) * e.h.div_ceil(64),
        slice_type: if key { 2 } else { 1 },
        ref_pic_list0: [invalid; 15],
        ref_pic_list1: [invalid; 15],
        max_num_merge_cand: 5,
        ..Default::default()
    };
    if !key {
        slice.ref_pic_list0[0] = reference;
    }
    slice.slice_fields.value = 1;
    e.buffer(VABufferType_VAEncSliceParameterBufferType, &slice)?;
    headers::hevc(e, key)
}
fn av1(e: &mut Encoder, key: bool) -> Result<()> {
    if key {
        let mut seq = VAEncSequenceParameterBufferAV1 {
            seq_profile: 0,
            seq_level_idx: 13,
            intra_period: e.keyint() as u32,
            ip_period: 1,
            bits_per_second: e.bps(),
            order_hint_bits_minus_1: 7,
            ..Default::default()
        };
        seq.seq_fields.value = (1 << 2) | (1 << 3) | (1 << 8) | (1 << 17) | (1 << 18); // filter intra, edge filter, order hint, 420
        e.buffer(VABufferType_VAEncSequenceParameterBufferType, &seq)?;
    }
    let mut pic = VAEncPictureParameterBufferAV1 {
        frame_width_minus_1: (e.w - 1) as u16,
        frame_height_minus_1: (e.h - 1) as u16,
        reconstructed_frame: e.surfaces[1 + (e.index % 2) as usize],
        coded_buf: e.coded,
        reference_frames: [if key { VA_INVALID_ID } else { e.surfaces[1 + ((e.index + 1) % 2) as usize] }; 8],
        primary_ref_frame: if key { 7 } else { 0 },
        order_hint: (e.index % e.keyint()) as u8,
        refresh_frame_flags: 255,
        filter_level: [20, 20],
        filter_level_u: 20,
        filter_level_v: 20,
        base_qindex: e.qp(),
        min_base_qindex: 1,
        max_base_qindex: 255,
        tile_cols: 1,
        tile_rows: 1,
        interpolation_filter: 0,
        ..Default::default()
    };
    pic.picture_flags.value = u32::from(!key) | (1 << 9); // inter/key, combined frame OBU
    pic.ref_frame_ctrl_l0.value = if key { 0 } else { 1 }; // LAST_FRAME
    pic.mode_control_flags.value = 2 << 7; // TX_MODE_SELECT
    pic.tile_group_obu_hdr_info.value = 2; // OBU size field present
    if key {
        headers::av1_sequence(e)?;
    }
    headers::av1_picture(e, key, &mut pic)?;
    e.buffer(VABufferType_VAEncPictureParameterBufferType, &pic)?;
    e.buffer(VABufferType_VAEncSliceParameterBufferType, &VAEncTileGroupBufferAV1 { tg_start: 0, tg_end: 0, ..Default::default() })?;
    Ok(())
}

fn h264_headers(e: &mut Encoder, key: bool, frame: u32, cabac: bool) -> Result<()> {
    use filmcraft_bitstream::BitWriter;
    use filmcraft_h264enc::nal;
    if key {
        let profile = match e.settings.h264_profile {
            H264Profile::Baseline => 66,
            H264Profile::Main => 77,
            H264Profile::High => 100,
        };
        let sps = nal::Sps {
            profile_idc: profile,
            constraint_flags: if profile == 66 { 0xc0 } else { 0 },
            level_idc: e.h264_level(),
            width_mbs: e.w.div_ceil(16),
            height_mbs: e.h.div_ceil(16),
            crop_right: e.w.div_ceil(16) * 16 - e.w,
            crop_bottom: e.h.div_ceil(16) * 16 - e.h,
            log2_max_frame_num: 16,
            log2_max_poc_lsb: 16,
            max_num_ref_frames: 1,
            vui: nal::Vui {
                sar: e.settings.pixel_aspect.map(|(n, d)| (n.clamp(1, 65535) as u16, d.clamp(1, 65535) as u16)).unwrap_or((1, 1)),
                video_full_range: false,
                colour_primaries: 1,
                transfer: 1,
                matrix: 1,
                num_units_in_tick: e.rate.den as u32,
                time_scale: e.rate.num as u32 * 2,
                max_num_reorder_frames: 0,
                max_dec_frame_buffering: 1,
            },
        };
        let pps = nal::Pps { cabac, pic_init_qp: e.qp() as i32, chroma_qp_offset: 0, transform_8x8: profile == 100, high: profile == 100 };
        for (mask, kind, ty, rbsp) in [(VA_ENC_PACKED_HEADER_SEQUENCE, 1, 7, sps.rbsp()), (VA_ENC_PACKED_HEADER_PICTURE, 2, 8, pps.rbsp())] {
            if e.packed & mask != 0 {
                let mut bytes = vec![0, 0, 0, 1];
                bytes.extend(nal::nal(3, ty, &rbsp));
                e.packed_header(kind, &bytes, bytes.len() as u32 * 8, true)?;
            }
        }
    }
    if e.packed & VA_ENC_PACKED_HEADER_SLICE != 0 {
        let header = nal::SliceHeader {
            first_mb: 0,
            slice_type: if key { nal::SliceType::I } else { nal::SliceType::P },
            nal_ref_idc: 3,
            idr: key,
            idr_pic_id: ((e.index / e.keyint()) % 65536) as u32,
            frame_num: frame,
            log2_max_frame_num: 16,
            poc_lsb: frame * 2,
            log2_max_poc_lsb: 16,
            l0_modification: None,
            cabac,
            slice_qp: e.qp() as i32,
            pic_init_qp: e.qp() as i32,
            disable_deblocking: 0,
            alpha_offset_div2: 0,
            beta_offset_div2: 0,
        };
        let mut b = BitWriter::new();
        b.write_bytes(&[0, 0, 0, 1, if key { 0x65 } else { 0x61 }]);
        header.write(&mut b);
        let bits = b.bit_len() as u32;
        e.packed_header(3, &b.finish(), bits, false)?;
    }
    Ok(())
}
