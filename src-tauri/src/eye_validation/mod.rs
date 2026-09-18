//! Development-only eye-validation helpers.
//!
//! Used by the `label_eyes` and `validate_eyes` binaries. Not invoked by the
//! production PhotoMind app. Ground-truth labels come only from the human
//! labeler; this module never writes PhotoMind OPEN/CLOSED predictions into
//! `ground_truth.json`.

pub mod metrics;

use crate::face_analysis::yunet::{self, FaceGeometry};
use crate::face_analysis::{eye_patch_rect, BoundingBox};
use crate::ingest;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use image::{DynamicImage, RgbImage};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Same preview size `validate_eyes` uses, so face indices match.
pub const ANALYSIS_MAX_EDGE: u32 = 512;
pub const MIN_USABLE_EYES: usize = 100;
pub const PREFERRED_USABLE_EYES: usize = 200;

pub const LABEL_OPEN: &str = "OPEN";
pub const LABEL_CLOSED: &str = "CLOSED";
pub const LABEL_IGNORE: &str = "IGNORE";

pub const COMPOSITION_CHECKLIST: &[&str] = &[
    "NORMAL: frontal",
    "NORMAL: open eyes",
    "NORMAL: closed eyes",
    "WINK: one open / one closed",
    "EXPRESSION: smiling",
    "EXPRESSION: strong smiling/squinting",
    "EYEWEAR: glasses",
    "EYEWEAR: reflections",
    "EYEWEAR: sunglasses",
    "POSE: slight angle",
    "POSE: side angle",
    "POSE: looking down",
    "POSE: looking sideways",
    "QUALITY: sharp",
    "QUALITY: slightly blurry",
    "QUALITY: dark",
    "QUALITY: bright",
    "SIZE: large face",
    "SIZE: medium face",
    "SIZE: small face",
    "GROUPS: 2 people",
    "GROUPS: several people",
    "GROUPS: mixed open/closed eyes",
];

