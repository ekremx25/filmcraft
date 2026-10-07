//! Minimal low-delay headers, written from ITU-T H.265 (08/2021) §7.3 and AV1 v1.0.0 §5.
//! Compression is entirely the driver's job; these headers describe its submitted parameters.
use super::*;
use filmcraft_bitstream::{BitWriter, escape_rbsp};

fn ptl(b: &mut BitWriter) {
    b.write_bits(1, 8); // Main, main tier
    b.write_bits(1 << 30, 32); // Main compatibility
    b.write_bits(0x9000, 16);
    b.write_bits(0, 32); // progressive, frame-only source
    b.write_bits(153, 8); // level 5.1
}
fn hevc_nal(e: &mut Encoder, kind: u32, ty: u8, mut b: BitWriter) -> Result<()> {
    b.rbsp_trailing();
    let mut data = vec![0, 0, 0, 1, ty << 1, 1];
    data.extend(escape_rbsp(&b.finish()));
    e.packed_header(kind, &data, data.len() as u32 * 8, true)
}
pub(super) fn hevc(e: &mut Encoder, key: bool) -> Result<()> {
    if key && e.packed & VA_ENC_PACKED_HEADER_SEQUENCE != 0 {
        let mut b = BitWriter::new();
        b.write_bits(0, 4);
        b.write_bits(3, 2);
        b.write_bits(0, 6);
        b.write_bits(0, 3);
        b.write_bit(true);
        b.write_bits(65535, 16);
        ptl(&mut b);
        b.write_bit(false);
        b.write_ue(1);
        b.write_ue(0);
        b.write_ue(0); // ordering: 2 pictures, no reorder
        b.write_bits(0, 6);
        b.write_ue(0);
        b.write_bit(false);
        b.write_bit(false);
        hevc_nal(e, 1, 32, b)?;
        let mut b = BitWriter::new();
        b.write_bits(0, 4);
        b.write_bits(0, 3);
        b.write_bit(true);
        ptl(&mut b);
        b.write_ue(0);
        b.write_ue(1);
        b.write_ue(e.w);
        b.write_ue(e.h);
        b.write_bit(false);
        b.write_ue(0);
        b.write_ue(0);
        b.write_ue(12); // 8 bit, 16 bit POC
        b.write_bit(false);
        b.write_ue(1);
        b.write_ue(0);
        b.write_ue(0);
        for v in [0, 3, 0, 3, 2, 2] {
            b.write_ue(v);
        } // CU8..64, TU4..32, hierarchy 2
        for v in [false, false, false, false] {
            b.write_bit(v);
        } // scaling, AMP, SAO, PCM
        b.write_ue(0);
        b.write_bit(false);
        b.write_bit(false);
        b.write_bit(true); // no RPS, LT, TMVP; smoothing
        b.write_bit(true); // VUI
        b.write_bit(true);
        b.write_bits(255, 8);
        let (n, d) = e.settings.pixel_aspect.unwrap_or((1, 1));
        b.write_bits(n.clamp(1, 65535), 16);
        b.write_bits(d.clamp(1, 65535), 16);
        b.write_bit(false);
        b.write_bit(true);
        b.write_bits(5, 3);
        b.write_bit(false);
        b.write_bit(true);
        for _ in 0..3 {
            b.write_bits(1, 8);
        } // limited BT.709
        for _ in 0..5 {
            b.write_bit(false);
        } // chroma location, neutral, field, frame info, display window
        b.write_bit(true);
        b.write_bits(e.rate.den as u32, 32);
        b.write_bits(e.rate.num as u32, 32);
        b.write_bit(false);
        b.write_bit(false);
        b.write_bit(false); // proportional POC, HRD, restriction
        b.write_bit(false); // SPS extension
        hevc_nal(e, 1, 33, b)?;
    }
    if key && e.packed & VA_ENC_PACKED_HEADER_PICTURE != 0 {
        let mut b = BitWriter::new();
        b.write_ue(0);
        b.write_ue(0);
        b.write_bit(false);
        b.write_bit(false);
        b.write_bits(0, 3);
        b.write_bit(false);
        b.write_bit(false);
        b.write_ue(0);
        b.write_ue(0);
        b.write_se(e.qp() as i32 - 26);
        b.write_bit(false);
        b.write_bit(false);
        b.write_bit(true);
        b.write_ue(0); // CU QP delta
        b.write_se(0);
        b.write_se(0);
        for _ in 0..6 {
            b.write_bit(false);
        } // chroma offsets, weights, bypass, tiles, WPP
        b.write_bit(true);
        b.write_bit(false);
        b.write_bit(false);
        b.write_bit(false); // filter across slices, deblock control, scaling, list mods
        b.write_ue(0);
        b.write_bit(false);
        b.write_bit(false);
        hevc_nal(e, 2, 34, b)?;
    }
    if e.packed & VA_ENC_PACKED_HEADER_SLICE != 0 {
        let mut b = BitWriter::new();
        b.write_bytes(&[0, 0, 0, 1, if key { 38 } else { 2 }, 1]);
        b.write_bit(true);
        if key {
            b.write_bit(false);
        }
        b.write_ue(0);
        b.write_ue(if key { 2 } else { 1 });
        if !key {
            b.write_bits((e.index % e.keyint()) as u32, 16);
            b.write_bit(false); // explicit short-term RPS
            b.write_ue(1);
            b.write_ue(0);
            b.write_ue(0);
            b.write_bit(true); // previous POC only
            b.write_bit(false); // default reference count
            b.write_ue(0); // five_minus_max_num_merge_cand
        }
        b.write_se(0);
        b.write_bit(true); // QP delta, filter across slices
        let bits = b.bit_len() as u32;
        e.packed_header(3, &b.finish(), bits, false)?;
    }
    Ok(())
}

