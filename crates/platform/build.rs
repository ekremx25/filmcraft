fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=src/vaapi/api.h");
    if std::env::var("CARGO_CFG_TARGET_OS")? != "linux" {
        return Ok(());
    }
    {
        let bindings = bindgen::Builder::default()
            .header("src/vaapi/api.h")
            .allowlist_type("VAEncSequenceParameterBuffer(H264|HEVC|AV1)|VAEncPictureParameterBuffer(H264|HEVC|AV1)|VAEncSliceParameterBuffer(H264|HEVC)|VAEncTileGroupBufferAV1|VAEncMiscParameter(RateControl|FrameRate)|VACodedBufferSegment|VAEncMiscParameterType|VAEncPackedHeaderParameterBuffer")
            .allowlist_var("VA.*|H264_.*|HEVC_.*")
            .allowlist_function("va(Initialize|Terminate|QueryVendorString|MaxNumProfiles|MaxNumEntrypoints|QueryConfigProfiles|QueryConfigEntrypoints|GetConfigAttributes|CreateConfig|DestroyConfig|CreateSurfaces|DestroySurfaces|CreateContext|DestroyContext|CreateBuffer|DestroyBuffer|MapBuffer|UnmapBuffer|CreateImage|DestroyImage|PutImage|BeginPicture|RenderPicture|EndPicture|SyncSurface|ErrorStr)")
            .dynamic_library_name("LibVa")
            .dynamic_link_require_all(true)
            .derive_default(true)
            .wrap_unsafe_ops(true)
            .generate_comments(false)
            .layout_tests(false)
            .generate()?;
        bindings.write_to_file(std::path::PathBuf::from(std::env::var("OUT_DIR")?).join("va.rs"))?;
    }
    Ok(())
}
