# Linux VAAPI hardware export

FilmCraft can encode H.264 (Constrained Baseline/Main/High), HEVC Main and AV1 Main through
Linux libva. Codec availability comes from DRM render-node profiles and encode entrypoints;
the implementation is not tied to AMD. Validated on an AMD Radeon RX 9070 XT, Mesa/radeonsi.
Intel drivers exposing the required profiles, rate control and packed headers are candidates,
but have not been validated by this change.

## Build and run

Runtime: libva (`libva.so.2`, `libva-drm.so.2`), a VAAPI-capable driver (Mesa/radeonsi on AMD),
and permission to open `/dev/dri/renderD*`. The libraries load dynamically: if absent, Auto
still uses FilmCraft's software encoder. No FFmpeg, vainfo or encoder subprocess is required.

Linux build dependencies also include libva development headers (with the AV1 encode API)
and libclang for bindgen. On Arch/CachyOS: `libva`, `clang`, `mesa`; on Debian/Ubuntu:
`libva-dev`, `libclang-dev` and the appropriate runtime driver. Binding generation is skipped
for non-Linux targets. No Linux imports or library loads are compiled on macOS/Windows/web.

```sh
cd /home/ekrem/filmcraft
CARGO_BUILD_JOBS=1 FILMCRAFT_LOG=info cargo run --release -p filmcraft
```

Open **Export → Format → H.264 / HEVC (H.265) / AV1**, then **Video → Encoding → Hardware (VAAPI)**.
For H.264 choose Baseline, Main or High. **Rate Control** offers driver-supported CBR, VBR and
Quality (constant QP). QP ranges are 1–51 for H.264/HEVC and 1–255 for AV1; lower means higher
quality. Constant QP is not CRF and does not constrain bitrate. Auto and Software retain the
existing software bitrate controls. Unsupported codecs are disabled in the format menu.

