#![allow(non_upper_case_globals)]
//! Native VA-API encode (libva 1.24 public headers); no subprocess encoder.
//! Only this module contains the Linux media FFI. Input/upload is separate from codec submission
//! so a future DMA-BUF importer can replace the CPU NV12 staging path.
use filmcraft_export::{hardware::Capability, *};
use filmcraft_time::FrameRate;
use std::{
    ffi::{CStr, c_void},
    fs::File,
    os::fd::AsRawFd,
    path::PathBuf,
    ptr,
};

// bindgen output is declarations and ABI accessors, not codec implementation. Its generated
// pointer operations are constrained by the safe wrappers below; do not hand-edit it.
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_imports,
    clippy::all,
    clippy::undocumented_unsafe_blocks,
    clippy::unwrap_used,
    clippy::expect_used
)]
mod ffi {
    include!(concat!(env!("OUT_DIR"), "/va.rs"));
}
use ffi::*;
mod codecs;
mod headers;
mod input;
mod stream;

fn error(s: impl Into<String>) -> ExportError {
    ExportError::Encode(format!("VAAPI: {}", s.into()))
}
fn check(status: i32, operation: &str) -> Result<()> {
    if status == 0 { Ok(()) } else { Err(error(format!("{operation} failed (VAStatus {status:#x})"))) }
}