#[derive(Debug, Clone, Default)]
pub struct GroundTruth {
    pub files: BTreeMap<String, Vec<GroundTruthFace>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroundTruthFace {
    pub face: usize,
    pub left_eye: String,
    pub right_eye: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LabelProgress {
    pub photos_reviewed: usize,
    pub faces_reviewed: usize,
    pub eyes_labelled: usize,
    pub open: usize,
    pub closed: usize,
    pub ignore: usize,
    pub usable_eyes: usize,
    pub min_target: usize,
    pub preferred_target: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreparedEye {
    pub center_x: f32,
    pub center_y: f32,
    pub patch: BoundingBox,
    pub jpeg: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreparedFace {
    pub face_index: usize,
    pub bbox: BoundingBox,
    pub face_jpeg: String,
    pub subject_left_eye: PreparedEye,
    pub subject_right_eye: PreparedEye,
}

#[derive(Debug, Clone)]
pub enum PreparedPhoto {
    Missing,
    Unreadable { message: String },
    NoFaces { width: u32, height: u32, jpeg: String },
    Faces {
        width: u32,
        height: u32,
        jpeg: String,
        faces: Vec<PreparedFace>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkItem {
    pub filename: String,
    pub kind: WorkKind,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum WorkKind {
    Face { face_index: usize, face_count: usize },
    NoFaces,
    Missing,
    Unreadable { message: String },
}

pub fn default_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("eye-validation")
}

pub fn photos_dir(root: &Path) -> PathBuf {
    root.join("photos")
}

pub fn ground_truth_path(root: &Path) -> PathBuf {
    root.join("ground_truth.json")
}

pub fn failure_review_path(root: &Path) -> PathBuf {
    root.join("failure-review.html")
}

pub fn results_dir(root: &Path) -> PathBuf {
    root.join("results")
}

pub fn ensure_validation_dirs(root: &Path) -> Result<(), String> {
    fs::create_dir_all(photos_dir(root)).map_err(|e| e.to_string())?;
    fs::create_dir_all(results_dir(root)).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn list_image_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if name.starts_with('.') || name.starts_with("._") {
            continue;
        }
        if let Some(ext) = path.extension() {
            let ext = ext.to_string_lossy().to_lowercase();
            if matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "heic" | "webp") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

pub fn is_valid_label(value: &str) -> bool {
    matches!(value, "OPEN" | "CLOSED" | "IGNORE")
}

pub fn parse_ground_truth(text: &str) -> Result<GroundTruth, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("ground_truth.json is corrupt: {e}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| "ground_truth.json must be a JSON object".to_string())?;
    let mut files = BTreeMap::new();
    for (key, val) in obj {
        if key.starts_with('_') || val.is_null() || !val.is_array() {
            continue;
        }
        let faces: Vec<GroundTruthFace> = serde_json::from_value(val.clone())
            .map_err(|e| format!("Invalid labels for {key}: {e}"))?;
        for face in &faces {
            if !face.left_eye.is_empty() && !is_valid_label(&face.left_eye) {
                return Err(format!(
                    "Invalid left-eye label in {key} face {}: {}",
                    face.face, face.left_eye
                ));
            }
            if !face.right_eye.is_empty() && !is_valid_label(&face.right_eye) {
                return Err(format!(
                    "Invalid right-eye label in {key} face {}: {}",
                    face.face, face.right_eye
                ));
            }
        }
        files.insert(key.clone(), faces);
    }
    Ok(GroundTruth { files })
}

pub fn load_ground_truth(path: &Path) -> Result<GroundTruth, String> {
    if !path.exists() {
        return Ok(GroundTruth::default());
    }
    let text = fs::read_to_string(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(GroundTruth::default());
    }
    parse_ground_truth(&text)
}

pub fn ground_truth_json(gt: &GroundTruth) -> Result<String, String> {
    let mut map = serde_json::Map::new();
    map.insert(
        "_meta".into(),
        serde_json::json!({
            "left_eye": "SUBJECT'S LEFT EYE (anatomical, YuNet left landmark)",
            "right_eye": "SUBJECT'S RIGHT EYE (anatomical, YuNet right landmark)",
            "face": "0-based detection order; same order as validate_eyes"
        }),
    );
    for (name, faces) in &gt.files {
        map.insert(
            name.clone(),
            serde_json::to_value(faces).map_err(|e| e.to_string())?,
        );
    }
    serde_json::to_string_pretty(&serde_json::Value::Object(map)).map_err(|e| e.to_string())
}

/// Write `contents` via a sibling temp file + rename so a crash cannot truncate
/// an existing `ground_truth.json`.
pub fn atomic_write(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let file_name = path
        .file_name()
        .ok_or_else(|| "atomic_write path has no file name".to_string())?
        .to_string_lossy();
    let tmp = path.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()));
    let write_tmp = || -> Result<(), String> {
        let mut file = File::create(&tmp).map_err(|e| e.to_string())?;
        file.write_all(contents.as_bytes()).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        Ok(())
    };
    if let Err(error) = write_tmp() {
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        e.to_string()
    })?;
    Ok(())
}

pub fn save_ground_truth(path: &Path, gt: &GroundTruth) -> Result<(), String> {
    atomic_write(path, &ground_truth_json(gt)?)
}

pub fn upsert_face_label(
    gt: &mut GroundTruth,
    filename: &str,
    face_index: usize,
    left_eye: Option<&str>,
    right_eye: Option<&str>,
) -> Result<(), String> {
    if let Some(v) = left_eye {
        if !is_valid_label(v) {
            return Err(format!("Invalid left-eye label: {v}"));
        }
    }
    if let Some(v) = right_eye {
        if !is_valid_label(v) {
            return Err(format!("Invalid right-eye label: {v}"));
        }
    }
    let faces = gt.files.entry(filename.to_string()).or_default();
    if let Some(existing) = faces.iter_mut().find(|f| f.face == face_index) {
        if let Some(v) = left_eye {
            existing.left_eye = v.to_string();
        }
        if let Some(v) = right_eye {
            existing.right_eye = v.to_string();
        }
    } else {
        faces.push(GroundTruthFace {
            face: face_index,
            left_eye: left_eye.unwrap_or("").to_string(),
            right_eye: right_eye.unwrap_or("").to_string(),
        });
        faces.sort_by_key(|f| f.face);
    }
    Ok(())
}

pub fn mark_photo_reviewed(gt: &mut GroundTruth, filename: &str) {
    gt.files.entry(filename.to_string()).or_default();
}

pub fn face_labels<'a>(
    gt: &'a GroundTruth,
    filename: &str,
    face_index: usize,
) -> Option<&'a GroundTruthFace> {
    gt.files
        .get(filename)
        .and_then(|faces| faces.iter().find(|f| f.face == face_index))
}

pub fn face_is_complete(gt: &GroundTruth, filename: &str, face_index: usize) -> bool {
    face_labels(gt, filename, face_index)
        .map(|f| is_valid_label(&f.left_eye) && is_valid_label(&f.right_eye))
        .unwrap_or(false)
}

pub fn photo_is_reviewed(gt: &GroundTruth, filename: &str) -> bool {
    gt.files.contains_key(filename)
}

pub fn compute_progress(gt: &GroundTruth, photo_names: &[String]) -> LabelProgress {
    let known: std::collections::HashSet<&str> = photo_names.iter().map(|s| s.as_str()).collect();
    let mut progress = LabelProgress {
        min_target: MIN_USABLE_EYES,
        preferred_target: PREFERRED_USABLE_EYES,
        ..LabelProgress::default()
    };
    for (name, faces) in &gt.files {
        if !known.is_empty() && !known.contains(name.as_str()) {
            continue;
        }
        progress.photos_reviewed += 1;
        for face in faces {
            let left_ok = is_valid_label(&face.left_eye);
            let right_ok = is_valid_label(&face.right_eye);
            if left_ok && right_ok {
                progress.faces_reviewed += 1;
            }
            for label in [&face.left_eye, &face.right_eye] {
                if !is_valid_label(label) {
                    continue;
                }
                progress.eyes_labelled += 1;
                match label.as_str() {
                    "OPEN" => progress.open += 1,
                    "CLOSED" => progress.closed += 1,
                    "IGNORE" => progress.ignore += 1,
                    _ => {}
                }
            }
        }
    }
    progress.usable_eyes = progress.open + progress.closed;
    progress
}

pub fn first_unlabeled_index(items: &[WorkItem], gt: &GroundTruth) -> usize {
    for (idx, item) in items.iter().enumerate() {
        match item.kind {
            WorkKind::Face { face_index, .. } => {
                if !face_is_complete(gt, &item.filename, face_index) {
                    return idx;
                }
            }
            WorkKind::NoFaces | WorkKind::Missing | WorkKind::Unreadable { .. } => {
                if !photo_is_reviewed(gt, &item.filename) {
                    return idx;
                }
            }
        }
    }
    items.len().saturating_sub(1).min(items.len())
}

pub fn prepare_photo(path: &Path) -> PreparedPhoto {
    if !path.exists() {
        return PreparedPhoto::Missing;
    }
    let rgb = match ingest::load_normalized(path, ANALYSIS_MAX_EDGE) {
        Ok(img) => img.into_rgb8(),
        Err(message) => return PreparedPhoto::Unreadable { message },
    };
    let analysis = yunet::AnalysisImage::from_rgb(rgb.clone());
    let geometry = match yunet::detect_face_geometry(&analysis) {
        Ok(faces) => faces,
        Err(error) => {
            return PreparedPhoto::Unreadable {
                message: error.to_string(),
            }
        }
    };
    let jpeg = encode_rgb_jpeg(&rgb);
    let (width, height) = rgb.dimensions();
    if geometry.is_empty() {
        return PreparedPhoto::NoFaces {
            width,
            height,
            jpeg,
        };
    }
    let faces = geometry
        .into_iter()
        .enumerate()
        .map(|(face_index, geom)| prepare_face(&rgb, face_index, &geom))
        .collect();
    PreparedPhoto::Faces {
        width,
        height,
        jpeg,
        faces,
    }
}

pub fn build_work_items(paths: &[PathBuf], cache: &mut BTreeMap<String, PreparedPhoto>) -> Vec<WorkItem> {
    let mut items = Vec::new();
    for path in paths {
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let prepared = cache
            .entry(filename.clone())
            .or_insert_with(|| prepare_photo(path))
            .clone();
        match prepared {
            PreparedPhoto::Missing => items.push(WorkItem {
                filename,
                kind: WorkKind::Missing,
            }),
            PreparedPhoto::Unreadable { message } => items.push(WorkItem {
                filename,
                kind: WorkKind::Unreadable { message },
            }),
            PreparedPhoto::NoFaces { .. } => items.push(WorkItem {
                filename,
                kind: WorkKind::NoFaces,
            }),
            PreparedPhoto::Faces { ref faces, .. } => {
                let face_count = faces.len();
                for face in faces {
                    items.push(WorkItem {
                        filename: filename.clone(),
                        kind: WorkKind::Face {
                            face_index: face.face_index,
                            face_count,
                        },
                    });
                }
            }
        }
    }
    items
}

fn prepare_face(rgb: &RgbImage, face_index: usize, geom: &FaceGeometry) -> PreparedFace {
    let (img_w, img_h) = rgb.dimensions();
    let face_scale = geom.bbox.width.min(geom.bbox.height);
    let face_jpeg = encode_rgb_jpeg(&crop_bbox(rgb, &geom.bbox, 0.12));
    PreparedFace {
        face_index,
        bbox: geom.bbox.clone(),
        face_jpeg,
        subject_left_eye: prepare_eye(rgb, geom.subject_left_eye, face_scale, img_w, img_h),
        subject_right_eye: prepare_eye(rgb, geom.subject_right_eye, face_scale, img_w, img_h),
    }
}

fn prepare_eye(
    rgb: &RgbImage,
    center: (f32, f32),
    face_scale: f32,
    img_w: u32,
    img_h: u32,
) -> PreparedEye {
    let (px, py, pw, ph) = eye_patch_rect(center, face_scale, img_w, img_h);
    let context = context_eye_rect(center, face_scale, img_w, img_h);
    PreparedEye {
        center_x: center.0,
        center_y: center.1,
        patch: BoundingBox {
            x: px as f32,
            y: py as f32,
            width: pw as f32,
            height: ph as f32,
        },
        jpeg: encode_rgb_jpeg(&crop_rect(rgb, context.0, context.1, context.2, context.3)),
    }
}

fn context_eye_rect(
    center: (f32, f32),
    face_scale: f32,
    img_w: u32,
    img_h: u32,
) -> (u32, u32, u32, u32) {
    let radius = (face_scale * 0.22).max(12.0) as u32;
    let cx = center.0.max(0.0) as u32;
    let cy = center.1.max(0.0) as u32;
    let x = cx.saturating_sub(radius);
    let y = cy.saturating_sub(radius);
    let w = (radius * 2).min(img_w.saturating_sub(x));
    let h = (radius * 2).min(img_h.saturating_sub(y));
    (x, y, w, h)
}

fn crop_bbox(rgb: &RgbImage, bbox: &BoundingBox, pad: f32) -> RgbImage {
    let pad_x = bbox.width * pad;
    let pad_y = bbox.height * pad;
    let x = (bbox.x - pad_x).max(0.0) as u32;
    let y = (bbox.y - pad_y).max(0.0) as u32;
    let w = (bbox.width + pad_x * 2.0).max(1.0) as u32;
    let h = (bbox.height + pad_y * 2.0).max(1.0) as u32;
    crop_rect(rgb, x, y, w, h)
}

fn crop_rect(rgb: &RgbImage, x: u32, y: u32, w: u32, h: u32) -> RgbImage {
    let (img_w, img_h) = rgb.dimensions();
    let x = x.min(img_w.saturating_sub(1));
    let y = y.min(img_h.saturating_sub(1));
    let w = w.max(1).min(img_w.saturating_sub(x));
    let h = h.max(1).min(img_h.saturating_sub(y));
    image::imageops::crop_imm(rgb, x, y, w, h).to_image()
}

fn encode_rgb_jpeg(rgb: &RgbImage) -> String {
    match ingest::encode_jpeg(&DynamicImage::ImageRgb8(rgb.clone())) {
        Ok(bytes) => format!("data:image/jpeg;base64,{}", STANDARD.encode(bytes)),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("photomind-eye-val-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_gt() -> GroundTruth {
        let mut gt = GroundTruth::default();
        upsert_face_label(&mut gt, "a.jpg", 0, Some("OPEN"), Some("CLOSED")).unwrap();
        upsert_face_label(&mut gt, "a.jpg", 1, Some("IGNORE"), Some("OPEN")).unwrap();
        gt
    }

    #[test]
    fn label_save_and_reload_round_trip() {
        let dir = temp_dir();
        let path = dir.join("ground_truth.json");
        let gt = sample_gt();
        save_ground_truth(&path, &gt).unwrap();
        let loaded = load_ground_truth(&path).unwrap();
        assert_eq!(loaded.files.get("a.jpg").unwrap().len(), 2);
        assert_eq!(loaded.files["a.jpg"][0].left_eye, "OPEN");
        assert_eq!(loaded.files["a.jpg"][0].right_eye, "CLOSED");
        assert_eq!(loaded.files["a.jpg"][1].left_eye, "IGNORE");
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn resume_lands_on_first_unlabeled_face() {
        let mut gt = GroundTruth::default();
        upsert_face_label(&mut gt, "a.jpg", 0, Some("OPEN"), Some("OPEN")).unwrap();
        let items = vec![
            WorkItem {
                filename: "a.jpg".into(),
                kind: WorkKind::Face {
                    face_index: 0,
                    face_count: 2,
                },
            },
            WorkItem {
                filename: "a.jpg".into(),
                kind: WorkKind::Face {
                    face_index: 1,
                    face_count: 2,
                },
            },
            WorkItem {
                filename: "b.jpg".into(),
                kind: WorkKind::Face {
                    face_index: 0,
                    face_count: 1,
                },
            },
        ];
        assert_eq!(first_unlabeled_index(&items, &gt), 1);
        upsert_face_label(&mut gt, "a.jpg", 1, Some("CLOSED"), Some("CLOSED")).unwrap();
        assert_eq!(first_unlabeled_index(&items, &gt), 2);
    }

    #[test]
    fn multi_face_photo_keeps_independent_labels() {
        let gt = sample_gt();
        assert!(face_is_complete(&gt, "a.jpg", 0));
        assert!(face_is_complete(&gt, "a.jpg", 1));
        assert_eq!(gt.files["a.jpg"][0].face, 0);
        assert_eq!(gt.files["a.jpg"][1].face, 1);
        let progress = compute_progress(&gt, &["a.jpg".into()]);
        assert_eq!(progress.faces_reviewed, 2);
        assert_eq!(progress.open, 2);
        assert_eq!(progress.closed, 1);
        assert_eq!(progress.ignore, 1);
        assert_eq!(progress.usable_eyes, 3);
    }

    #[test]
    fn ignore_counts_as_labelled_but_not_usable() {
        let mut gt = GroundTruth::default();
        upsert_face_label(&mut gt, "s.jpg", 0, Some("IGNORE"), Some("IGNORE")).unwrap();
        let progress = compute_progress(&gt, &["s.jpg".into()]);
        assert_eq!(progress.eyes_labelled, 2);
        assert_eq!(progress.ignore, 2);
        assert_eq!(progress.usable_eyes, 0);
        assert!(face_is_complete(&gt, "s.jpg", 0));
    }

    #[test]
    fn atomic_write_replaces_without_destroying_previous_on_tmp_crash() {
        let dir = temp_dir();
        let path = dir.join("ground_truth.json");
        fs::write(&path, "{\"keep.jpg\":[{\"face\":0,\"left_eye\":\"OPEN\",\"right_eye\":\"OPEN\"}]}").unwrap();
        let tmp = path.with_file_name(format!(
            ".ground_truth.json.tmp-{}",
            std::process::id()
        ));
        fs::write(&tmp, "CORRUPT-PARTIAL").unwrap();
        let loaded = load_ground_truth(&path).unwrap();
        assert_eq!(loaded.files["keep.jpg"][0].left_eye, "OPEN");
        fs::remove_file(&tmp).ok();
        atomic_write(&path, "{\"keep.jpg\":[{\"face\":0,\"left_eye\":\"CLOSED\",\"right_eye\":\"CLOSED\"}]}").unwrap();
        let loaded = load_ground_truth(&path).unwrap();
        assert_eq!(loaded.files["keep.jpg"][0].left_eye, "CLOSED");
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn missing_image_is_reported() {
        let path = PathBuf::from("/definitely/missing/photomind-eye-val.jpg");
        assert!(matches!(prepare_photo(&path), PreparedPhoto::Missing));
    }

    #[test]
    fn corrupt_image_is_unreadable() {
        let dir = temp_dir();
        let path = dir.join("broken.jpg");
        fs::write(&path, b"this is not an image").unwrap();
        match prepare_photo(&path) {
            PreparedPhoto::Unreadable { message } => assert!(!message.is_empty()),
            other => panic!("expected Unreadable, got {other:?}"),
        }
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn no_detected_face_on_blank_image() {
        let dir = temp_dir();
        let path = dir.join("blank.png");
        RgbImage::from_pixel(128, 128, Rgb([120, 120, 120]))
            .save(&path)
            .unwrap();
        match prepare_photo(&path) {
            PreparedPhoto::NoFaces { .. } => {}
            other => panic!("expected NoFaces, got {other:?}"),
        }
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn left_right_payload_uses_subject_landmarks_and_hides_predictions() {
        let rgb = RgbImage::from_pixel(200, 200, Rgb([40, 40, 40]));
        let geom = FaceGeometry {
            bbox: BoundingBox {
                x: 40.0,
                y: 30.0,
                width: 80.0,
                height: 90.0,
            },
            detection_confidence: 0.9,
            subject_left_eye: (100.0, 55.0),
            subject_right_eye: (60.0, 54.0),
        };
        let face = prepare_face(&rgb, 0, &geom);
        assert_eq!(face.subject_left_eye.center_x, 100.0);
        assert_eq!(face.subject_right_eye.center_x, 60.0);
        assert!(face.subject_right_eye.center_x < face.subject_left_eye.center_x);
        let json = serde_json::to_string(&face).unwrap();
        assert!(json.contains("subject_left_eye"));
        assert!(json.contains("subject_right_eye"));
        assert!(!json.contains("openness"));
        assert!(!json.contains("UNCERTAIN"));
        assert!(!json.contains("\"state\""));
        assert!(!json.contains("confidence"));
    }

    #[test]
    fn corrupt_ground_truth_does_not_parse_as_empty() {
        let err = parse_ground_truth("{not json").unwrap_err();
        assert!(err.contains("corrupt"));
    }

    #[test]
    fn partial_labels_save_and_resume_stays_on_that_face() {
        let dir = temp_dir();
        let path = dir.join("ground_truth.json");
        let mut gt = GroundTruth::default();
        upsert_face_label(&mut gt, "a.jpg", 0, Some("OPEN"), None).unwrap();
        save_ground_truth(&path, &gt).unwrap();
        let loaded = load_ground_truth(&path).unwrap();
        assert_eq!(loaded.files["a.jpg"][0].left_eye, "OPEN");
        assert_eq!(loaded.files["a.jpg"][0].right_eye, "");
        assert!(!face_is_complete(&loaded, "a.jpg", 0));
        let items = vec![WorkItem {
            filename: "a.jpg".into(),
            kind: WorkKind::Face {
                face_index: 0,
                face_count: 1,
            },
        }];
        assert_eq!(first_unlabeled_index(&items, &loaded), 0);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn skips_comment_keys_in_ground_truth() {
        let gt = parse_ground_truth(
            r#"{
            "_comment": "hello",
            "_instructions": "label me",
            "real.jpg": [{"face":0,"left_eye":"OPEN","right_eye":"CLOSED"}]
        }"#,
        )
        .unwrap();
        assert_eq!(gt.files.len(), 1);
        assert_eq!(gt.files["real.jpg"][0].right_eye, "CLOSED");
    }

    #[test]
    fn resume_skips_reviewed_no_face_photos() {
        let mut gt = GroundTruth::default();
        mark_photo_reviewed(&mut gt, "empty.jpg");
        let items = vec![
            WorkItem {
                filename: "empty.jpg".into(),
                kind: WorkKind::NoFaces,
            },
            WorkItem {
                filename: "next.jpg".into(),
                kind: WorkKind::Face {
                    face_index: 0,
                    face_count: 1,
                },
            },
        ];
        assert_eq!(first_unlabeled_index(&items, &gt), 1);
    }
}