Settings are serialized as `videoEncoding: "auto" | "hardware" | "software"` and optional
`hardwareQp`. Old presets/projects default to Auto. Existing software codecs are unchanged.
The single build worker limits peak memory during release LTO on machines without swap.
Run build/check/test commands sequentially. To keep full-suite validation inexpensive, disable
LTO for its many test binaries (normal application builds retain the workspace's release LTO):

```sh
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=2 \
  CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 cargo xtask ci
```

- **Auto** tries hardware first, including encoding a disposable initial frame and checking
  its codec configuration. Initialization failure falls back to software before opening output.
- **Hardware** reports failure and never invokes a software encoder.
- **Software** bypasses every registered hardware factory.
- HEVC/AV1 currently have no FilmCraft software encoders. If hardware is unavailable, Auto
  explains this rather than changing codec. Software for these formats is disabled in the UI.
- Mid-stream errors terminate the export with an error. There is no unsafe mid-file codec switch.

## Architecture and limits

The existing `VideoEncoder`, `register_encoder`, `EncoderFrame`, stepped export pipeline and
MP4/MOV writer remain in use. Platform-neutral policy/capabilities live in `export::hardware`;
`platform::register()` registers the Linux factory, as it already does for macOS decoders.
All Linux FFI is isolated in `platform::vaapi`. There are no external encoding processes.

The first path is RGBA8 → CPU BT.709 limited-range NV12 → reusable libva staging image →
one input surface plus two alternating reconstructed surfaces → GPU encoder → existing muxer.
A reused coded buffer is synchronized and copied before another frame starts. Sequence,
picture and slice/OBU headers describe the submitted native parameters; image compression
is exclusively the hardware encoder's work. H.264 reuses FilmCraft's existing header writer.
H.264/HEVC Annex-B is converted to four-byte length-prefixed samples, with parameter sets in
`avcC`/`hvcC`. AV1 stores its sequence header in `av1C`, excludes temporal delimiters from
samples and uses low-overhead OBUs. DTS equals PTS; no B-frames/reordering.

Current hardware combinations:

- 8-bit, progressive SDR Rec.709, 4:2:0 NV12, MP4/MOV; even dimensions, maximum 8192 per axis
  (driver limits can be smaller). H.264 level is selected using the existing level logic.
- HEVC/AV1 use Main tier, level 5.1: width ≤4096, area ≤8,912,896 pixels, ≤60 fps,
  maximum bitrate ≤40 Mbps. HEVC additionally needs dimensions divisible by eight.
- The selected profile must support packed sequence/picture headers, and packed slices for
  H.264/HEVC. Drivers lacking these are not advertised by this backend.
- CBR/VBR are driver rate-control requests, not promises of exact bitrate on short/simple clips.
- HDR/P010/10-bit, interlacing, two-pass hardware and MXF hardware are unsupported. Auto can
  retain the existing software behavior where a software encoder exists. Hardware errors
  explain the unsupported combination; an HDR sequence must explicitly select SDR export.
- There is no zero-copy yet. Replace the `input::convert`/`input::upload` boundary with GPU
  NV12/P010 conversion and DMA-BUF import later; leave submission, packet handling and muxing intact.

## Diagnostics and tests

`FILMCRAFT_LOG=info` enables FilmCraft diagnostics on stderr, including codec, render node,
VA profile/entrypoint, driver vendor string, rate control and resolution. Auto fallback and
software selection are logged once at initialization; there are no per-frame logs.

In another terminal during export:

```sh
amdgpu_top
# Or collect timestamped samples as JSON:
amdgpu_top -J -u 1 -s 200 -n 50 > target/vaapi-gpu.json
```

Watch VCN/media encode activity and the FilmCraft process's video-engine time. Newer VCN
hardware may expose one combined VCN engine rather than separate decode/encode graphs.
For driver diagnostics only (optional external utility): `vainfo --display drm --device /dev/dri/renderD128`.
Its absence never prevents export. Check render-node access and the driver if capabilities are empty.

```sh
cargo test -p filmcraft-export hardware::tests
cargo test -p filmcraft-platform --lib
FILMCRAFT_REQUIRE_VAAPI=1 cargo test -p filmcraft-platform --test vaapi -- --nocapture
cargo test -p filmcraft-ui-egui --test export_ui

# Synthetic moving-gradient encoder benchmark, 150 frames of 1080p30:
cargo run --release -p filmcraft-platform --example vaapi_encode -- h264 1920 1080 150
cargo run --release -p filmcraft-platform --example vaapi_encode -- hevc 1920 1080 150
cargo run --release -p filmcraft-platform --example vaapi_encode -- av1 1920 1080 150
cargo run --release -p filmcraft-platform --example vaapi_encode -- h264 1920 1080 150 software

# Development oracles only:
ffprobe -v error -count_frames -show_streams target/vaapi-h264-hardware.mp4
ffmpeg -v error -i target/vaapi-h264-hardware.mp4 -f null -
```

The hardware integration test renders a real FilmCraft sequence for five seconds at 1920×1080,
30 fps, with a cut inside a GOP. For each advertised codec it checks 150 decoded frames, no
FFmpeg errors, and first/last colors. It also exercises CBR/CQP, software bypass and HDR Auto
fallback. With no device it skips; `FILMCRAFT_REQUIRE_VAAPI=1` makes missing hardware a failure.
Missing optional oracle executables are reported by testkit; native encoding still runs.

## Validation and measured performance (2026-10-07)

On RX 9070 XT / Ryzen 9 7900X (24 logical CPUs), CachyOS 7.2.9, Mesa 26.2.4 and libva 2.24.1:

- The full `cargo xtask ci` command above passed: 1,929 tests, zero failures and zero filtered
  tests; 39 existing optional tests remained ignored. Formatting, Clippy, layers, assets and
  all 42 WASM crates passed. The Windows GNU target check of `filmcraft-platform` also passed.
- The normal release desktop build (with workspace LTO) passed. Actual desktop UI exports of
  H.264/MP4, HEVC/MOV and AV1/MP4 with AAC decoded without errors. Baseline/Main H.264 were
  additionally checked; the five-second integration files cover High, HEVC Main and AV1 Main,
  1080p30, CBR/VBR/CQP, BT.709 limited range, and keyframes at 0/60/120.
- The CI run exposed two existing portability issues, also fixed: a shortcut test assumed an
  empty macOS conflict list on Linux, and interpolated source coordinates perturbed texel centers
  in the GPU effect stage. A new 1:1 source-copy regression test reproduces the latter before the
  fix; all 21 GPU tests now pass without relaxing their numerical tolerances.

Single-run moving-gradient benchmark, 150 frames, 1920×1080 at 30 fps, High profile,
VBR target 20 Mbps / maximum 30 Mbps, key interval 60, normal release build:

| Encoder | FPS (generation + conversion + encode + mux) | Process CPU usage | CPU time |
|---|---:|---:|---:|
| Software H.264 | 81.5 | 1402.9% | 26.09 s |
| VAAPI H.264 | 63.1 | 77.5% | 1.87 s |

100% CPU means one logical CPU, not the whole machine. CPU time includes initialization;
FPS excludes it. This first synchronous CPU-conversion implementation substantially reduces CPU
work but is slower than the parallel software encoder on this synthetic input. These numbers
are not a prediction for every timeline or an equal-perceptual-quality comparison.

A separate 1,500-frame hardware run captured 80 `amdgpu_top` samples. The encoder process's
`VCN_Unified` reached 24% (`Media` 12%); this confirms hardware compression independently of
the graphics/compositor engine. Both benchmark files also decoded without errors.

## Dependencies and clean-room provenance

- `bindgen` 0.72.1: BSD-3-Clause, build-time only (MSRV 1.70).
- `libloading` 0.8.9: ISC, already present in the workspace lockfile (MSRV 1.71).
- System libva: MIT; native public API declarations only. No GPL/LGPL encoder libraries.
- Internal FilmCraft bitstream/H.264/HEVC/AV1 parsers and H.264 header writer: MIT OR Apache-2.0.

Implementation references: installed libva 1.24 public headers, ITU-T H.264 (04/2013),
ITU-T H.265 (08/2021) §7.3, AV1 bitstream specification v1.0.0 §5 and AV1 ISOBMFF binding v1.3.0.
No third-party encoder/decoder implementation source was used. FFmpeg is only a test oracle.