pub(super) fn av1_sequence(e: &mut Encoder) -> Result<()> {
    let mut b = BitWriter::new();
    b.write_bits(0, 3);
    b.write_bit(false);
    b.write_bit(false); // Main, video, full header
    b.write_bit(false);
    b.write_bit(false);
    b.write_bits(0, 5); // timing, initial delay, operating points
    b.write_bits(0, 12);
    b.write_bits(13, 5);
    b.write_bit(false); // op id, level 5.1, main tier
    b.write_bits(15, 4);
    b.write_bits(15, 4);
    b.write_bits(e.w - 1, 16);
    b.write_bits(e.h - 1, 16);
    b.write_bit(false);
    b.write_bit(false);
    b.write_bit(true);
    b.write_bit(true); // frame ids, SB128, filter intra, edge filter
    for _ in 0..4 {
        b.write_bit(false);
    } // inter-intra, masked, warped, dual filter
    b.write_bit(true);
    b.write_bit(false);
    b.write_bit(false); // order hint, joint compound, ref MVs
    b.write_bit(false);
    b.write_bit(false); // screen content choose/force
    b.write_bits(7, 3); // 8 order hint bits
    b.write_bit(false);
    b.write_bit(false);
    b.write_bit(false); // superres, CDEF, restoration
    b.write_bit(false);
    b.write_bit(false);
    b.write_bit(true); // 8 bit, monochrome, color description
    for _ in 0..3 {
        b.write_bits(1, 8);
    }
    b.write_bit(false);
    b.write_bits(0, 2);
    b.write_bit(false);
    b.write_bit(false); // limited, chroma pos, separate UV, grain
    b.rbsp_trailing();
    let payload = b.finish();
    let mut data = vec![0x0a, payload.len() as u8];
    data.extend(payload);
    e.stream.set_av1_sequence(&data, &data[2..])?;
    e.packed_header(1, &data, data.len() as u32 * 8, true)
}

pub(super) fn av1_picture(e: &mut Encoder, key: bool, pic: &mut VAEncPictureParameterBufferAV1) -> Result<()> {
    // A single tile is deliberately bounded by AV1's max tile width/area.
    if e.w > 4096 || e.w as u64 * e.h as u64 > 4096 * 2304 {
        return Err(error("AV1 single-tile hardware export is limited to width 4096 and 4096×2304 pixels"));
    }
    let mut b = BitWriter::new();
    b.write_bytes(&[0x32, 0x80, 0x80, 0x80, 0]); // Frame OBU, four-byte LEB128 size patched by driver
    b.write_bit(false);
    b.write_bits(u32::from(!key), 2);
    b.write_bit(true); // show-existing, type, show
    if !key {
        b.write_bit(false);
    } // error resilient
    b.write_bit(false);
    b.write_bit(false);
    b.write_bits((e.index % e.keyint()) as u32 & 255, 8); // disable CDF, size override, order hint
    if !key {
        b.write_bits(0, 3);
        b.write_bits(255, 8); // primary ref, refresh all
        b.write_bit(false);
        for _ in 0..7 {
            b.write_bits(0, 3);
        } // explicit reference indices, slot zero
    }
    b.write_bit(false); // render and frame size equal
    if !key {
        b.write_bit(false);
        b.write_bit(false);
        b.write_bits(0, 2);
        b.write_bit(false); // high precision MV, fixed interpolation, motion mode switch
    }
    b.write_bit(false); // disable_frame_end_update_cdf
    b.write_bit(true); // uniform tile spacing
    if e.w.div_ceil(64) > 1 {
        b.write_bit(false);
    } // one column
    if e.h.div_ceil(64) > 1 {
        b.write_bit(false);
    } // one row
    pic.bit_offset_qindex = b.bit_len() as u32;
    b.write_bits(pic.base_qindex as u32, 8);
    b.write_bit(false);
    b.write_bit(false);
    b.write_bit(false);
    b.write_bit(false); // YDC, UDC, UAC, qmatrix
    pic.bit_offset_segmentation = b.bit_len() as u32;
    b.write_bit(false); // segmentation disabled
    b.write_bit(false); // delta Q disabled
    pic.bit_offset_loopfilter_params = b.bit_len() as u32;
    b.write_bits(20, 6);
    b.write_bits(20, 6);
    b.write_bits(20, 6);
    b.write_bits(20, 6);
    b.write_bits(0, 3);
    b.write_bit(false);
    b.write_bit(true); // TX_MODE_SELECT
    if !key {
        b.write_bit(false);
    } // single reference
    b.write_bit(false); // reduced transform set
    if !key {
        for _ in 0..7 {
            b.write_bit(false);
        }
    } // global motion identity
    pic.byte_offset_frame_hdr_obu_size = 1;
    pic.size_in_bits_frame_hdr_obu = b.bit_len() as u32;
    let bits = b.bit_len() as u32;
    e.packed_header(2, &b.finish(), bits, true)
}
