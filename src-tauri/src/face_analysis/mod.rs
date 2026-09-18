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

/// Run face analysis on a normalized RGB preview; failures never panic outward.
pub fn analyze_rgb_preview(rgb: RgbImage) -> FaceAnalysis {
    let image = yunet::AnalysisImage::from_rgb(rgb);
    let analyzer = LocalFaceAnalyzer;
    match catch_unwind(AssertUnwindSafe(|| analyzer.analyze(&image))) {
        Ok(Ok(analysis)) => analysis,
        Ok(Err(error)) => {
            tracing::warn!(stage = "face", error = %error, "Face analysis failed");
            FaceAnalysis {
                face_count: 0,
                faces: Vec::new(),
                analyzer_version: ANALYZER_VERSION.into(),
            }
        }
        Err(_) => {
            tracing::warn!(stage = "face", "Face analysis panicked");
            FaceAnalysis {
                face_count: 0,
                faces: Vec::new(),
                analyzer_version: ANALYZER_VERSION.into(),
            }
        }
    }
}