struct Device {
    api: LibVa,
    _drm: libloading::Library,
    _file: File,
    display: VADisplay,
    node: String,
}
// SAFETY: libva displays may be used from different threads. This object is moved, never shared;
// all calls and resource destruction remain serialized by the owning encoder's &mut self.
unsafe impl Send for Device {}
impl Device {
    fn open(path: &std::path::Path) -> Result<Self> {
        let file = File::options().read(true).write(true).open(path).map_err(|e| error(format!("{}: {e}", path.display())))?;
        // SAFETY: the system SONAMEs provide the ABI declared in the libva public headers.
        let (api, drm) = unsafe {
            (LibVa::new("libva.so.2").map_err(|e| error(e.to_string()))?, libloading::Library::new("libva-drm.so.2").map_err(|e| error(e.to_string()))?)
        };
        // SAFETY: exact vaGetDisplayDRM signature; library and descriptor outlive the display.
        let display = unsafe {
            let get: libloading::Symbol<unsafe extern "C" fn(i32) -> VADisplay> = drm.get(b"vaGetDisplayDRM\0").map_err(|e| error(e.to_string()))?;
            get(file.as_raw_fd())
        };
        if display.is_null() {
            return Err(error("vaGetDisplayDRM returned no display"));
        }
        let (mut major, mut minor) = (0, 0);
        // SAFETY: display comes from libva; outputs point to live local integers.
        check(unsafe { (api.vaInitialize)(display, &mut major, &mut minor) }, "vaInitialize")?;
        Ok(Self { api, _drm: drm, _file: file, display, node: path.display().to_string() })
    }
    fn attributes(&self, profile: VAProfile, entry: VAEntrypoint) -> Result<Vec<VAConfigAttrib>> {
        let mut attrs =
            [VAConfigAttribType_VAConfigAttribRTFormat, VAConfigAttribType_VAConfigAttribRateControl, VAConfigAttribType_VAConfigAttribEncPackedHeaders]
                .map(|type_| VAConfigAttrib { type_, value: 0 });
        // SAFETY: attribute array length is passed accurately, live display and enum values.
        check(unsafe { (self.api.vaGetConfigAttributes)(self.display, profile, entry, attrs.as_mut_ptr(), attrs.len() as i32) }, "vaGetConfigAttributes")?;
        Ok(attrs.to_vec())
    }
    fn candidates(&self) -> Result<Vec<Candidate>> {
        // SAFETY: valid initialized display, these functions return required output capacities.
        let (np, ne) = unsafe { ((self.api.vaMaxNumProfiles)(self.display), (self.api.vaMaxNumEntrypoints)(self.display)) };
        if !(1..=1024).contains(&np) || !(1..=1024).contains(&ne) {
            return Err(error("invalid driver capability count"));
        }
        let mut profiles = vec![0; np as usize];
        let mut n = 0;
        // SAFETY: array has vaMaxNumProfiles capacity, count output is live.
        check(unsafe { (self.api.vaQueryConfigProfiles)(self.display, profiles.as_mut_ptr(), &mut n) }, "vaQueryConfigProfiles")?;
        let profiles = profiles.get(..usize::try_from(n).unwrap_or(usize::MAX)).ok_or_else(|| error("invalid profile count"))?;
        let mut result = Vec::new();
        for &profile in profiles {
            let Some((format, h264)) = profile_codec(profile) else { continue };
            let mut entries = vec![0; ne as usize];
            // SAFETY: array has vaMaxNumEntrypoints capacity.
            check(unsafe { (self.api.vaQueryConfigEntrypoints)(self.display, profile, entries.as_mut_ptr(), &mut n) }, "vaQueryConfigEntrypoints")?;
            for &entry in entries.get(..usize::try_from(n).unwrap_or(usize::MAX)).ok_or_else(|| error("invalid entrypoint count"))? {
                if !matches!(entry, VAEntrypoint_VAEntrypointEncSlice | VAEntrypoint_VAEntrypointEncSliceLP) {
                    continue;
                }
                let a = self.attributes(profile, entry)?;
                if a[0].value == VA_ATTRIB_NOT_SUPPORTED || a[0].value & VA_RT_FORMAT_YUV420 == 0 {
                    continue;
                }
                let required = required_headers(format);
                if a[2].value == VA_ATTRIB_NOT_SUPPORTED || a[2].value & required != required {
                    continue;
                }
                let rc = if a[1].value == VA_ATTRIB_NOT_SUPPORTED { 0 } else { a[1].value };
                result.push(Candidate {
                    profile,
                    entry,
                    cap: Capability {
                        format,
                        profile: h264,
                        device: self.node.clone(),
                        cbr: rc & VA_RC_CBR != 0,
                        vbr: rc & VA_RC_VBR != 0,
                        cqp: rc & VA_RC_CQP != 0,
                    },
                });
            }
        }
        Ok(result)
    }
}
impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: all encode resources have already been destroyed; file and libraries are live.
        unsafe {
            (self.api.vaTerminate)(self.display);
        }
    }
}
struct Candidate {
    profile: VAProfile,
    entry: VAEntrypoint,
    cap: Capability,
}
fn profile_codec(p: VAProfile) -> Option<(Format, Option<H264Profile>)> {
    Some(match p {
        VAProfile_VAProfileH264ConstrainedBaseline => (Format::H264, Some(H264Profile::Baseline)),
        VAProfile_VAProfileH264Main => (Format::H264, Some(H264Profile::Main)),
        VAProfile_VAProfileH264High => (Format::H264, Some(H264Profile::High)),
        VAProfile_VAProfileHEVCMain => (Format::Hevc, None),
        VAProfile_VAProfileAV1Profile0 => (Format::Av1, None),
        _ => return None,
    })
}
fn required_headers(format: Format) -> u32 {
    VA_ENC_PACKED_HEADER_SEQUENCE | VA_ENC_PACKED_HEADER_PICTURE | if format == Format::Av1 { 0 } else { VA_ENC_PACKED_HEADER_SLICE }
}
fn nodes() -> Vec<PathBuf> {
    let mut paths: Vec<_> = std::fs::read_dir("/dev/dri")
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_str().is_some_and(|n| n.strip_prefix("renderD").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))))
        .map(|e| e.path())
        .collect();
    paths.sort();
    paths
}

pub fn register() {
    let mut caps = Vec::new();
    for path in nodes() {
        match Device::open(&path).and_then(|d| d.candidates()) {
            Ok(cs) => caps.extend(cs.into_iter().map(|c| c.cap)),
            Err(e) => log::info!("VAAPI capability discovery: {e}"),
        }
    }
    filmcraft_export::hardware::register(factory, caps);
}

