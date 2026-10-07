//! Hardware smoke test / encoder-only benchmark. No external encoder is invoked.
#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use filmcraft_export::*;
    use filmcraft_isobmff::*;
    filmcraft_platform::register();
    let args: Vec<String> = std::env::args().collect();
    let format = Format::from_name(args.get(1).map(String::as_str).unwrap_or("h264")).ok_or("unknown codec")?;
    let w: u32 = args.get(2).map(String::as_str).unwrap_or("1920").parse()?;
    let h: u32 = args.get(3).map(String::as_str).unwrap_or("1080").parse()?;
    let count: u64 = args.get(4).map(String::as_str).unwrap_or("150").parse()?;
    if !(16..=8192).contains(&w) || !(16..=8192).contains(&h) || count == 0 || count > 100000 {
        return Err("invalid size/frame count".into());
    }
    let mode = if args.get(5).is_some_and(|s| s == "software") { VideoEncoding::Software } else { VideoEncoding::Hardware };
    let settings = ExportSettings { format, video_encoding: mode, bitrate_mode: BitrateMode::Vbr1Pass, keyframe_distance: Some(60), ..Default::default() };
    eprintln!("Capabilities: {:?}", hardware::capabilities());
    let mut enc = hardware::make_encoder(&settings, w, h, filmcraft_time::FrameRate::FPS_30)?;
    let path = format!("target/vaapi-{}-{}.mp4", format.id(), if mode == VideoEncoding::Software { "software" } else { "hardware" });
    let mut rgba = vec![0; (w * h * 4) as usize];
    let mut mux = None;
    let mut track = 0;
    let start = std::time::Instant::now();
    for index in 0..count {
        for y in 0..h {
            for x in 0..w {
                let p = ((y * w + x) * 4) as usize;
                rgba[p..p + 4].copy_from_slice(&[((x + index as u32 * 3) % 256) as u8, ((y + index as u32) % 256) as u8, 100, 255]);
            }
        }
        let packets = enc.encode(&EncoderFrame { width: w, height: h, rgba: &rgba, hdr: None, index })?;
        if mux.is_none() {
            let mut m = Mp4Writer::new(std::io::BufWriter::new(std::fs::File::create(&path)?), WriterOptions::new(Brand::Mp4))?;
            let mut config = TrackConfig::new(enc.sample_entry(), enc.timescale());
            config.media_start = enc.media_start();
            track = m.add_track(config)?;
            mux = Some(m);
        }
        let m = mux.as_mut().ok_or("muxer missing")?;
        for p in packets {
            m.write_sample(track, WriteSample { data: &p.data, duration: p.duration, composition_offset: p.composition_offset, is_sync: p.key })?;
        }
    }
    let mut m = mux.ok_or("no frames encoded")?;
    for p in enc.flush()? {
        m.write_sample(track, WriteSample { data: &p.data, duration: p.duration, composition_offset: p.composition_offset, is_sync: p.key })?;
    }
    m.finish()?;
    eprintln!("{path}: {} frames in {:.3}s ({:.1} fps)", count, start.elapsed().as_secs_f64(), count as f64 / start.elapsed().as_secs_f64());
    Ok(())
}
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("VAAPI is Linux-only");
}
