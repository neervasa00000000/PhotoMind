use super::sharpness::region_sharpness;
use super::types::{
    BoundingBox, DetectedFace, ExpressionAnalysis, EyeAnalysis, EyeState, FaceAnalysis,
    FaceAnalysisError, ANALYZER_VERSION,
};
use image::{GrayImage, RgbImage};
use ort::session::{Session, SessionOutputs};
use ort::value::Tensor;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

const INPUT_SIZE: u32 = 640;
const SCORE_THRESHOLD: f32 = 0.6;
const NMS_THRESHOLD: f32 = 0.3;
const EYE_OPEN_THRESHOLD: f32 = 0.52;
const EYE_CLOSED_THRESHOLD: f32 = 0.38;
const MIN_MEANINGFUL_FACE_PX: f32 = 24.0;

static SESSION: OnceLock<Mutex<Option<Session>>> = OnceLock::new();

fn with_session<F, R>(f: F) -> Result<R, FaceAnalysisError>
where
    F: FnOnce(&mut Session) -> Result<R, FaceAnalysisError>,
{
    let lock = SESSION.get_or_init(|| Mutex::new(None));
    let mut guard = lock
        .lock()
        .map_err(|_| FaceAnalysisError::InferenceFailed("Face model lock poisoned".into()))?;
    if guard.is_none() {
        let path = resolve_model_path();
        if !path.is_file() {
            return Err(FaceAnalysisError::ModelUnavailable(format!(
                "Missing YuNet model at {}",
                path.display()
            )));
        }
        let session = Session::builder()
            .map_err(|e| FaceAnalysisError::ModelUnavailable(e.to_string()))?
            .commit_from_file(&path)
            .map_err(|e| FaceAnalysisError::ModelUnavailable(e.to_string()))?;
        *guard = Some(session);
    }
    f(guard.as_mut().unwrap())
}

