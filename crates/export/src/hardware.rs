//! Platform-neutral encoder policy. OS backends register capabilities and factories at startup.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VideoEncoding {
    #[default]
    Auto,
    Hardware,
    Software,
}
impl VideoEncoding {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Hardware => "Hardware (VAAPI)",
            Self::Software => "Software",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Capability {
    pub format: Format,
    pub device: String,
    pub profile: Option<H264Profile>,
    pub cbr: bool,
    pub vbr: bool,
    pub cqp: bool,
}

fn registry() -> &'static RwLock<Vec<(EncoderFactory, Vec<Capability>)>> {
    static REGISTRY: OnceLock<RwLock<Vec<(EncoderFactory, Vec<Capability>)>>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

/// Register once per backend; repeated initialization replaces its capability snapshot.
/// Uses the existing encoder registry, with explicit classification for required-hardware policy.
pub fn register(factory: EncoderFactory, capabilities: Vec<Capability>) {
    let mut r = registry().write().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = r.iter_mut().find(|(f, _)| std::ptr::fn_addr_eq(*f, factory)) {
        entry.1 = capabilities;
    } else {
        r.push((factory, capabilities));
    }
    drop(r);
    register_encoder(factory);
}

pub fn capabilities() -> Vec<Capability> {
    registry().read().unwrap_or_else(|e| e.into_inner()).iter().flat_map(|(_, c)| c.clone()).collect()
}

pub fn supports(s: &ExportSettings) -> bool {
    if s.format.is_mxf() || s.signal.is_hdr() || s.field_order != FieldOrder::Progressive || s.bitrate_mode == BitrateMode::Vbr2Pass {
        return false;
    }
    capabilities().iter().any(|c| {
        c.format == s.video_format()
            && (c.format != Format::H264 || c.profile == Some(s.h264_profile))
            && if s.hardware_qp.is_some() {
                c.cqp
            } else {
                match s.bitrate_mode {
                    BitrateMode::Cbr => c.cbr,
                    BitrateMode::Vbr1Pass => c.vbr,
                    BitrateMode::Vbr2Pass => false,
                }
            }
    })
}

/// Create through registered factories; Hardware can never reach a software factory.
pub fn make_encoder(s: &ExportSettings, w: u32, h: u32, rate: FrameRate) -> Result<Box<dyn VideoEncoder>> {
    let factories = video_factories().read().unwrap_or_else(|e| e.into_inner()).clone();
    let hardware: Vec<_> = registry().read().unwrap_or_else(|e| e.into_inner()).iter().map(|(f, _)| *f).collect();
    select(&factories, &hardware, s, w, h, rate)
}

fn select(factories: &[EncoderFactory], hardware: &[EncoderFactory], s: &ExportSettings, w: u32, h: u32, rate: FrameRate) -> Result<Box<dyn VideoEncoder>> {
    let mut reasons = Vec::new();
    if s.video_encoding != VideoEncoding::Software {
        for f in hardware {
            if let Some(result) = f(s.video_format(), w, h, rate, s) {
                match result {
                    Ok(enc) => return Ok(enc),
                    Err(e) => {
                        log::info!("Hardware export initialization declined: {e}");
                        reasons.push(e.to_string());
                    }
                }
            }
        }
        if s.video_encoding == VideoEncoding::Hardware {
            return Err(ExportError::Unsupported(format!(
                "Hardware Encoding was requested for {} but no compatible VAAPI encoder could be initialized. {}",
                s.video_format().label(),
                reasons.join("; ")
            )));
        }
    }
    for f in factories {
        if !hardware.iter().any(|hw| std::ptr::fn_addr_eq(*hw, *f))
            && let Some(result) = f(s.video_format(), w, h, rate, s)
        {
            log::info!("FilmCraft export encoder: Software {}", s.video_format().label());
            return result;
        }
    }
    Err(ExportError::Unsupported(format!(
        "No software {} encoder is installed; this format requires compatible hardware. {}",
        s.video_format().label(),
        reasons.join("; ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn failed(_: Format, _: u32, _: u32, _: FrameRate, _: &ExportSettings) -> Option<Result<Box<dyn VideoEncoder>>> {
        Some(Err(ExportError::Encode("test driver unavailable".into())))
    }
    #[test]
    fn selection_and_legacy_default() {
        let mut s: ExportSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.video_encoding, VideoEncoding::Auto);
        let factories: [EncoderFactory; 2] = [failed, h264_factory];
        let hw: [EncoderFactory; 1] = [failed];
        assert!(select(&factories, &hw, &s, 32, 32, FrameRate::FPS_30).is_ok());
        s.video_encoding = VideoEncoding::Hardware;
        assert!(select(&factories, &hw, &s, 32, 32, FrameRate::FPS_30).err().unwrap().to_string().contains("test driver unavailable"));
        assert!(select(&factories, &[], &s, 32, 32, FrameRate::FPS_30).is_err());
        s.video_encoding = VideoEncoding::Software;
        assert!(select(&factories, &hw, &s, 32, 32, FrameRate::FPS_30).is_ok());
        s.format = Format::Hevc;
        assert!(select(&factories, &hw, &s, 32, 32, FrameRate::FPS_30).is_err());
        assert_eq!(serde_json::from_str::<ExportSettings>(&serde_json::to_string(&s).unwrap()).unwrap().video_encoding, VideoEncoding::Software);
    }
}
