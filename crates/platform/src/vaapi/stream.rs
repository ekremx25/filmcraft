//! Driver bitstream to existing MP4 sample entries. Specs: H.264 Annex B, H.265 Annex B,
//! ISO/IEC 14496-15 configuration records, AV1 v1.0.0 OBU syntax / AV1 ISOBMFF v1.3.0.
use super::*;
use filmcraft_bitstream::{annexb_nals, unescape_rbsp};
use filmcraft_isobmff::{Av1Config, AvcConfig, CodecConfig, FourCc, HevcConfig, HevcNalArray, SampleEntry};
pub(super) struct Stream {
    pub entry: SampleEntry,
    format: Format,
    w: u16,
    h: u16,
    ready: bool,
}
impl Stream {
    pub fn new(format: Format, w: u32, h: u32) -> Self {
        Self { entry: SampleEntry::avc(AvcConfig::new(Vec::new(), Vec::new(), 4), w as u16, h as u16), format, w: w as u16, h: h as u16, ready: false }
    }
    pub fn packet(&mut self, data: &[u8], key: bool) -> Result<Vec<u8>> {
        if data.is_empty() {
            return Err(error("driver returned an empty access unit"));
        }
        if self.format == Format::Av1 {
            return self.av1(data);
        }
        if !data.starts_with(&[0, 0, 1]) && !data.starts_with(&[0, 0, 0, 1]) {
            return Err(error("expected Annex-B start code"));
        }
        let nals = annexb_nals(data);
        let (mut vps, mut sps, mut pps) = (Vec::new(), Vec::new(), Vec::new());
        let mut sample = Vec::with_capacity(data.len());
        let mut random_access = false;
        let mut slices = false;
        for n in nals {
            if n.len() < if self.format == Format::H264 { 1 } else { 2 } || n[0] & 0x80 != 0 {
                return Err(error("invalid NAL header"));
            }
            let ty = if self.format == Format::H264 { n[0] & 31 } else { (n[0] >> 1) & 63 };
            match (self.format, ty) {
                (Format::H264, 7) | (Format::Hevc, 33) => sps.push(n.to_vec()),
                (Format::H264, 8) | (Format::Hevc, 34) => pps.push(n.to_vec()),
                (Format::Hevc, 32) => vps.push(n.to_vec()),
                (Format::H264, 9) | (Format::Hevc, 35) => {} // AUD not needed in MP4
                _ => {
                    random_access |= (self.format == Format::H264 && ty == 5) || (self.format == Format::Hevc && (16..=21).contains(&ty));
                    slices |= (self.format == Format::H264 && (1..=5).contains(&ty)) || (self.format == Format::Hevc && ty <= 31);
                    let size = u32::try_from(n.len()).map_err(|_| error("NAL too large"))?;
                    sample.extend_from_slice(&size.to_be_bytes());
                    sample.extend_from_slice(n);
                }
            }
        }
        if !slices || key != random_access {
            return Err(error(format!(
                "driver returned missing slices or unexpected random-access flags (key={key}, random_access={random_access}, slices={slices}, prefix={:02x?})",
                &data[..data.len().min(64)]
            )));
        }
        if !sps.is_empty() && !pps.is_empty() {
            if sps.len() != 1 || pps.len() != 1 || vps.len() > 1 || sps.iter().chain(&pps).chain(&vps).any(|n| n.len() > u16::MAX as usize) {
                return Err(error("unexpected parameter-set count or size"));
            }
            let mut entry = if self.format == Format::H264 {
                let rbsp = unescape_rbsp(sps[0].get(1..).ok_or_else(|| error("truncated SPS"))?);
                let parsed = filmcraft_h264::params::Sps::parse(&rbsp).map_err(|e| error(format!("invalid SPS: {e}")))?;
                let mut sets = vec![None; 32];
                let id = parsed.id as usize;
                *sets.get_mut(id).ok_or_else(|| error("invalid SPS id"))? = Some(parsed);
                filmcraft_h264::params::Pps::parse(&unescape_rbsp(&pps[0][1..]), &sets).map_err(|e| error(format!("invalid PPS: {e}")))?;
                SampleEntry::avc(AvcConfig::new(sps, pps, 4), self.w, self.h)
            } else {
                if vps.is_empty() {
                    return Err(error("missing HEVC VPS"));
                }
                filmcraft_hevc::params::Vps::parse(&unescape_rbsp(&vps[0][2..])).map_err(|e| error(format!("invalid HEVC VPS: {e}")))?;
                filmcraft_hevc::params::Pps::parse(&unescape_rbsp(&pps[0][2..])).map_err(|e| error(format!("invalid HEVC PPS: {e}")))?;
                let rbsp = unescape_rbsp(sps[0].get(2..).ok_or_else(|| error("truncated HEVC SPS"))?);
                let parsed = filmcraft_hevc::params::Sps::parse(&rbsp).map_err(|e| error(format!("invalid HEVC SPS: {e}")))?;
                let constraints = rbsp.get(6..12).ok_or_else(|| error("truncated HEVC profile_tier_level"))?.iter().fold(0u64, |v, b| (v << 8) | *b as u64);
                SampleEntry::hevc(
                    HevcConfig {
                        general_profile_space: parsed.ptl.profile_space,
                        general_tier_flag: parsed.ptl.tier,
                        general_profile_idc: parsed.ptl.profile_idc,
                        general_profile_compatibility_flags: parsed.ptl.compatibility,
                        general_constraint_indicator_flags: constraints,
                        general_level_idc: parsed.ptl.level_idc,
                        chroma_format_idc: parsed.chroma_format_idc as u8,
                        bit_depth_luma: parsed.bit_depth_luma as u8,
                        bit_depth_chroma: parsed.bit_depth_chroma as u8,
                        num_temporal_layers: (parsed.max_sub_layers_minus1 + 1) as u8,
                        temporal_id_nested: rbsp[0] & 1 != 0,
                        length_size: 4,
                        arrays: vec![
                            HevcNalArray { completeness: true, nal_type: 32, nalus: vps },
                            HevcNalArray { completeness: true, nal_type: 33, nalus: sps },
                            HevcNalArray { completeness: true, nal_type: 34, nalus: pps },
                        ],
                        ..Default::default()
                    },
                    self.w,
                    self.h,
                )
            };
            ColorSignal::default().apply_to(&mut entry, false);
            if self.ready && entry != self.entry {
                return Err(error("codec configuration changed during export"));
            }
            self.entry = entry;
            self.ready = true;
        }
        if !self.ready {
            return Err(error("driver did not generate required codec parameter sets; packed headers may be required by this driver"));
        }
        Ok(sample)
    }
    pub(super) fn set_av1_sequence(&mut self, data: &[u8], payload: &[u8]) -> Result<()> {
        let seq = filmcraft_av1::SequenceHeader::parse(payload).map_err(|e| error(format!("invalid AV1 sequence: {e}")))?;
        if seq.profile != 0 || seq.color.bit_depth != 8 {
            return Err(error("unexpected AV1 profile/depth"));
        }
        self.entry = SampleEntry::video(
            FourCc(*b"av01"),
            CodecConfig::Av1(Av1Config {
                seq_profile: seq.profile,
                seq_level_idx_0: seq.seq_level_idx[0],
                chroma_subsampling_x: true,
                chroma_subsampling_y: true,
                chroma_sample_position: seq.color.chroma_sample_position,
                config_obus: data.to_vec(),
                ..Default::default()
            }),
            self.w,
            self.h,
        );
        ColorSignal::default().apply_to(&mut self.entry, false);
        self.ready = true;
        Ok(())
    }
    fn av1(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        let mut pos = 0usize;
        let mut out = Vec::with_capacity(data.len());
        let mut frame = false;
        while pos < data.len() {
            let start = pos;
            let header = data[pos];
            pos += 1;
            if header & 0x83 != 2 {
                return Err(error("invalid AV1 OBU header or missing size field"));
            }
            if header & 4 != 0 {
                pos = pos.checked_add(1).ok_or_else(|| error("OBU overflow"))?;
            }
            let mut size = 0u64;
            let mut done = false;
            for shift in (0..56).step_by(7) {
                let b = *data.get(pos).ok_or_else(|| error("truncated OBU length"))?;
                pos += 1;
                size |= ((b & 127) as u64) << shift;
                if b & 128 == 0 {
                    done = true;
                    break;
                }
            }
            if !done {
                return Err(error("oversized OBU length"));
            }
            let end = pos.checked_add(usize::try_from(size).map_err(|_| error("OBU too large"))?).ok_or_else(|| error("OBU overflow"))?;
            let payload = data.get(pos..end).ok_or_else(|| error("truncated OBU payload"))?;
            let ty = (header >> 3) & 15;
            if ty == 1 {
                self.set_av1_sequence(&data[start..end], payload)?;
            }
            frame |= ty == 3 || ty == 6;
            // AV1 ISOBMFF excludes temporal delimiter OBUs from samples.
            if ty != 2 {
                out.extend_from_slice(&data[start..end]);
            }
            pos = end;
        }
        if !self.ready || !frame {
            return Err(error(format!("missing AV1 sequence/frame header (ready={}, frame={frame}, prefix={:02x?})", self.ready, &data[..data.len().min(64)])));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn annexb_and_avcc_reuse_existing_parser() {
        let mut stream = Stream::new(Format::H264, 32, 32);
        stream.ready = true;
        let sample = stream.packet(&[0, 0, 0, 1, 0x65, 0x88, 0x99, 0, 0, 1, 6, 0x80], true).unwrap();
        assert_eq!(filmcraft_bitstream::length_prefixed_nals(&sample, 4).unwrap(), vec![&[0x65, 0x88, 0x99][..], &[6, 0x80][..]]);
        assert!(stream.packet(&[0, 0, 1, 0x41, 0x88], true).is_err());
    }
    #[test]
    fn malformed_streams_fail_without_panicking() {
        for format in [Format::H264, Format::Hevc, Format::Av1] {
            for bytes in [&[][..], &[0, 0, 1], &[0, 0, 1, 0xff], &[0, 0, 1, 0x67], &[0x0a, 0xff, 0xff], &[0x0a, 0x7f], &[0xff; 32]] {
                assert!(Stream::new(format, 32, 32).packet(bytes, true).is_err());
            }
        }
    }
}

#[cfg(test)]
mod mutation_tests {
    use super::*;
    #[test]
    fn parameter_sets_are_extracted_and_truncations_are_bounded() {
        let enc = filmcraft_h264enc::Encoder::new(filmcraft_h264enc::EncoderConfig::new(32, 32, 30, 1)).unwrap();
        let (sps, pps) = enc.sps_pps();
        let mut bytes = Vec::new();
        for nal in [&sps[..], &pps[..], &[0x65, 0x88, 0x99]] {
            bytes.extend([0, 0, 0, 1]);
            bytes.extend(nal);
        }
        let mut stream = Stream::new(Format::H264, 32, 32);
        let packet = stream.packet(&bytes, true).unwrap();
        assert_eq!(filmcraft_bitstream::length_prefixed_nals(&packet, 4).unwrap(), vec![&[0x65, 0x88, 0x99][..]]);
        match stream.entry.codec {
            CodecConfig::Avc(cfg) => {
                assert_eq!(cfg.sps, vec![sps]);
                assert_eq!(cfg.pps, vec![pps]);
            }
            _ => panic!("wrong codec"),
        }
        for format in [Format::H264, Format::Hevc, Format::Av1] {
            for end in 0..bytes.len() {
                assert!(
                    std::panic::catch_unwind(|| {
                        let _ = Stream::new(format, 32, 32).packet(&bytes[..end], true);
                    })
                    .is_ok()
                );
            }
            for index in 0..bytes.len() {
                for bit in 0..8 {
                    let mut corrupt = bytes.clone();
                    corrupt[index] ^= 1 << bit;
                    assert!(
                        std::panic::catch_unwind(|| {
                            let _ = Stream::new(format, 32, 32).packet(&corrupt, true);
                        })
                        .is_ok()
                    );
                }
            }
        }
    }
}