pub fn factory(format: Format, w: u32, h: u32, rate: FrameRate, s: &ExportSettings) -> Option<Result<Box<dyn VideoEncoder>>> {
    if s.video_encoding == VideoEncoding::Software || !matches!(format, Format::H264 | Format::Hevc | Format::Av1) {
        return None;
    }
    Some(create(format, w, h, rate, s).map(|e| Box::new(e) as Box<dyn VideoEncoder>))
}
fn create(format: Format, w: u32, h: u32, rate: FrameRate, s: &ExportSettings) -> Result<Encoder> {
    input::validate(w, h)?;
    if s.signal != ColorSignal::default() || s.format.is_mxf() || s.field_order != FieldOrder::Progressive || s.bitrate_mode == BitrateMode::Vbr2Pass {
        return Err(error("hardware export supports progressive SDR Rec.709 MP4/MOV with CBR or one-pass VBR; use Software for HDR, MXF or two-pass encoding"));
    }
    if rate.num <= 0 || rate.num > 65535 || rate.den <= 0 || rate.den > 65535 {
        return Err(error("frame rate numerator/denominator must be in 1..65535"));
    }
    if s.keyframe_distance.is_some_and(|k| k == 0 || k > 600) {
        return Err(error("keyframe distance must be in 1..600"));
    }
    if format != Format::H264 {
        let max_kbps = if s.bitrate_mode == BitrateMode::Cbr { s.bitrate_kbps } else { s.max_bitrate_kbps.unwrap_or(s.bitrate_kbps.saturating_mul(3) / 2) };
        if w > 4096 || w as u64 * h as u64 > 8_912_896 || rate.num > rate.den * 60 || (s.hardware_qp.is_none() && max_kbps > 40_000) {
            return Err(error("HEVC/AV1 Main level 5.1 hardware export is limited to width 4096, 8,912,896 pixels, 60 fps and maximum bitrate 40 Mbps"));
        }
    }
    let mut reasons = Vec::new();
    for path in nodes() {
        let device = match Device::open(&path) {
            Ok(d) => d,
            Err(e) => {
                reasons.push(e.to_string());
                continue;
            }
        };
        let candidates = device.candidates()?;
        // A fresh display per attempt keeps failed initialization isolated.
        for c in candidates.into_iter().filter(|c| c.cap.format == format && (format != Format::H264 || c.cap.profile == Some(s.h264_profile))) {
            match Encoder::new(Device::open(&path)?, c, w, h, rate, s) {
                Ok(enc) => return Ok(enc),
                Err(e) => reasons.push(e.to_string()),
            }
        }
    }
    Err(error(format!("{} hardware encoding unavailable for selected profile/rate control on DRM render nodes. {}", format.label(), reasons.join("; "))))
}

