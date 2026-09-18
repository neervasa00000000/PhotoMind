//! Baseline V1 metrics. Do not tune thresholds from this module.

use super::{is_valid_label, list_image_files, load_ground_truth, ANALYSIS_MAX_EDGE};
use crate::face_analysis::yunet::{self, AnalysisImage};
use crate::face_analysis::{DetectedFace, EyeState};
use crate::ingest;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use image::DynamicImage;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum SizeBucket {
    Under40,
    From40To79,
    From80To119,
    From120To199,
    From200Plus,
}

impl SizeBucket {
    pub fn from_px(px: f32) -> Self {
        if px < 40.0 {
            Self::Under40
        } else if px < 80.0 {
            Self::From40To79
        } else if px < 120.0 {
            Self::From80To119
        } else if px < 200.0 {
            Self::From120To199
        } else {
            Self::From200Plus
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Under40 => "<40px",
            Self::From40To79 => "40–79px",
            Self::From80To119 => "80–119px",
            Self::From120To199 => "120–199px",
            Self::From200Plus => "200px+",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum ConfidenceBucket {
    Below50,
    From50To60,
    From60To70,
    From70To80,
    From80To90,
    From90To100,
}

impl ConfidenceBucket {
    pub fn from_conf(c: f32) -> Self {
        if c < 0.50 {
            Self::Below50
        } else if c < 0.60 {
            Self::From50To60
        } else if c < 0.70 {
            Self::From60To70
        } else if c < 0.80 {
            Self::From70To80
        } else if c < 0.90 {
            Self::From80To90
        } else {
            Self::From90To100
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Below50 => "<0.50",
            Self::From50To60 => "0.50–0.60",
            Self::From60To70 => "0.60–0.70",
            Self::From70To80 => "0.70–0.80",
            Self::From80To90 => "0.80–0.90",
            Self::From90To100 => "0.90–1.00",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Confusion {
    pub true_open_pred_open: usize,
    pub true_open_pred_closed: usize,
    pub true_open_pred_uncertain: usize,
    pub true_closed_pred_open: usize,
    pub true_closed_pred_closed: usize,
    pub true_closed_pred_uncertain: usize,
}

impl Default for Confusion {
    fn default() -> Self {
        Self {
            true_open_pred_open: 0,
            true_open_pred_closed: 0,
            true_open_pred_uncertain: 0,
            true_closed_pred_open: 0,
            true_closed_pred_closed: 0,
            true_closed_pred_uncertain: 0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct BucketStats {
    pub samples: usize,
    pub confident: usize,
    pub correct: usize,
    pub uncertain: usize,
    pub false_closed: usize,
}

impl BucketStats {
    fn record(&mut self, gt_open: bool, pred: EyeState) {
        self.samples += 1;
        match pred {
            EyeState::Uncertain => self.uncertain += 1,
            EyeState::Open => {
                self.confident += 1;
                if gt_open {
                    self.correct += 1;
                }
            }
            EyeState::Closed => {
                self.confident += 1;
                if !gt_open {
                    self.correct += 1;
                }
                if gt_open {
                    self.false_closed += 1;
                }
            }
        }
    }

    pub fn coverage(&self) -> f64 {
        ratio(self.confident, self.samples)
    }

    pub fn accuracy(&self) -> f64 {
        ratio(self.correct, self.confident)
    }

    pub fn false_closed_rate(&self) -> f64 {
        ratio(self.false_closed, self.samples)
    }

    pub fn uncertain_rate(&self) -> f64 {
        ratio(self.uncertain, self.samples)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FailureCase {
    pub filename: String,
    pub face_index: usize,
    pub side: String,
    pub ground_truth: String,
    pub predicted: String,
    pub confidence: f32,
    pub face_size: f32,
    pub openness_score: Option<f32>,
    pub face_jpeg: String,
    pub eye_jpeg: String,
}

#[derive(Debug, Clone)]
pub struct FalseClosedSample {
    pub confidence: f32,
    pub face_size: f32,
    pub openness_score: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct ValidationReport {
    pub images_processed: usize,
    pub images_failed: usize,
    pub faces_detected: usize,
    pub labelled_eyes: usize,
    pub open_gt: usize,
    pub closed_gt: usize,
    pub ignore: usize,
    pub photomind_open: usize,
    pub photomind_closed: usize,
    pub photomind_uncertain: usize,
    pub confusion: Confusion,
    pub false_closed: Vec<FalseClosedSample>,
    pub size: BTreeMap<SizeBucket, BucketStats>,
    pub confidence: BTreeMap<ConfidenceBucket, BucketStats>,
    pub openness_hist: BTreeMap<String, usize>,
    pub failures: Vec<FailureCase>,
}

impl Default for ValidationReport {
    fn default() -> Self {
        Self {
            images_processed: 0,
            images_failed: 0,
            faces_detected: 0,
            labelled_eyes: 0,
            open_gt: 0,
            closed_gt: 0,
            ignore: 0,
            photomind_open: 0,
            photomind_closed: 0,
            photomind_uncertain: 0,
            confusion: Confusion::default(),
            false_closed: Vec::new(),
            size: BTreeMap::new(),
            confidence: BTreeMap::new(),
            openness_hist: BTreeMap::new(),
            failures: Vec::new(),
        }
    }
}

impl ValidationReport {
    pub fn testable_eyes(&self) -> usize {
        self.open_gt + self.closed_gt
    }

    pub fn confident_predictions(&self) -> usize {
        self.photomind_open + self.photomind_closed
    }

    pub fn confident_correct(&self) -> usize {
        self.confusion.true_open_pred_open + self.confusion.true_closed_pred_closed
    }

    pub fn confident_accuracy(&self) -> f64 {
        ratio(self.confident_correct(), self.confident_predictions())
    }

    pub fn coverage(&self) -> f64 {
        ratio(self.confident_predictions(), self.testable_eyes())
    }

    pub fn uncertain_rate(&self) -> f64 {
        ratio(self.photomind_uncertain, self.testable_eyes())
    }

    pub fn false_closed_count(&self) -> usize {
        self.confusion.true_open_pred_closed
    }

    pub fn false_closed_rate(&self) -> f64 {
        ratio(self.false_closed_count(), self.open_gt)
    }

    pub fn false_open_rate(&self) -> f64 {
        ratio(self.confusion.true_closed_pred_open, self.closed_gt)
    }

    pub fn open_precision(&self) -> f64 {
        ratio(
            self.confusion.true_open_pred_open,
            self.confusion.true_open_pred_open + self.confusion.true_closed_pred_open,
        )
    }

    pub fn open_recall(&self) -> f64 {
        ratio(self.confusion.true_open_pred_open, self.open_gt)
    }

    pub fn closed_precision(&self) -> f64 {
        ratio(
            self.confusion.true_closed_pred_closed,
            self.confusion.true_closed_pred_closed + self.confusion.true_open_pred_closed,
        )
    }

    pub fn closed_recall(&self) -> f64 {
        ratio(self.confusion.true_closed_pred_closed, self.closed_gt)
    }
}

pub fn record_eye(
    report: &mut ValidationReport,
    gt: &str,
    pred: EyeState,
    confidence: f32,
    face_size: f32,
    openness_score: Option<f32>,
    failure: Option<FailureCase>,
) {
    if gt == "IGNORE" {
        report.ignore += 1;
        report.labelled_eyes += 1;
        return;
    }
    if gt != "OPEN" && gt != "CLOSED" {
        return;
    }
    report.labelled_eyes += 1;
    let gt_open = gt == "OPEN";
    if gt_open {
        report.open_gt += 1;
    } else {
        report.closed_gt += 1;
    }
    match pred {
        EyeState::Open => report.photomind_open += 1,
        EyeState::Closed => report.photomind_closed += 1,
        EyeState::Uncertain => report.photomind_uncertain += 1,
    }
    match (gt, pred) {
        ("OPEN", EyeState::Open) => report.confusion.true_open_pred_open += 1,
        ("OPEN", EyeState::Closed) => {
            report.confusion.true_open_pred_closed += 1;
            report.false_closed.push(FalseClosedSample {
                confidence,
                face_size,
                openness_score,
            });
        }
        ("OPEN", EyeState::Uncertain) => report.confusion.true_open_pred_uncertain += 1,
        ("CLOSED", EyeState::Open) => report.confusion.true_closed_pred_open += 1,
        ("CLOSED", EyeState::Closed) => report.confusion.true_closed_pred_closed += 1,
        ("CLOSED", EyeState::Uncertain) => report.confusion.true_closed_pred_uncertain += 1,
        _ => {}
    }
    report
        .size
        .entry(SizeBucket::from_px(face_size))
        .or_default()
        .record(gt_open, pred);
    report
        .confidence
        .entry(ConfidenceBucket::from_conf(confidence))
        .or_default()
        .record(gt_open, pred);
    if matches!(pred, EyeState::Closed) && gt_open {
        let key = openness_bucket(openness_score);
        *report.openness_hist.entry(key).or_default() += 1;
    }
    if let Some(case) = failure {
        report.failures.push(case);
    }
}

pub fn run_validation(
    photos_dir: &Path,
    ground_truth_path: Option<&Path>,
    failure_review_path: Option<&Path>,
    report_path: Option<&Path>,
) -> Result<ValidationReport, String> {
    let ground_truth = match ground_truth_path {
        Some(path) => Some(load_ground_truth(path)?),
        None => None,
    };
    let images = list_image_files(photos_dir)?;
    let mut report = ValidationReport::default();

    for img_path in &images {
        let filename = img_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let rgb = match ingest::load_normalized(img_path, ANALYSIS_MAX_EDGE) {
            Ok(img) => img.into_rgb8(),
            Err(_) => {
                report.images_failed += 1;
                continue;
            }
        };
        let analysis = match yunet::analyze_yunet(&AnalysisImage::from_rgb(rgb.clone())) {
            Ok(a) => a,
            Err(_) => {
                report.images_failed += 1;
                continue;
            }
        };
        report.images_processed += 1;
        report.faces_detected += analysis.faces.len();
        let gt_faces = ground_truth
            .as_ref()
            .and_then(|gt| gt.files.get(&filename));
        let geometry = yunet::detect_face_geometry(&AnalysisImage::from_rgb(rgb.clone())).unwrap_or_default();
        for (face_idx, face) in analysis.faces.iter().enumerate() {
            let gt_face = gt_faces.and_then(|faces| faces.iter().find(|f| f.face == face_idx));
            let face_size = face.bbox.width.min(face.bbox.height);
            let (left_c, right_c) = geometry
                .get(face_idx)
                .map(|g| (g.subject_left_eye, g.subject_right_eye))
                .unwrap_or((
                    (face.bbox.x + face.bbox.width * 0.7, face.bbox.y + face.bbox.height * 0.38),
                    (face.bbox.x + face.bbox.width * 0.3, face.bbox.y + face.bbox.height * 0.38),
                ));
            record_detected_eye(
                &mut report,
                &rgb,
                face,
                filename.as_str(),
                face_idx,
                "SUBJECT'S LEFT EYE",
                left_c,
                face.left_eye.as_ref(),
                gt_face.map(|f| f.left_eye.as_str()),
                face_size,
            );
            record_detected_eye(
                &mut report,
                &rgb,
                face,
                filename.as_str(),
                face_idx,
                "SUBJECT'S RIGHT EYE",
                right_c,
                face.right_eye.as_ref(),
                gt_face.map(|f| f.right_eye.as_str()),
                face_size,
            );
        }
    }

    if let Some(path) = report_path {
        super::atomic_write(path, &format_report(&report))?;
    }
    if let Some(path) = failure_review_path {
        super::atomic_write(path, &failure_review_html(&report))?;
    }
    let _ = ground_truth;
    Ok(report)
}

fn record_detected_eye(
    report: &mut ValidationReport,
    rgb: &image::RgbImage,
    face: &DetectedFace,
    filename: &str,
    face_idx: usize,
    side: &str,
    eye_center: (f32, f32),
    eye: Option<&crate::face_analysis::EyeAnalysis>,
    gt: Option<&str>,
    face_size: f32,
) {
    let Some(gt) = gt else {
        return;
    };
    if !is_valid_label(gt) {
        return;
    }
    let Some(eye) = eye else {
        if gt != "IGNORE" {
            record_eye(
                report,
                gt,
                EyeState::Uncertain,
                0.0,
                face_size,
                None,
                None,
            );
        } else {
            report.ignore += 1;
            report.labelled_eyes += 1;
        }
        return;
    };
    let dangerous = (gt == "OPEN" && eye.state == EyeState::Closed)
        || (gt == "CLOSED" && eye.state == EyeState::Open);
    let failure = if dangerous {
        Some(FailureCase {
            filename: filename.to_string(),
            face_index: face_idx,
            side: side.to_string(),
            ground_truth: gt.to_string(),
            predicted: eye.state.as_str().to_string(),
            confidence: eye.confidence,
            face_size,
            openness_score: eye.openness_score,
            face_jpeg: jpeg_crop(rgb, &face.bbox, 0.12),
            eye_jpeg: jpeg_eye(rgb, eye_center, face_size),
        })
    } else {
        None
    };
    record_eye(
        report,
        gt,
        eye.state,
        eye.confidence,
        face_size,
        eye.openness_score,
        failure,
    );
}

fn jpeg_crop(rgb: &image::RgbImage, bbox: &crate::face_analysis::BoundingBox, pad: f32) -> String {
    let pad_x = bbox.width * pad;
    let pad_y = bbox.height * pad;
    let x = (bbox.x - pad_x).max(0.0) as u32;
    let y = (bbox.y - pad_y).max(0.0) as u32;
    let w = (bbox.width + pad_x * 2.0).max(1.0) as u32;
    let h = (bbox.height + pad_y * 2.0).max(1.0) as u32;
    let crop = image::imageops::crop_imm(
        rgb,
        x.min(rgb.width().saturating_sub(1)),
        y.min(rgb.height().saturating_sub(1)),
        w.max(1).min(rgb.width().saturating_sub(x.min(rgb.width().saturating_sub(1)))),
        h.max(1).min(rgb.height().saturating_sub(y.min(rgb.height().saturating_sub(1)))),
    )
    .to_image();
    match ingest::encode_jpeg(&DynamicImage::ImageRgb8(crop)) {
        Ok(bytes) => format!("data:image/jpeg;base64,{}", STANDARD.encode(bytes)),
        Err(_) => String::new(),
    }
}

fn jpeg_eye(rgb: &image::RgbImage, center: (f32, f32), face_size: f32) -> String {
    let radius = (face_size * 0.22).max(12.0) as u32;
    let cx = center.0.max(0.0) as u32;
    let cy = center.1.max(0.0) as u32;
    let x = cx.saturating_sub(radius).min(rgb.width().saturating_sub(1));
    let y = cy.saturating_sub(radius).min(rgb.height().saturating_sub(1));
    let w = (radius * 2).max(1).min(rgb.width().saturating_sub(x));
    let h = (radius * 2).max(1).min(rgb.height().saturating_sub(y));
    let crop = image::imageops::crop_imm(rgb, x, y, w, h).to_image();
    match ingest::encode_jpeg(&DynamicImage::ImageRgb8(crop)) {
        Ok(bytes) => format!("data:image/jpeg;base64,{}", STANDARD.encode(bytes)),
        Err(_) => String::new(),
    }
}

fn openness_bucket(score: Option<f32>) -> String {
    match score {
        None => "none".into(),
        Some(s) => {
            let lo = ((s * 10.0).floor() / 10.0).clamp(0.0, 0.9);
            format!("{lo:.2}–{:.2}", (lo + 0.10).min(1.0))
        }
    }
}

fn ratio(num: usize, den: usize) -> f64 {
    if den == 0 {
        0.0
    } else {
        num as f64 / den as f64
    }
}

fn pct(v: f64) -> String {
    format!("{:.1}%", v * 100.0)
}

fn hist_conf(samples: &[FalseClosedSample]) -> String {
    let mut map = BTreeMap::new();
    for s in samples {
        *map.entry(ConfidenceBucket::from_conf(s.confidence)).or_insert(0usize) += 1;
    }
    if map.is_empty() {
        return "    (none)".into();
    }
    map.into_iter()
        .map(|(b, n)| format!("    {}: {n}", b.label()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn hist_size(samples: &[FalseClosedSample]) -> String {
    let mut map = BTreeMap::new();
    for s in samples {
        *map.entry(SizeBucket::from_px(s.face_size)).or_insert(0usize) += 1;
    }
    if map.is_empty() {
        return "    (none)".into();
    }
    map.into_iter()
        .map(|(b, n)| format!("    {}: {n}", b.label()))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn format_report(report: &ValidationReport) -> String {
    let mut out = String::new();
    out.push_str("=== PhotoMind Eye Detection Baseline V1 ===\n\n");
    out.push_str("Eye algorithm changed: NO\n");
    out.push_str("Thresholds: EYE_OPEN_THRESHOLD=0.52  EYE_CLOSED_THRESHOLD=0.38\n");
    out.push_str("MIN_MEANINGFUL_FACE_PX=24.0  normalization=64.0\n");
    out.push_str("ENABLE_EYE_BASED_RANKING: false\n\n");
    out.push_str(&format!("Images processed: {}\n", report.images_processed));
    out.push_str(&format!("Images failed to load: {}\n", report.images_failed));
    out.push_str(&format!("Total faces detected: {}\n\n", report.faces_detected));

    out.push_str("TOTAL LABELLED EYES: {}\n".replace("{}", &report.labelled_eyes.to_string()).as_str());
    out.push_str(&format!("OPEN ground truth: {}\n", report.open_gt));
    out.push_str(&format!("CLOSED ground truth: {}\n", report.closed_gt));
    out.push_str(&format!("IGNORE: {}\n\n", report.ignore));

    out.push_str(&format!("PhotoMind OPEN: {}\n", report.photomind_open));
    out.push_str(&format!("PhotoMind CLOSED: {}\n", report.photomind_closed));
    out.push_str(&format!("PhotoMind UNCERTAIN: {}\n\n", report.photomind_uncertain));

    out.push_str(&format!(
        "CONFIDENT ACCURACY: {} ({}/{})\n",
        pct(report.confident_accuracy()),
        report.confident_correct(),
        report.confident_predictions()
    ));
    out.push_str(&format!("COVERAGE: {}\n", pct(report.coverage())));
    out.push_str(&format!("UNCERTAIN RATE: {}\n", pct(report.uncertain_rate())));
    out.push_str(&format!("FALSE-CLOSED RATE: {}\n", pct(report.false_closed_rate())));
    out.push_str(&format!("FALSE-OPEN RATE: {}\n", pct(report.false_open_rate())));
    out.push_str(&format!("OPEN precision: {}\n", pct(report.open_precision())));
    out.push_str(&format!("OPEN recall: {}\n", pct(report.open_recall())));
    out.push_str(&format!("CLOSED precision: {}\n", pct(report.closed_precision())));
    out.push_str(&format!("CLOSED recall: {}\n\n", pct(report.closed_recall())));

    out.push_str("--- Confusion matrix (including UNCERTAIN) ---\n");
    out.push_str("                 Pred OPEN   Pred CLOSED   Pred UNCERTAIN\n");
    out.push_str(&format!(
        "True OPEN        {:>9}   {:>11}   {:>14}\n",
        report.confusion.true_open_pred_open,
        report.confusion.true_open_pred_closed,
        report.confusion.true_open_pred_uncertain
    ));
    out.push_str(&format!(
        "True CLOSED      {:>9}   {:>11}   {:>14}\n\n",
        report.confusion.true_closed_pred_open,
        report.confusion.true_closed_pred_closed,
        report.confusion.true_closed_pred_uncertain
    ));

    out.push_str("--- FALSE CLOSED ---\n");
    out.push_str("GROUND TRUTH = OPEN\nPHOTOMIND = CLOSED\n");
    out.push_str("This is particularly dangerous for PhotoMind.\n");
    out.push_str(&format!("count: {}\n", report.false_closed_count()));
    out.push_str(&format!("rate: {} of true-OPEN eyes\n", pct(report.false_closed_rate())));
    out.push_str("confidence distribution:\n");
    out.push_str(&hist_conf(&report.false_closed));
    out.push_str("\nface-size distribution:\n");
    out.push_str(&hist_size(&report.false_closed));
    out.push_str("\nopenness-score distribution:\n");
    if report.openness_hist.is_empty() {
        out.push_str("    (none)\n");
    } else {
        for (bucket, n) in &report.openness_hist {
            out.push_str(&format!("    {bucket}: {n}\n"));
        }
    }

    out.push_str("\n--- SIZE ANALYSIS ---\n");
    for bucket in [
        SizeBucket::Under40,
        SizeBucket::From40To79,
        SizeBucket::From80To119,
        SizeBucket::From120To199,
        SizeBucket::From200Plus,
    ] {
        let stats = report.size.get(&bucket).cloned().unwrap_or_default();
        out.push_str(&format!(
            "{}: samples={}  coverage={}  accuracy={}  false-closed rate={}  uncertain rate={}\n",
            bucket.label(),
            stats.samples,
            pct(stats.coverage()),
            pct(stats.accuracy()),
            pct(stats.false_closed_rate()),
            pct(stats.uncertain_rate())
        ));
    }

    out.push_str("\n--- CONFIDENCE ANALYSIS ---\n");
    out.push_str("(Includes a <0.50 row plus the requested 0.50–1.00 bins.)\n");
    for bucket in [
        ConfidenceBucket::Below50,
        ConfidenceBucket::From50To60,
        ConfidenceBucket::From60To70,
        ConfidenceBucket::From70To80,
        ConfidenceBucket::From80To90,
        ConfidenceBucket::From90To100,
    ] {
        let stats = report.confidence.get(&bucket).cloned().unwrap_or_default();
        out.push_str(&format!(
            "{}: sample count={}  accuracy={}  false-closed count={}\n",
            bucket.label(),
            stats.samples,
            pct(stats.accuracy()),
            stats.false_closed
        ));
    }

    out.push_str("\nDo NOT enable eye ranking from this report.\n");
    out.push_str("Return the numbers and stop. The next step is a human decision:\n");
    out.push_str("A. KEEP THE 0 MB HEURISTIC\n");
    out.push_str("B. RESTRICT IT\n");
    out.push_str("C. IMPROVE THE HEURISTIC\n");
    out.push_str("D. ADD A SMALL DEDICATED MODEL\n");
    out
}

pub fn failure_review_html(report: &ValidationReport) -> String {
    let mut cards = String::new();
    let false_closed: Vec<&FailureCase> = report
        .failures
        .iter()
        .filter(|c| c.ground_truth == "OPEN" && c.predicted == "CLOSED")
        .collect();
    let false_open: Vec<&FailureCase> = report
        .failures
        .iter()
        .filter(|c| c.ground_truth == "CLOSED" && c.predicted == "OPEN")
        .collect();
    let render = |title: &str, cases: &[&FailureCase]| {
        let mut s = format!("<h2>{} ({})</h2>", title, cases.len());
        if cases.is_empty() {
            s.push_str("<p class='empty'>None.</p>");
            return s;
        }
        for c in cases {
            let score = c
                .openness_score
                .map(|v| format!("{v:.3}"))
                .unwrap_or_else(|| "none".into());
            s.push_str(&format!(
                "<article class='card'>
                    <header>
                        <strong>{}</strong> · face {} · {}
                    </header>
                    <p>GT {} → PhotoMind {} · conf {:.2} · face {:.0}px · openness {}</p>
                    <div class='imgs'>
                        <figure><img src='{}' alt='face'><figcaption>Face crop</figcaption></figure>
                        <figure><img src='{}' alt='eye'><figcaption>Eye region</figcaption></figure>
                    </div>
                </article>",
                html_escape(&c.filename),
                c.face_index + 1,
                html_escape(&c.side),
                html_escape(&c.ground_truth),
                html_escape(&c.predicted),
                c.confidence,
                c.face_size,
                score,
                c.face_jpeg,
                c.eye_jpeg
            ));
        }
        s
    };
    cards.push_str(&render(
        "Actual OPEN → predicted CLOSED",
        &false_closed,
    ));
    cards.push_str(&render(
        "Actual CLOSED → predicted OPEN",
        &false_open,
    ));
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>PhotoMind eye failure review (LOCAL)</title>
<style>
body {{ font-family: ui-sans-serif, system-ui, sans-serif; margin: 24px; background: #111; color: #eee; }}
h1 {{ font-size: 22px; }}
.card {{ background: #1c1c1c; border: 1px solid #333; border-radius: 12px; padding: 16px; margin: 12px 0; }}
.imgs {{ display: flex; gap: 16px; flex-wrap: wrap; }}
img {{ max-height: 220px; border-radius: 8px; background: #000; }}
.empty {{ opacity: 0.6; }}
.warn {{ color: #fbbf24; }}
</style>
</head>
<body>
<h1>PhotoMind eye failure review</h1>
<p class="warn">LOCAL ONLY. Gitignored. Original photographs were not modified. Filenames only — do not commit this file.</p>
<p>False-closed: {fc} · False-open: {fo}</p>
{cards}
</body>
</html>
"#,
        fc = false_closed.len(),
        fo = false_open.len(),
        cards = cards
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn false_closed_is_open_gt_closed_pred() {
        let mut report = ValidationReport::default();
        record_eye(&mut report, "OPEN", EyeState::Closed, 0.82, 90.0, Some(0.30), None);
        record_eye(&mut report, "OPEN", EyeState::Open, 0.9, 90.0, Some(0.70), None);
        record_eye(&mut report, "CLOSED", EyeState::Open, 0.7, 90.0, Some(0.60), None);
        record_eye(&mut report, "IGNORE", EyeState::Closed, 0.9, 90.0, None, None);
        assert_eq!(report.false_closed_count(), 1);
        assert_eq!(report.open_gt, 2);
        assert_eq!(report.ignore, 1);
        assert!((report.false_closed_rate() - 0.5).abs() < 1e-6);
        assert_eq!(report.confusion.true_closed_pred_open, 1);
        let text = format_report(&report);
        assert!(text.contains("FALSE CLOSED"));
        assert!(text.contains("GROUND TRUTH = OPEN"));
        assert!(text.contains("ENABLE_EYE_BASED_RANKING: false"));
    }

    #[test]
    fn size_and_confidence_buckets() {
        let mut report = ValidationReport::default();
        record_eye(&mut report, "OPEN", EyeState::Open, 0.55, 30.0, Some(0.6), None);
        record_eye(&mut report, "OPEN", EyeState::Closed, 0.95, 210.0, Some(0.2), None);
        assert_eq!(report.size[&SizeBucket::Under40].samples, 1);
        assert_eq!(report.size[&SizeBucket::From200Plus].false_closed, 1);
        assert_eq!(report.confidence[&ConfidenceBucket::From50To60].samples, 1);
        assert_eq!(report.confidence[&ConfidenceBucket::From90To100].false_closed, 1);
    }

    #[test]
    fn precision_recall_include_uncertain_as_miss_for_recall() {
        let mut report = ValidationReport::default();
        record_eye(&mut report, "OPEN", EyeState::Open, 0.8, 80.0, None, None);
        record_eye(&mut report, "OPEN", EyeState::Uncertain, 0.4, 80.0, None, None);
        record_eye(&mut report, "CLOSED", EyeState::Closed, 0.8, 80.0, None, None);
        assert!((report.open_recall() - 0.5).abs() < 1e-6);
        assert!((report.open_precision() - 1.0).abs() < 1e-6);
        assert!((report.closed_recall() - 1.0).abs() < 1e-6);
    }
}
