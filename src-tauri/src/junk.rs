//! Junk detector: rule-based, cheap signals that flag content most people
//! would delete rather than keep — blown shots, screenshots, documents and
//! whiteboards. It never deletes anything; it only labels high-confidence
//! cases so a later layer can act on or present them.

use crate::scanner::ImageMetrics;
use image::imageops::FilterType;

pub struct JunkHint {
    pub category: &'static str,
    pub confidence: f64,
    pub reason: &'static str,
}

/// Cheap color + flatness signals derived from a normalized working image.
pub fn compute_signals(img: &image::DynamicImage) -> (f64, f64) {
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    let total = (w * h).max(1) as f64;

    let mut sat_sum = 0.0;
    for p in rgb.pixels() {
        let max = p[0].max(p[1]).max(p[2]);
        let min = p[0].min(p[1]).min(p[2]);
        let sat = if max == 0 {
            0.0
        } else {
            (max - min) as f64 / max as f64
        };
        sat_sum += sat;
    }
    let mean_saturation = sat_sum / total;

    // Flatness: fraction of 16x16 blocks whose luma stays within a tight range.
    // Screenshots, documents and whiteboards are dominated by such panels,
    // whereas photos have texture everywhere.
    let luma = img.to_luma8();
    let block = 16u32;
    let bx = (w / block).max(1);
    let by = (h / block).max(1);
    let luma_cropped = image::imageops::resize(&luma, bx * block, by * block, FilterType::Nearest);
    let mut flat_blocks = 0f64;
    let mut total_blocks = 0f64;
    for gy in 0..by {
        for gx in 0..bx {
            let mut sum = 0u64;
            let mut sq = 0u64;
            for y in 0..block {
                for x in 0..block {
                    let v = luma_cropped.get_pixel(gx * block + x, gy * block + y)[0] as u64;
                    sum += v;
                    sq += v * v;
                }
            }
            let n = (block * block) as f64;
            let mean = sum as f64 / n;
            let var = (sq as f64 / n) - (mean * mean);
            if var.sqrt() < 14.0 {
                flat_blocks += 1.0;
            }
            total_blocks += 1.0;
        }
    }
    let flat_fraction = flat_blocks / total_blocks;

    (mean_saturation, flat_fraction)
}

/// Conservative, ordered rules. Returns at most one label per photo and only
/// when the evidence is unambiguous; uncertain cases are left unlabeled.
pub fn detect_junk(m: &ImageMetrics) -> Option<JunkHint> {
    if m.highlight_clipping >= 0.30 && m.contrast <= 0.25 {
        let confidence = (0.55 + m.highlight_clipping * 0.4).min(0.95);
        return Some(JunkHint {
            category: "overexposed",
            confidence,
            reason: "Blown highlights and almost no tonal range.",
        });
    }
    if m.shadow_clipping >= 0.35 && m.exposure <= 0.18 {
        let confidence = (0.55 + m.shadow_clipping * 0.4).min(0.95);
        return Some(JunkHint {
            category: "underexposed",
            confidence,
            reason: "Crushed shadows — too dark to keep detail.",
        });
    }
    if m.flat_fraction >= 0.55 && m.sharpness >= 0.5 && m.mean_saturation <= 0.10 {
        let confidence = (0.60 + m.flat_fraction * 0.25).min(0.9);
        return Some(JunkHint {
            category: "screenshot",
            confidence,
            reason: "Crisp edges over flat panels: a screenshot or screen capture.",
        });
    }
    if m.exposure >= 0.60
        && m.mean_saturation <= 0.12
        && m.flat_fraction >= 0.40
        && m.contrast <= 0.40
    {
        return Some(JunkHint {
            category: "document",
            confidence: 0.6,
            reason: "Light, low-color layout: a document or whiteboard photo.",
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(s: f64, e: f64, c: f64, h: f64, sh: f64, sat: f64, flat: f64) -> ImageMetrics {
        ImageMetrics {
            sharpness: s,
            exposure: e,
            contrast: c,
            highlight_clipping: h,
            shadow_clipping: sh,
            mean_saturation: sat,
            flat_fraction: flat,
        }
    }

    #[test]
    fn blown_out_photo_is_flagged() {
        let m = metrics(0.2, 0.95, 0.12, 0.40, 0.0, 0.05, 0.3);
        let hint = detect_junk(&m).unwrap();
        assert_eq!(hint.category, "overexposed");
        assert!(hint.confidence > 0.5);
    }

    #[test]
    fn crushed_shadows_are_flagged() {
        let m = metrics(0.2, 0.10, 0.2, 0.0, 0.45, 0.1, 0.3);
        let hint = detect_junk(&m).unwrap();
        assert_eq!(hint.category, "underexposed");
    }

    #[test]
    fn screenshot_is_flagged_over_document() {
        let m = metrics(0.8, 0.5, 0.5, 0.02, 0.01, 0.05, 0.7);
        let hint = detect_junk(&m).unwrap();
        assert_eq!(hint.category, "screenshot");
    }

    #[test]
    fn whiteboard_document_is_flagged() {
        let m = metrics(0.3, 0.75, 0.3, 0.01, 0.01, 0.06, 0.5);
        let hint = detect_junk(&m).unwrap();
        assert_eq!(hint.category, "document");
    }

    #[test]
    fn a_normal_photo_is_not_flagged() {
        let m = metrics(0.4, 0.45, 0.5, 0.05, 0.05, 0.35, 0.2);
        assert!(detect_junk(&m).is_none());
    }

    fn rgb(w: u32, h: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> image::RgbImage {
        image::RgbImage::from_fn(w, h, |x, y| image::Rgb(pixel(x, y)))
    }

    #[test]
    fn white_document_image_yields_high_flatness_and_low_saturation() {
        let img = image::DynamicImage::ImageRgb8(rgb(512, 512, |x, y| {
            let row = y % 64;
            if row < 4 && (x / 8) % 3 == 0 {
                [90, 90, 90]
            } else {
                [248, 248, 248]
            }
        }));
        let (sat, flat) = compute_signals(&img);
        assert!(sat < 0.05, "saturation too high: {sat}");
        assert!(flat > 0.55, "flatness too low: {flat}");
        let m = metrics(0.3, 0.75, 0.3, 0.01, 0.01, sat, flat);
        assert_eq!(detect_junk(&m).unwrap().category, "document");
    }

    #[test]
    fn screenshot_image_is_flat_crisp_and_colorless() {
        let img = image::DynamicImage::ImageRgb8(rgb(512, 512, |x, y| {
            let panel = match ((x / 128).min(1), (y / 128).min(1)) {
                (0, 0) => 220,
                (1, 0) => 128,
                (0, 1) => 96,
                _ => 210,
            };
            // thin crisp separators between panels, like a UI
            if x % 128 == 0 || y % 128 == 0 {
                [40, 40, 40]
            } else {
                [panel, panel, panel]
            }
        }));
        let (sat, flat) = compute_signals(&img);
        assert!(sat < 0.03, "saturation too high: {sat}");
        assert!(flat > 0.5, "flatness too low: {flat}");
        let m = metrics(0.9, 0.5, 0.5, 0.02, 0.01, sat, flat);
        assert_eq!(detect_junk(&m).unwrap().category, "screenshot");
    }
}
