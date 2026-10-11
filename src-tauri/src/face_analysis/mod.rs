pub mod sharpness;
#[cfg(test)]
mod tests_logic;
pub mod types;
pub mod yunet;

use image::RgbImage;
use std::panic::{catch_unwind, AssertUnwindSafe};
use types::{FaceAnalysis, FaceAnalysisError};

pub use types::{
    BoundingBox, DetectedFace, ExpressionAnalysis, EyeAnalysis, EyeState, GroupFaceMetrics,
    ANALYZER_VERSION,
};
pub use yunet::{detect_face_geometry, eye_patch_rect, FaceGeometry};

pub trait FaceAnalyzer {
    fn analyze(&self, image: &yunet::AnalysisImage) -> Result<FaceAnalysis, FaceAnalysisError>;
}

pub struct LocalFaceAnalyzer;

impl FaceAnalyzer for LocalFaceAnalyzer {
    fn analyze(&self, image: &yunet::AnalysisImage) -> Result<FaceAnalysis, FaceAnalysisError> {
        yunet::analyze_yunet(image)
    }
}

/// Run face analysis on a normalized RGB preview. A failure must remain
/// retryable instead of being persisted as a successful zero-face result.
pub fn analyze_rgb_preview(rgb: RgbImage) -> Result<FaceAnalysis, FaceAnalysisError> {
    let image = yunet::AnalysisImage::from_rgb(rgb);
    let analyzer = LocalFaceAnalyzer;
    match catch_unwind(AssertUnwindSafe(|| analyzer.analyze(&image))) {
        Ok(Ok(analysis)) => Ok(analysis),
        Ok(Err(error)) => {
            tracing::warn!(stage = "face", error = %error, "Face analysis failed");
            Err(error)
        }
        Err(_) => {
            tracing::warn!(stage = "face", "Face analysis panicked");
            Err(FaceAnalysisError::InferenceFailed(
                "Face analysis panicked".into(),
            ))
        }
    }
}