pub fn resolve_model_path() -> PathBuf {
    if let Ok(path) = std::env::var("PHOTOMIND_FACE_MODEL") {
        return PathBuf::from(path);
    }
    if let Ok(dir) = std::env::var("PHOTOMIND_RESOURCE_DIR") {
        let candidate = PathBuf::from(dir).join("models/face_detection_yunet_2023mar.onnx");
        if candidate.is_file() {
            return candidate;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models/face_detection_yunet_2023mar.onnx")
}

pub struct AnalysisImage {
    pub rgb: RgbImage,
    pub gray: GrayImage,
}

impl AnalysisImage {
    pub fn from_rgb(rgb: RgbImage) -> Self {
        let gray = image::DynamicImage::ImageRgb8(rgb.clone()).to_luma8();
        Self { rgb, gray }
    }
}

pub fn analyze_yunet(image: &AnalysisImage) -> Result<FaceAnalysis, FaceAnalysisError> {
    let detections = detections_for_image(image)?;
    let faces = detections
        .into_iter()
        .map(|d| enrich_face(&image.gray, d))
        .collect::<Vec<_>>();
    Ok(FaceAnalysis {
        face_count: faces.len(),
        faces,
        analyzer_version: ANALYZER_VERSION.into(),
    })
}

/// Face box and eye centers for the development labeler.
///
/// Eye centers are the subject's anatomical left/right (OpenCV YuNet order),
/// not camera-left/camera-right. This does not include OPEN/CLOSED predictions.
#[derive(Debug, Clone)]
pub struct FaceGeometry {
    pub bbox: BoundingBox,
    pub detection_confidence: f32,
    pub subject_left_eye: (f32, f32),
    pub subject_right_eye: (f32, f32),
}

pub fn detect_face_geometry(image: &AnalysisImage) -> Result<Vec<FaceGeometry>, FaceAnalysisError> {
    Ok(detections_for_image(image)?
        .into_iter()
        .map(|d| FaceGeometry {
            bbox: d.bbox,
            detection_confidence: d.score,
            subject_left_eye: d.left_eye,
            subject_right_eye: d.right_eye,
        })
        .collect())
}

/// Exact heuristic eye patch used by classify (radius = face_scale * 0.08, min 4px).
pub fn eye_patch_rect(
    center: (f32, f32),
    face_scale: f32,
    img_w: u32,
    img_h: u32,
) -> (u32, u32, u32, u32) {
    let radius = (face_scale * 0.08).max(4.0) as u32;
    let cx = center.0.max(0.0) as u32;
    let cy = center.1.max(0.0) as u32;
    let x = cx.saturating_sub(radius);
    let y = cy.saturating_sub(radius);
    let patch_w = (radius * 2).min(img_w.saturating_sub(x));
    let patch_h = (radius * 2).min(img_h.saturating_sub(y));
    (x, y, patch_w, patch_h)
}

fn detections_for_image(image: &AnalysisImage) -> Result<Vec<RawDetection>, FaceAnalysisError> {
    let (orig_w, orig_h) = image.rgb.dimensions();
    if orig_w < 32 || orig_h < 32 {
        return Ok(Vec::new());
    }

    let scale_x = orig_w as f32 / INPUT_SIZE as f32;
    let scale_y = orig_h as f32 / INPUT_SIZE as f32;
    let input = build_input_tensor(&image.rgb, INPUT_SIZE, INPUT_SIZE);

    with_session(|session| {
        let outputs = session
            .run(ort::inputs![input])
            .map_err(|e| FaceAnalysisError::InferenceFailed(e.to_string()))?;

        let detections = decode_yunet_outputs(&outputs, scale_x, scale_y)?;
        Ok(nms(detections, NMS_THRESHOLD)
            .into_iter()
            .filter(|d| d.score >= SCORE_THRESHOLD)
            .collect())
    })
}

// YuNet 2023mar exposes twelve heads (class, objectness, box and landmarks at
// strides 8, 16 and 32), not a final Nx15 detection tensor. OpenCV's
// FaceDetectorYN postprocessing provides the decoding equations used here.
fn decode_yunet_outputs(
    outputs: &SessionOutputs<'_>,
    scale_x: f32,
    scale_y: f32,
) -> Result<Vec<RawDetection>, FaceAnalysisError> {
    let mut detections = Vec::new();
    for stride in [8_usize, 16, 32] {
        let read = |prefix: &str, width: usize| -> Result<&[f32], FaceAnalysisError> {
            let name = format!("{prefix}_{stride}");
            let output = outputs.get(&name).ok_or_else(|| {
                FaceAnalysisError::InferenceFailed(format!("Missing YuNet output {name}"))
            })?;
            let (_, data) = output
                .try_extract_tensor::<f32>()
                .map_err(|e| FaceAnalysisError::InferenceFailed(e.to_string()))?;
            let cells = (INPUT_SIZE as usize / stride).pow(2);
            if data.len() != cells * width {
                return Err(FaceAnalysisError::InferenceFailed(format!(
                    "Unexpected YuNet output size for {name}: {}",
                    data.len()
                )));
            }
            Ok(data)
        };
        let cls = read("cls", 1)?;
        let obj = read("obj", 1)?;
        let bbox = read("bbox", 4)?;
        let kps = read("kps", 10)?;
        detections.extend(decode_head(cls, obj, bbox, kps, stride, scale_x, scale_y));
    }
    Ok(detections)
}

fn decode_head(
    cls: &[f32],
    obj: &[f32],
    bbox: &[f32],
    kps: &[f32],
    stride: usize,
    scale_x: f32,
    scale_y: f32,
) -> Vec<RawDetection> {
    let mut detections = Vec::new();
    let cols = INPUT_SIZE as usize / stride;
    for index in 0..cls.len() {
        let score = (cls[index].clamp(0.0, 1.0) * obj[index].clamp(0.0, 1.0)).sqrt();
        if !score.is_finite() || score < SCORE_THRESHOLD {
            continue;
        }
        let col = (index % cols) as f32;
        let row = (index / cols) as f32;
        let stride = stride as f32;
        let cx = (col + bbox[index * 4]) * stride;
        let cy = (row + bbox[index * 4 + 1]) * stride;
        let width = bbox[index * 4 + 2].exp() * stride;
        let height = bbox[index * 4 + 3].exp() * stride;
        if ![cx, cy, width, height]
            .iter()
            .all(|value| value.is_finite())
        {
            continue;
        }
        let point = |n: usize| -> (f32, f32) {
            (
                (kps[index * 10 + n * 2] + col) * stride * scale_x,
                (kps[index * 10 + n * 2 + 1] + row) * stride * scale_y,
            )
        };
        detections.push(RawDetection {
            bbox: BoundingBox {
                x: (cx - width / 2.0) * scale_x,
                y: (cy - height / 2.0) * scale_y,
                width: width * scale_x,
                height: height * scale_y,
            },
            score,
            right_eye: point(0),
            left_eye: point(1),
            nose: point(2),
            mouth_right: point(3),
            mouth_left: point(4),
        });
    }
    detections
}

struct RawDetection {
    bbox: BoundingBox,
    score: f32,
    /// Subject's anatomical right eye (YuNet cols 4–5). Image-left on a frontal face.
    right_eye: (f32, f32),
    /// Subject's anatomical left eye (YuNet cols 6–7). Image-right on a frontal face.
    left_eye: (f32, f32),
    nose: (f32, f32),
    mouth_right: (f32, f32),
    mouth_left: (f32, f32),
}

fn build_input_tensor(rgb: &RgbImage, width: u32, height: u32) -> Tensor<f32> {
    let resized =
        image::imageops::resize(rgb, width, height, image::imageops::FilterType::Triangle);
    let mut data = vec![0.0f32; (3 * width * height) as usize];
    let plane = (width * height) as usize;
    for (i, pixel) in resized.pixels().enumerate() {
        data[i] = pixel[2] as f32;
        data[plane + i] = pixel[1] as f32;
        data[2 * plane + i] = pixel[0] as f32;
    }
    Tensor::from_array((
        [1_i64, 3, height as i64, width as i64],
        data.into_boxed_slice(),
    ))
    .expect("tensor shape valid")
}

fn iou(a: &BoundingBox, b: &BoundingBox) -> f32 {
    let x1 = a.x.max(b.x);
    let y1 = a.y.max(b.y);
    let x2 = (a.x + a.width).min(b.x + b.width);
    let y2 = (a.y + a.height).min(b.y + b.height);
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let union = a.width * a.height + b.width * b.height - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

fn nms(mut detections: Vec<RawDetection>, threshold: f32) -> Vec<RawDetection> {
    detections.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept = Vec::new();
    for det in detections {
        if kept
            .iter()
            .any(|k: &RawDetection| iou(&k.bbox, &det.bbox) > threshold)
        {
            continue;
        }
        kept.push(det);
    }
    kept
}

fn enrich_face(gray: &GrayImage, raw: RawDetection) -> DetectedFace {
    let (img_w, img_h) = gray.dimensions();
    let bx = raw.bbox.x.max(0.0) as u32;
    let by = raw.bbox.y.max(0.0) as u32;
    let bw = raw.bbox.width.max(1.0) as u32;
    let bh = raw.bbox.height.max(1.0) as u32;
    let bw = bw.min(img_w.saturating_sub(bx));
    let bh = bh.min(img_h.saturating_sub(by));

    let face_sharpness = region_sharpness(gray, bx, by, bw, bh);
    let face_scale = raw.bbox.width.min(raw.bbox.height);

    let left_eye = analyze_eye(gray, raw.left_eye, face_scale);
    let right_eye = analyze_eye(gray, raw.right_eye, face_scale);
    let expression = analyze_expression(
        &raw.bbox,
        raw.nose,
        raw.mouth_left,
        raw.mouth_right,
        face_scale,
    );

    DetectedFace {
        bbox: raw.bbox,
        detection_confidence: raw.score,
        face_sharpness,
        left_eye: Some(left_eye),
        right_eye: Some(right_eye),
        expression: Some(expression),
    }
}

fn analyze_eye(gray: &GrayImage, center: (f32, f32), face_scale: f32) -> EyeAnalysis {
    let visibility_confidence = (face_scale / 80.0).clamp(0.0, 1.0);
    if face_scale < MIN_MEANINGFUL_FACE_PX {
        return EyeAnalysis {
            visibility_confidence,
            openness_score: None,
            state: EyeState::Uncertain,
            confidence: 0.2,
            region_sharpness: None,
        };
    }

    let (w, h) = gray.dimensions();
    let (x, y, patch_w, patch_h) = eye_patch_rect(center, face_scale, w, h);
    let region_sharpness = region_sharpness(gray, x, y, patch_w, patch_h);

    let mut vertical_edges = 0.0f32;
    let mut samples = 0u32;
    for py in (y + 1)..(y + patch_h.saturating_sub(1)).min(h - 1) {
        for px in (x + 1)..(x + patch_w.saturating_sub(1)).min(w - 1) {
            let up = gray.get_pixel(px, py - 1)[0] as f32;
            let down = gray.get_pixel(px, py + 1)[0] as f32;
            vertical_edges += (up - down).abs();
            samples += 1;
        }
    }
    let openness_score = if samples == 0 {
        None
    } else {
        Some((vertical_edges / samples as f32 / 64.0).clamp(0.0, 1.0))
    };

    let (state, confidence) = match openness_score {
        None => (EyeState::Uncertain, 0.25),
        Some(score) if score >= EYE_OPEN_THRESHOLD => {
            let conf = ((score - EYE_OPEN_THRESHOLD) / 0.35 + 0.55).clamp(0.0, 1.0);
            (EyeState::Open, conf * visibility_confidence)
        }
        Some(score) if score <= EYE_CLOSED_THRESHOLD => {
            let conf = ((EYE_CLOSED_THRESHOLD - score) / 0.35 + 0.55).clamp(0.0, 1.0);
            (EyeState::Closed, conf * visibility_confidence)
        }
        Some(_) => (EyeState::Uncertain, 0.45 * visibility_confidence),
    };

    EyeAnalysis {
        visibility_confidence,
        openness_score,
        state,
        confidence,
        region_sharpness,
    }
}

fn analyze_expression(
    bbox: &BoundingBox,
    nose: (f32, f32),
    mouth_left: (f32, f32),
    mouth_right: (f32, f32),
    face_scale: f32,
) -> ExpressionAnalysis {
    let confidence = (face_scale / 70.0).clamp(0.0, 1.0);
    if face_scale < MIN_MEANINGFUL_FACE_PX {
        return ExpressionAnalysis {
            smile_score: None,
            confidence: confidence * 0.3,
        };
    }
    let mouth_width =
        ((mouth_right.0 - mouth_left.0).powi(2) + (mouth_right.1 - mouth_left.1).powi(2)).sqrt();
    let width_norm = mouth_width / bbox.width.max(1.0);
    let mouth_y = (mouth_left.1 + mouth_right.1) * 0.5;
    let corner_lift = ((nose.1 - mouth_y) / bbox.height.max(1.0)).clamp(-0.5, 0.5);
    let smile_score = Some((width_norm * 0.65 + (corner_lift + 0.15) * 0.35).clamp(0.0, 1.0));
    ExpressionAnalysis {
        smile_score,
        confidence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_path_points_at_bundled_file_in_dev() {
        let path = resolve_model_path();
        assert!(
            path.ends_with("face_detection_yunet_2023mar.onnx"),
            "{path:?}"
        );
    }

    #[test]
    fn analyze_blank_image_returns_zero_faces() {
        let rgb = RgbImage::from_pixel(128, 128, image::Rgb([120, 120, 120]));
        let image = AnalysisImage::from_rgb(rgb);
        let result = analyze_yunet(&image).expect("inference");
        assert_eq!(result.face_count, 0);
    }

    #[tokio::test]
    #[ignore = "requires an explicitly selected local photo library"]
    async fn manual_real_library_face_smoke() {
        use sqlx::sqlite::SqliteConnectOptions;
        let path = std::env::var("PHOTOMIND_TEST_DB").expect("set PHOTOMIND_TEST_DB");
        let pool = sqlx::SqlitePool::connect_with(
            SqliteConnectOptions::new().filename(path).read_only(true),
        )
        .await
        .unwrap();
        let paths: Vec<String> = sqlx::query_scalar(
            "SELECT p.absolute_path FROM photos p WHERE NOT EXISTS
             (SELECT 1 FROM bin_entries b WHERE b.photo_id = p.id) LIMIT 50",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let mut found = 0;
        let mut checked = 0;
        for path in paths {
            if let Ok(image) = crate::ingest::load_normalized(std::path::Path::new(&path), 512) {
                checked += 1;
                found += analyze_yunet(&AnalysisImage::from_rgb(image.into_rgb8()))
                    .unwrap()
                    .face_count;
            }
        }
        println!("Checked {checked} local photos; detected {found} faces");
        assert!(found > 0, "No faces found in the selected library");
    }

    #[test]
    fn baseline_v1_thresholds_and_normalization_unchanged() {
        assert_eq!(EYE_OPEN_THRESHOLD, 0.52);
        assert_eq!(EYE_CLOSED_THRESHOLD, 0.38);
        assert_eq!(MIN_MEANINGFUL_FACE_PX, 24.0);
        let (x, y, w, h) = eye_patch_rect((40.0, 50.0), 80.0, 200, 200);
        // radius = max(80*0.08, 4) = 6; patch is 12x12 at (34, 44)
        assert_eq!((x, y, w, h), (34, 44, 12, 12));
    }

    #[test]
    fn yunet_landmarks_are_subject_anatomical_left_and_right() {
        let cells = (INPUT_SIZE / 8).pow(2) as usize;
        let mut cls = vec![0.0; cells];
        let mut obj = vec![0.0; cells];
        let mut bbox = vec![0.0; cells * 4];
        let mut kps = vec![0.0; cells * 10];
        let index = 10 * (INPUT_SIZE as usize / 8) + 12;
        cls[index] = 0.9;
        obj[index] = 0.9;
        bbox[index * 4 + 2] = (10.0_f32).ln();
        bbox[index * 4 + 3] = (11.0_f32).ln();
        kps[index * 10] = -2.0;
        kps[index * 10 + 2] = 2.0;
        let dets = decode_head(&cls, &obj, &bbox, &kps, 8, 1.0, 1.0);
        assert_eq!(dets.len(), 1);
        assert_eq!(dets[0].right_eye, (80.0, 80.0));
        assert_eq!(dets[0].left_eye, (112.0, 80.0));
        assert!((dets[0].bbox.width - 80.0).abs() < 0.001);
        assert!(
            dets[0].right_eye.0 < dets[0].left_eye.0,
            "frontal subject-right eye must sit at smaller image-x than subject-left"
        );
        let geom = FaceGeometry {
            bbox: dets[0].bbox.clone(),
            detection_confidence: dets[0].score,
            subject_left_eye: dets[0].left_eye,
            subject_right_eye: dets[0].right_eye,
        };
        assert_eq!(geom.subject_left_eye, (112.0, 80.0));
        assert_eq!(geom.subject_right_eye, (80.0, 80.0));
    }
}