struct Encoder {
    device: Device,
    config: u32,
    context: u32,
    surfaces: [u32; 3], // upload + two alternating reconstructed references
    coded: u32,
    packed: u32,
    image: VAImage,
    buffers: Vec<u32>,
    format: Format,
    w: u32,
    h: u32,
    rate: FrameRate,
    settings: ExportSettings,
    index: u64,
    nv12: Vec<u8>,
    stream: stream::Stream,
}
impl Encoder {
    fn new(device: Device, c: Candidate, w: u32, h: u32, rate: FrameRate, s: &ExportSettings) -> Result<Self> {
        let rc = if s.hardware_qp.is_some() {
            VA_RC_CQP
        } else if s.bitrate_mode == BitrateMode::Cbr {
            VA_RC_CBR
        } else {
            VA_RC_VBR
        };
        if (rc == VA_RC_CQP && !c.cap.cqp) || (rc == VA_RC_CBR && !c.cap.cbr) || (rc == VA_RC_VBR && !c.cap.vbr) {
            return Err(error("selected rate control is not exposed by this profile/entrypoint"));
        }
        if s.hardware_qp.is_some_and(|qp| qp == 0 || (c.cap.format != Format::Av1 && qp > 51)) {
            return Err(error("constant QP must be 1..51 (AV1: 1..255)"));
        }
        let mut enc = Self {
            device,
            config: VA_INVALID_ID,
            context: VA_INVALID_ID,
            surfaces: [VA_INVALID_ID; 3],
            coded: VA_INVALID_ID,
            packed: 0,
            image: VAImage { image_id: VA_INVALID_ID, ..Default::default() },
            buffers: Vec::new(),
            format: c.cap.format,
            w,
            h,
            rate,
            settings: s.clone(),
            index: 0,
            nv12: vec![0; (w * h * 3 / 2) as usize],
            stream: stream::Stream::new(c.cap.format, w, h),
        };
        let supported = enc.device.attributes(c.profile, c.entry)?[2].value;
        enc.packed = if supported == VA_ATTRIB_NOT_SUPPORTED {
            0
        } else {
            supported & (VA_ENC_PACKED_HEADER_SEQUENCE | VA_ENC_PACKED_HEADER_PICTURE | VA_ENC_PACKED_HEADER_SLICE)
        };
        if enc.format == Format::Av1 {
            enc.packed &= VA_ENC_PACKED_HEADER_SEQUENCE | VA_ENC_PACKED_HEADER_PICTURE;
        }
        let required = required_headers(enc.format);
        if enc.packed & required != required {
            return Err(error(format!(
                "{} profile {} entrypoint {} lacks required packed headers ({:#x}, required {:#x})",
                enc.device.node, c.profile, c.entry, enc.packed, required
            )));
        }
        let mut attrs = [
            VAConfigAttrib { type_: VAConfigAttribType_VAConfigAttribRTFormat, value: VA_RT_FORMAT_YUV420 },
            VAConfigAttrib { type_: VAConfigAttribType_VAConfigAttribRateControl, value: rc },
            VAConfigAttrib { type_: VAConfigAttribType_VAConfigAttribEncPackedHeaders, value: enc.packed },
        ];
        let d = &enc.device;
        // SAFETY: all outputs live in the RAII owner; bounded dimensions and correct buffer counts.
        unsafe {
            check((d.api.vaCreateConfig)(d.display, c.profile, c.entry, attrs.as_mut_ptr(), attrs.len() as i32, &mut enc.config), "vaCreateConfig")?;
            check((d.api.vaCreateSurfaces)(d.display, VA_RT_FORMAT_YUV420, w, h, enc.surfaces.as_mut_ptr(), 3, ptr::null_mut(), 0), "vaCreateSurfaces")?;
            check(
                (d.api.vaCreateContext)(d.display, enc.config, w as i32, h as i32, VA_PROGRESSIVE as i32, enc.surfaces.as_mut_ptr(), 3, &mut enc.context),
                "vaCreateContext",
            )?;
            check(
                (d.api.vaCreateBuffer)(d.display, enc.context, VABufferType_VAEncCodedBufferType, w * h * 4 + 65536, 1, ptr::null_mut(), &mut enc.coded),
                "coded buffer",
            )?;
            let mut format = VAImageFormat { fourcc: VA_FOURCC_NV12, byte_order: VA_LSB_FIRST, bits_per_pixel: 12, ..Default::default() };
            check((d.api.vaCreateImage)(d.display, &mut format, w as i32, h as i32, &mut enc.image), "NV12 staging image")?;
        }
        // Probe one black frame before returning. Missing driver-generated configuration or late
        // initialization failures can still fall back in Auto before any output is written.
        let rgba = vec![0; (w * h * 4) as usize];
        enc.encode(&EncoderFrame { width: w, height: h, rgba: &rgba, hdr: None, index: 0 })?;
        enc.index = 0;
        // SAFETY: query returns a driver-owned, NUL-terminated string valid for the display lifetime.
        let vendor = unsafe { (enc.device.api.vaQueryVendorString)(enc.device.display) };
        let vendor = if vendor.is_null() {
            "unknown driver".into()
        } else {
            // SAFETY: vendor was checked non-null and libva guarantees a C string.
            unsafe { CStr::from_ptr(vendor) }.to_string_lossy().into_owned()
        };
        log::info!(
            "FilmCraft export encoder: VAAPI {}; DRM node: {}; driver: {}; profile: {}; entrypoint: {}; rate control: {}; resolution: {}x{}",
            enc.format.label(),
            enc.device.node,
            vendor,
            c.profile,
            c.entry,
            s.hardware_qp.map(|q| format!("CQP {q}")).unwrap_or_else(|| s.bitrate_mode.label().into()),
            w,
            h
        );
        Ok(enc)
    }
    fn buffer<T>(&mut self, kind: VABufferType, value: &T) -> Result<()> {
        let mut id = VA_INVALID_ID;
        // SAFETY: used only with repr(C) libva parameter structs; libva copies exactly size_of<T>
        // bytes synchronously. The immutable input is not changed by vaCreateBuffer.
        check(
            // SAFETY: exact buffer size and live input pointer; libva copies synchronously.
            unsafe {
                (self.device.api.vaCreateBuffer)(
                    self.device.display,
                    self.context,
                    kind,
                    std::mem::size_of::<T>() as u32,
                    1,
                    ptr::from_ref(value).cast_mut().cast(),
                    &mut id,
                )
            },
            "parameter buffer",
        )?;
        self.buffers.push(id);
        Ok(())
    }
    fn packed_header(&mut self, kind: u32, data: &[u8], bits: u32, escaped: bool) -> Result<()> {
        self.buffer(
            VABufferType_VAEncPackedHeaderParameterBufferType,
            &VAEncPackedHeaderParameterBuffer { type_: kind, bit_length: bits, has_emulation_bytes: u8::from(escaped), ..Default::default() },
        )?;
        let mut id = VA_INVALID_ID;
        // SAFETY: the byte slice lives through the call; libva copies its exact length.
        check(
            // SAFETY: exact buffer size and live input pointer; libva copies synchronously.
            unsafe {
                (self.device.api.vaCreateBuffer)(
                    self.device.display,
                    self.context,
                    VABufferType_VAEncPackedHeaderDataBufferType,
                    data.len() as u32,
                    1,
                    data.as_ptr().cast_mut().cast(),
                    &mut id,
                )
            },
            "packed header",
        )?;
        self.buffers.push(id);
        Ok(())
    }
    fn clear_buffers(&mut self) {
        for id in self.buffers.drain(..) {
            // SAFETY: these ids are owned by this encoder, no mapped memory remains.
            unsafe {
                (self.device.api.vaDestroyBuffer)(self.device.display, id);
            }
        }
    }
    fn keyint(&self) -> u64 {
        self.settings.keyframe_distance.unwrap_or_else(|| (self.rate.num * 2 / self.rate.den).max(1) as u32).min(600) as u64
    }
    fn h264_level(&self) -> u8 {
        let needed = filmcraft_h264enc::nal::pick_level(
            self.w.div_ceil(16),
            self.h.div_ceil(16),
            self.rate.num as f64 / self.rate.den as f64,
            1,
            self.settings.hardware_qp.is_none().then_some(self.bps() / 1000),
            self.settings.h264_profile == H264Profile::High,
        );
        self.settings.h264_level.map_or(needed, |level| filmcraft_h264enc::nal::level_at_least(level, needed))
    }
    fn qp(&self) -> u8 {
        self.settings.hardware_qp.unwrap_or(if self.format == Format::Av1 { 100 } else { 26 })
    }
    fn bps(&self) -> u32 {
        if self.settings.bitrate_mode == BitrateMode::Cbr {
            return self.settings.bitrate_kbps.clamp(100, 1_000_000) * 1000;
        }
        self.settings.max_bitrate_kbps.unwrap_or(self.settings.bitrate_kbps.saturating_mul(3) / 2).clamp(100, 1_000_000) * 1000
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        self.clear_buffers();
        let d = &self.device;
        // SAFETY: valid owned ids only; destruction order releases dependents before the display.
        unsafe {
            if self.image.image_id != VA_INVALID_ID {
                (d.api.vaDestroyImage)(d.display, self.image.image_id);
            }
            if self.coded != VA_INVALID_ID {
                (d.api.vaDestroyBuffer)(d.display, self.coded);
            }
            if self.context != VA_INVALID_ID {
                (d.api.vaDestroyContext)(d.display, self.context);
            }
            for id in &mut self.surfaces {
                if *id != VA_INVALID_ID {
                    (d.api.vaDestroySurfaces)(d.display, id, 1);
                }
            }
            if self.config != VA_INVALID_ID {
                (d.api.vaDestroyConfig)(d.display, self.config);
            }
        }
    }
}

