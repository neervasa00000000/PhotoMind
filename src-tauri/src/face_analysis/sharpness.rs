use image::GrayImage;

/// Laplacian variance sharpness on a grayscale region, normalized 0–1 like global metrics.
pub fn region_sharpness(gray: &GrayImage, x: u32, y: u32, w: u32, h: u32) -> Option<f32> {
    let (img_w, img_h) = gray.dimensions();
    if w < 8 || h < 8 {
        return None;
    }
    let x = x.min(img_w.saturating_sub(1));
    let y = y.min(img_h.saturating_sub(1));
    let w = w.min(img_w - x);
    let h = h.min(img_h - y);
    if w < 8 || h < 8 {
        return None;
    }

    let mut laplacian_sum = 0.0f64;
    let mut laplacian_sq_sum = 0.0f64;
    let mut count = 0.0f64;
    for py in (y + 1)..(y + h - 1) {
        for px in (x + 1)..(x + w - 1) {
            let v = gray.get_pixel(px, py)[0] as f64 * -4.0
                + gray.get_pixel(px - 1, py)[0] as f64
                + gray.get_pixel(px + 1, py)[0] as f64
                + gray.get_pixel(px, py - 1)[0] as f64
                + gray.get_pixel(px, py + 1)[0] as f64;
            laplacian_sum += v;
            laplacian_sq_sum += v * v;
            count += 1.0;
        }
    }
    if count < 1.0 {
        return None;
    }
    let mean = laplacian_sum / count;
    let var = (laplacian_sq_sum / count) - (mean * mean);
    Some((var / 1000.0).clamp(0.0, 1.0) as f32)
}
