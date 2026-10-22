//! Small, bundled SFace model for offline face descriptors.
use image::RgbImage;
use ort::{session::Session, value::Tensor};
use std::{
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

const SIDE: usize = 112;
const MIN_FACE_PIXELS: f32 = 24.0;
const TEMPLATE: [(f32, f32); 5] = [
    (38.2946, 51.6963),
    (73.5318, 51.5014),
    (56.0252, 71.7366),
    (41.5493, 92.3655),
    (70.7299, 92.2041),
];
static SESSION: OnceLock<Mutex<Option<Session>>> = OnceLock::new();

pub fn model_path() -> PathBuf {
    if let Ok(dir) = std::env::var("PHOTOMIND_RESOURCE_DIR") {
        let path = PathBuf::from(dir).join("models/face_recognition_sface_2021dec_int8.onnx");
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("models/face_recognition_sface_2021dec_int8.onnx")
}

fn aligned_tensor(image: &RgbImage, points: &[(f32, f32); 5]) -> Option<Tensor<f32>> {
    let src_mean = points
        .iter()
        .fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
    let dst_mean = TEMPLATE
        .iter()
        .fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
    let src_mean = (src_mean.0 / 5.0, src_mean.1 / 5.0);
    let dst_mean = (dst_mean.0 / 5.0, dst_mean.1 / 5.0);
    let (mut dot, mut cross, mut norm) = (0.0, 0.0, 0.0);
    for (src, dst) in points.iter().zip(TEMPLATE) {
        let (sx, sy) = (src.0 - src_mean.0, src.1 - src_mean.1);
        let (dx, dy) = (dst.0 - dst_mean.0, dst.1 - dst_mean.1);
        dot += sx * dx + sy * dy;
        cross += sx * dy - sy * dx;
        norm += sx * sx + sy * sy;
    }
    if norm < 1.0 || !norm.is_finite() {
        return None;
    }
    let (a, b) = (dot / norm, cross / norm);
    let scale_sq = a * a + b * b;
    if scale_sq < 1e-8 || !scale_sq.is_finite() {
        return None;
    }
    let tx = dst_mean.0 - a * src_mean.0 + b * src_mean.1;
    let ty = dst_mean.1 - b * src_mean.0 - a * src_mean.1;
    let mut data = vec![0.0; 3 * SIDE * SIDE];
    let (width, height) = image.dimensions();
    for y in 0..SIDE {
        for x in 0..SIDE {
            let dx = x as f32 - tx;
            let dy = y as f32 - ty;
            let sx = (a * dx + b * dy) / scale_sq;
            let sy = (-b * dx + a * dy) / scale_sq;
            if sx < 0.0 || sy < 0.0 || sx >= (width - 1) as f32 || sy >= (height - 1) as f32 {
                continue;
            }
            let (x0, y0) = (sx.floor() as u32, sy.floor() as u32);
            let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
            let p00 = image.get_pixel(x0, y0);
            let p10 = image.get_pixel(x0 + 1, y0);
            let p01 = image.get_pixel(x0, y0 + 1);
            let p11 = image.get_pixel(x0 + 1, y0 + 1);
            for c in 0..3 {
                data[c * SIDE * SIDE + y * SIDE + x] = (1.0 - fx) * (1.0 - fy) * p00[c] as f32
                    + fx * (1.0 - fy) * p10[c] as f32
                    + (1.0 - fx) * fy * p01[c] as f32
                    + fx * fy * p11[c] as f32;
            }
        }
    }
    Tensor::from_array((
        [1_i64, 3, SIDE as i64, SIDE as i64],
        data.into_boxed_slice(),
    ))
    .ok()
}

pub fn descriptor(
    image: &RgbImage,
    face: &super::types::DetectedFace,
) -> Result<Option<Vec<f32>>, String> {
    if face.bbox.width.min(face.bbox.height) < MIN_FACE_PIXELS {
        return Ok(None);
    }
    let Some(points) = face.landmarks.as_ref() else {
        return Ok(None);
    };
    let Some(input) = aligned_tensor(image, points) else {
        return Ok(None);
    };
    let lock = SESSION.get_or_init(|| Mutex::new(None));
    let mut guard = lock
        .lock()
        .map_err(|_| "Face model lock poisoned".to_string())?;
    if guard.is_none() {
        let path = model_path();
        if !path.is_file() {
            return Err(format!(
                "Bundled face recognition model missing: {}",
                path.display()
            ));
        }
        *guard = Some(
            Session::builder()
                .map_err(|e| e.to_string())?
                .commit_from_file(path)
                .map_err(|e| e.to_string())?,
        );
    }
    let output = guard
        .as_mut()
        .unwrap()
        .run(ort::inputs![input])
        .map_err(|e| e.to_string())?;
    let (_, values) = output[0]
        .try_extract_tensor::<f32>()
        .map_err(|e| e.to_string())?;
    let mut feature = values.to_vec();
    if feature.len() != 128 || feature.iter().any(|v| !v.is_finite()) {
        return Err("Face model returned an invalid descriptor".into());
    }
    let norm = feature.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm < 1e-8 {
        return Ok(None);
    }
    for value in &mut feature {
        *value /= norm;
    }
    Ok(Some(feature))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aligned_landmarks_map_to_the_same_patch() {
        let image = RgbImage::from_fn(112, 112, |x, y| image::Rgb([x as u8, y as u8, 0]));
        let tensor = aligned_tensor(&image, &TEMPLATE).unwrap();
        let (_, values) = tensor.try_extract_tensor::<f32>().unwrap();
        let index = 60 * SIDE + 50;
        assert!((values[index] - 50.0).abs() < 0.01);
        assert!((values[SIDE * SIDE + index] - 60.0).abs() < 0.01);
    }

    #[test]
    fn bundled_model_runs_without_an_external_service() {
        let image = RgbImage::from_fn(112, 112, |x, y| {
            image::Rgb([(x * 2) as u8, (y * 2) as u8, ((x + y) % 256) as u8])
        });
        let face = super::super::types::DetectedFace {
            bbox: super::super::types::BoundingBox { x: 0.0, y: 0.0, width: 112.0, height: 112.0 },
            landmarks: Some(TEMPLATE),
            detection_confidence: 1.0,
            face_sharpness: None,
            left_eye: None,
            right_eye: None,
            expression: None,
        };
        let vector = descriptor(&image, &face).unwrap().unwrap();
        assert_eq!(vector.len(), 128);
        assert!((vector.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-4);
    }
}
