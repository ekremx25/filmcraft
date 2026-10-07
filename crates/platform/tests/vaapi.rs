//! Linux hardware integration. No driver: skip. Advertised hardware that fails: fail the test.
//! FFmpeg/ffprobe are external decode/inspection oracles only, never production dependencies.
#![cfg(target_os = "linux")]
use filmcraft_export::*;
use filmcraft_media::{Generator, MediaSource, generators::GeneratorSource};
use filmcraft_project::{ItemId, ItemKind, Label, MediaClip, MediaRef, Project, SequenceSettings, TrackKind};
use filmcraft_render::SourceMap;
use filmcraft_time::{FrameRate, Tick, TimeRange};
use std::{process::Command, sync::Arc};

fn project() -> (Arc<Project>, ItemId, SourceMap) {
    let mut p = Project::new("VAAPI integration");
    let rate = FrameRate::FPS_30;
    let seq = p.new_sequence("1080p", SequenceSettings { width: 1920, height: 1080, frame_rate: rate, ..Default::default() }, 1, 0, None);
    let mut map = SourceMap::default();
    // A cut at frame 75 (inside a GOP) exercises inter prediction and reference replacement.
    for (at, color) in [(0, [0.8, 0.1, 0.05, 1.0]), (75, [0.05, 0.2, 0.8, 1.0])] {
        let source = GeneratorSource::new(Generator::ColorMatte { color }, 1920, 1080, rate, rate.tick_of(75));
        let id = p.add_item(
            "matte",
            Label::Iris,
            ItemKind::Media(MediaClip {
                media: MediaRef::Generator(source.generator.clone()),
                info: source.info().clone(),
                interpret: Default::default(),
                mark_in: None,
                mark_out: None,
                markers: vec![],
                offline: false,
                proxy: None,
                identity: None,
            }),
            None,
        );
        let item = p.make_track_item(id, TrackKind::Video, rate.tick_of(at), TimeRange::new(Tick::ZERO, rate.tick_of(75)), rate).unwrap();
        p.sequence_mut(seq).unwrap().video_tracks[0].items.push(item);
        map.0.insert(id, Arc::new(source));
    }
    (Arc::new(p), seq, map)
}
#[test]
fn timeline_1080p_all_hardware_codecs_decode() {
    filmcraft_platform::register();
    let caps = hardware::capabilities();
    if caps.is_empty() {
        assert!(std::env::var_os("FILMCRAFT_REQUIRE_VAAPI").is_none(), "VAAPI required but absent");
        eprintln!("SKIPPED: no compatible VAAPI render node");
        return;
    }
    let ffmpeg = filmcraft_testkit::ffmpeg_or_skip("VAAPI decoder oracle");
    let ffprobe = filmcraft_testkit::ffprobe_or_skip("VAAPI frame count");
    let (p, seq, sources) = project();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fixtures/vaapi");
    std::fs::create_dir_all(&dir).unwrap();
    for format in [Format::H264, Format::Hevc, Format::Av1] {
        if !caps.iter().any(|c| c.format == format) {
            eprintln!("SKIPPED: {} not advertised", format.label());
            continue;
        }
        for (mode, qp, frames) in [(BitrateMode::Vbr1Pass, None, 150), (BitrateMode::Cbr, None, 4), (BitrateMode::Vbr1Pass, Some(26), 4)] {
            let path = dir.join(format!("{}-{:?}-{:?}.mp4", format.id(), mode, qp));
            let s = ExportSettings {
                format,
                video_encoding: VideoEncoding::Hardware,
                hardware_qp: qp,
                bitrate_mode: mode,
                path: path.to_string_lossy().into_owned(),
                include_audio: false,
                range: Some(TimeRange::new(Tick::ZERO, FrameRate::FPS_30.tick_of(frames))),
                keyframe_distance: Some(60),
                ..Default::default()
            };
            if !hardware::supports(&s) {
                eprintln!("SKIPPED: {} {mode:?} QP={qp:?} not advertised", format.label());
                continue;
            }
            let report = export(&p, seq, &s, &sources, &Progress::default()).unwrap();
            assert_eq!(report.frames, frames as u64);
            eprintln!("{} {:?} QP={qp:?}: {:.1} timeline fps, {}", format.label(), mode, report.render_fps, path.display());
            if let Some(ff) = &ffmpeg {
                let output = Command::new(ff).args(["-v", "error", "-i"]).arg(&path).args(["-f", "null", "-"]).output().unwrap();
                assert!(output.status.success() && output.stderr.is_empty(), "{}: {}", path.display(), String::from_utf8_lossy(&output.stderr));
                // Decode first and last frames to prove the mid-GOP cut produces the right color.
                if frames == 150 {
                    for (index, dominant) in [(0, 0), (149, 2)] {
                        let filter = format!("select=eq(n\\,{index}),scale=1:1");
                        let pixels = Command::new(ff)
                            .args(["-v", "error", "-i"])
                            .arg(&path)
                            .args(["-vf", &filter, "-frames:v", "1", "-pix_fmt", "rgb24", "-f", "rawvideo", "-"])
                            .output()
                            .unwrap();
                        assert!(pixels.status.success() && pixels.stderr.is_empty());
                        assert_eq!(pixels.stdout.len(), 3);
                        assert!(pixels.stdout[dominant] > 180 && pixels.stdout[2 - dominant] < 100, "wrong color/order: {:?}", pixels.stdout);
                    }
                }
            }
            if let Some(probe) = &ffprobe {
                let output = Command::new(probe)
                    .args(["-v", "error", "-select_streams", "v:0", "-count_frames", "-show_entries", "stream=nb_read_frames", "-of", "csv=p=0"])
                    .arg(&path)
                    .output()
                    .unwrap();
                assert!(output.status.success() && output.stderr.is_empty());
                assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), frames.to_string());
            }
        }
    }
}

#[test]
fn unsupported_input_policy_and_software_bypass() {
    filmcraft_platform::register();
    let mut s = ExportSettings { signal: ColorSignal::PQ, video_encoding: VideoEncoding::Hardware, ..Default::default() };
    assert!(hardware::make_encoder(&s, 32, 32, FrameRate::FPS_30).is_err());
    s.video_encoding = VideoEncoding::Auto;
    assert!(hardware::make_encoder(&s, 32, 32, FrameRate::FPS_30).is_ok());
    s.video_encoding = VideoEncoding::Software;
    assert!(filmcraft_platform::vaapi::factory(Format::H264, 32, 32, FrameRate::FPS_30, &s).is_none());
    assert!(hardware::make_encoder(&s, 32, 32, FrameRate::FPS_30).is_ok());
}
