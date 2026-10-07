//! Safe, reusable NV12 staging. A future DMA-BUF import replaces upload, not codec submission.
use super::*;
pub(super) fn validate(w: u32, h: u32) -> Result<()> {
    if !(16..=8192).contains(&w) || !(16..=8192).contains(&h) || !w.is_multiple_of(2) || !h.is_multiple_of(2) {
        return Err(error("NV12 needs even dimensions between 16 and 8192"));
    }
    Ok(())
}
pub(super) fn convert(f: &EncoderFrame, out: &mut [u8]) -> Result<()> {
    validate(f.width, f.height)?;
    let (w, h) = (f.width as usize, f.height as usize);
    let pixels = w * h; // dimensions bounded above before all indexing/allocation
    if f.rgba.len() != pixels * 4 || out.len() != pixels * 3 / 2 {
        return Err(error("invalid RGBA/NV12 buffer length"));
    }
    let (y, uv) = out.split_at_mut(pixels);
    for row in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let (mut u, mut v) = (0.0f32, 0.0f32);
            for dy in 0..2 {
                for dx in 0..2 {
                    let at = (row + dy) * w + x + dx;
                    let rgb = &f.rgba[at * 4..at * 4 + 3];
                    let (r, g, b) = (rgb[0] as f32 / 255.0, rgb[1] as f32 / 255.0, rgb[2] as f32 / 255.0);
                    let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                    y[at] = (16.0 + 219.0 * l).round().clamp(16.0, 235.0) as u8;
                    u += (b - l) / 1.8556;
                    v += (r - l) / 1.5748;
                }
            }
            uv[row / 2 * w + x] = (128.0 + 56.0 * u).round().clamp(16.0, 240.0) as u8;
            uv[row / 2 * w + x + 1] = (128.0 + 56.0 * v).round().clamp(16.0, 240.0) as u8;
        }
    }
    Ok(())
}
pub(super) fn upload(e: &Encoder) -> Result<()> {
    let image = &e.image;
    if image.format.fourcc != VA_FOURCC_NV12 || image.num_planes != 2 || image.data_size > 512 * 1024 * 1024 {
        return Err(error("driver did not provide a bounded NV12 image"));
    }
    let map = Mapping::new(&e.device, image.buf)?;
    // SAFETY: image.data_size is the driver's mapped image allocation; map guard owns the mapping.
    let data = unsafe { std::slice::from_raw_parts_mut(map.ptr.cast::<u8>(), image.data_size as usize) };
    data.fill(0);
    for plane in 0..2 {
        let rows = e.h as usize / if plane == 0 { 1 } else { 2 };
        let pitch = image.pitches[plane] as usize;
        if pitch < e.w as usize {
            return Err(error("NV12 pitch shorter than width"));
        }
        for row in 0..rows {
            let start = (image.offsets[plane] as usize)
                .checked_add(row.checked_mul(pitch).ok_or_else(|| error("NV12 pitch overflow"))?)
                .ok_or_else(|| error("NV12 offset overflow"))?;
            let end = start.checked_add(e.w as usize).ok_or_else(|| error("NV12 row overflow"))?;
            let dst = data.get_mut(start..end).ok_or_else(|| error("NV12 row outside image"))?;
            let src = if plane == 0 { 0 } else { (e.w * e.h) as usize } + row * e.w as usize;
            dst.copy_from_slice(&e.nv12[src..src + e.w as usize]);
        }
    }
    drop(map);
    // SAFETY: unmapped staging image and owned input surface; copy rectangle within both extents.
    check(unsafe { (e.device.api.vaPutImage)(e.device.display, e.surfaces[0], image.image_id, 0, 0, e.w, e.h, 0, 0, e.w, e.h) }, "vaPutImage")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_sizes_and_rec709_levels() {
        for (w, h) in [(0, 0), (17, 16), (16, 17), (u32::MAX, u32::MAX)] {
            assert!(validate(w, h).is_err());
        }
        let mut rgba = vec![0; 16 * 16 * 4];
        let mut nv12 = vec![0; 16 * 16 * 3 / 2];
        fn frame(rgba: &[u8]) -> EncoderFrame<'_> {
            EncoderFrame { width: 16, height: 16, rgba, hdr: None, index: 0 }
        }
        convert(&frame(&rgba), &mut nv12).unwrap();
        assert!(nv12[..256].iter().all(|v| *v == 16));
        assert!(nv12[256..].iter().all(|v| *v == 128));
        rgba.fill(255);
        convert(&frame(&rgba), &mut nv12).unwrap();
        assert!(nv12[..256].iter().all(|v| *v == 235));
        assert!(convert(&frame(&rgba[..100]), &mut nv12).is_err());
        assert!(convert(&frame(&rgba), &mut nv12[..10]).is_err());
        for p in rgba.as_chunks_mut::<4>().0 {
            p.copy_from_slice(&[255, 0, 0, 255]);
        }
        convert(&frame(&rgba), &mut nv12).unwrap();
        assert_eq!((nv12[0], nv12[256], nv12[257]), (63, 102, 240));
    }
}