struct Mapping<'a> {
    d: &'a Device,
    id: u32,
    ptr: *mut c_void,
}
impl<'a> Mapping<'a> {
    fn new(d: &'a Device, id: u32) -> Result<Self> {
        let mut p = ptr::null_mut();
        // SAFETY: live owned buffer, output pointer remains valid until the guard unmaps it.
        check(unsafe { (d.api.vaMapBuffer)(d.display, id, &mut p) }, "vaMapBuffer")?;
        let guard = Self { d, id, ptr: p };
        if p.is_null() {
            return Err(error("driver returned a null buffer mapping"));
        }
        Ok(guard)
    }
}
impl Drop for Mapping<'_> {
    fn drop(&mut self) {
        // SAFETY: successful map owned exclusively by this guard; no derived slices escape.
        unsafe {
            (self.d.api.vaUnmapBuffer)(self.d.display, self.id);
        }
    }
}
impl VideoEncoder for Encoder {
    fn sample_entry(&self) -> filmcraft_isobmff::SampleEntry {
        self.stream.entry.clone()
    }
    fn timescale(&self) -> u32 {
        self.rate.num as u32
    }
    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        Ok(Vec::new())
    }
    fn encode(&mut self, f: &EncoderFrame) -> Result<Vec<EncodedPacket>> {
        if f.width != self.w || f.height != self.h || f.hdr.is_some() {
            return Err(error("input dimensions/color differ from the initialized SDR encoder"));
        }
        input::convert(f, &mut self.nv12)?;
        input::upload(self)?;
        self.clear_buffers();
        let key = self.index.is_multiple_of(self.keyint());
        codecs::parameters(self, key)?;
        let d = &self.device;
        // SAFETY: resources belong to this display/context; all parameter buffers remain live
        // through submission and synchronization, and the input surface has been fully uploaded.
        unsafe {
            check((d.api.vaBeginPicture)(d.display, self.context, self.surfaces[0]), "vaBeginPicture")?;
            let render = (d.api.vaRenderPicture)(d.display, self.context, self.buffers.as_mut_ptr(), self.buffers.len() as i32);
            let end = (d.api.vaEndPicture)(d.display, self.context);
            check(render, "vaRenderPicture")?;
            check(end, "vaEndPicture")?;
            check((d.api.vaSyncSurface)(d.display, self.surfaces[0]), "vaSyncSurface")?;
        }
        let bytes = {
            let map = Mapping::new(d, self.coded)?;
            let mut next = map.ptr.cast::<VACodedBufferSegment>();
            let mut bytes = Vec::new();
            for _ in 0..1024 {
                if next.is_null() {
                    break;
                }
                // SAFETY: libva maps a linked list of VACodedBufferSegment, valid until unmap.
                let seg = unsafe { &*next };
                if seg.status & (VA_CODED_BUF_STATUS_SLICE_OVERFLOW_MASK | VA_CODED_BUF_STATUS_BAD_BITSTREAM) != 0 || seg.bit_offset != 0 {
                    return Err(error("coded buffer overflow, bad bitstream or unsupported bit offset"));
                }
                let len = seg.size as usize;
                if len > 256 * 1024 * 1024 || bytes.len().saturating_add(len) > 256 * 1024 * 1024 || (len != 0 && seg.buf.is_null()) {
                    return Err(error("invalid coded segment size/pointer"));
                }
                if len != 0 {
                    // SAFETY: driver owns seg.size initialized bytes for this mapping's lifetime.
                    bytes.extend_from_slice(unsafe { std::slice::from_raw_parts(seg.buf.cast::<u8>(), len) });
                }
                next = seg.next.cast();
            }
            if !next.is_null() {
                return Err(error("too many coded segments"));
            }
            bytes
        };
        let data = self.stream.packet(&bytes, key)?;
        self.index = self.index.checked_add(1).ok_or_else(|| error("frame counter overflow"))?;
        self.clear_buffers();
        Ok(vec![EncodedPacket { data, key, duration: self.rate.den as u32, composition_offset: 0 }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_mapping_is_encode_format_specific() {
        assert_eq!(profile_codec(VAProfile_VAProfileH264High), Some((Format::H264, Some(H264Profile::High))));
        assert_eq!(profile_codec(VAProfile_VAProfileHEVCMain), Some((Format::Hevc, None)));
        assert_eq!(profile_codec(VAProfile_VAProfileAV1Profile0), Some((Format::Av1, None)));
        assert_eq!(profile_codec(VAProfile_VAProfileHEVCMain10), None);
        assert_eq!(profile_codec(-123), None);
    }
}
