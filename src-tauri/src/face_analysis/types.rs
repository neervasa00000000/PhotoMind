use serde::{Deserialize, Serialize};

pub const ANALYZER_VERSION: &str = "face-v1-yunet";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EyeState {
    Open,
    Closed,
    Uncertain,
}

impl EyeState {
    pub fn as_str(self) -> &'static str {
        match self {
            EyeState::Open => "OPEN",
            EyeState::Closed => "CLOSED",
            EyeState::Uncertain => "UNCERTAIN",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "OPEN" => EyeState::Open,
            "CLOSED" => EyeState::Closed,
            _ => EyeState::Uncertain,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundingBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EyeAnalysis {
    pub visibility_confidence: f32,
    pub openness_score: Option<f32>,
    pub state: EyeState,
    pub confidence: f32,
    pub region_sharpness: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionAnalysis {
    pub smile_score: Option<f32>,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedFace {
    pub bbox: BoundingBox,
    pub detection_confidence: f32,
    pub face_sharpness: Option<f32>,
    pub left_eye: Option<EyeAnalysis>,
    pub right_eye: Option<EyeAnalysis>,
    pub expression: Option<ExpressionAnalysis>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaceAnalysis {
    pub face_count: usize,
    pub faces: Vec<DetectedFace>,
    pub analyzer_version: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroupFaceMetrics {
    pub face_count: usize,
    pub likely_all_eyes_open: bool,
    pub closed_eye_count: usize,
    pub uncertain_eye_count: usize,
    pub average_face_sharpness: Option<f32>,
    pub weakest_face_sharpness: Option<f32>,
    pub average_expression_score: Option<f32>,
    pub open_eye_face_count: usize,
    pub mode_face_count: Option<usize>,
}

impl GroupFaceMetrics {
    pub fn from_faces(faces: &[DetectedFace]) -> Self {
        if faces.is_empty() {
            return GroupFaceMetrics::default();
        }
        let mut closed_eye_count = 0usize;
        let mut uncertain_eye_count = 0usize;
        let mut open_eye_faces = 0usize;
        let mut sharpness_values = Vec::new();
        let mut expression_values = Vec::new();

        for face in faces {
            let mut face_has_closed = false;
            let mut face_has_uncertain = false;
            let mut face_both_open = true;
            for eye in [&face.left_eye, &face.right_eye].into_iter().flatten() {
                match eye.state {
                    EyeState::Closed => {
                        closed_eye_count += 1;
                        face_has_closed = true;
                        face_both_open = false;
                    }
                    EyeState::Uncertain => {
                        uncertain_eye_count += 1;
                        face_has_uncertain = true;
                        face_both_open = false;
                    }
                    EyeState::Open => {}
                }
            }
            if face_both_open
                && face.left_eye.is_some()
                && face.right_eye.is_some()
                && !face_has_closed
                && !face_has_uncertain
            {
                open_eye_faces += 1;
            }
            if let Some(s) = face.face_sharpness {
                sharpness_values.push(s);
            }
            if let Some(expr) = &face.expression {
                if expr.confidence >= 0.45 {
                    if let Some(score) = expr.smile_score {
                        expression_values.push(score);
                    }
                }
            }
        }

        let average_face_sharpness = average(&sharpness_values);
        let weakest_face_sharpness = sharpness_values
            .iter()
            .copied()
            .min_by(|a, b| a.total_cmp(b));
        let average_expression_score = average(&expression_values);
        let likely_all_eyes_open =
            closed_eye_count == 0 && uncertain_eye_count == 0 && open_eye_faces > 0;

        GroupFaceMetrics {
            face_count: faces.len(),
            likely_all_eyes_open,
            closed_eye_count,
            uncertain_eye_count,
            average_face_sharpness,
            weakest_face_sharpness,
            average_expression_score,
            open_eye_face_count: open_eye_faces,
            mode_face_count: None,
        }
    }
}

fn average(values: &[f32]) -> Option<f32> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f32>() / values.len() as f32)
    }
}

#[derive(Debug)]
pub enum FaceAnalysisError {
    ModelUnavailable(String),
    InferenceFailed(String),
    ImageTooSmall,
}

impl std::fmt::Display for FaceAnalysisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FaceAnalysisError::ModelUnavailable(m) => write!(f, "Face model unavailable: {m}"),
            FaceAnalysisError::InferenceFailed(m) => write!(f, "Face inference failed: {m}"),
            FaceAnalysisError::ImageTooSmall => write!(f, "Image too small for face analysis"),
        }
    }
}

impl std::error::Error for FaceAnalysisError {}
