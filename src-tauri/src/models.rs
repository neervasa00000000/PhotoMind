use serde::{Deserialize, Serialize};

/// A RAW file and its identical-shot JPEG, counted as ONE logical photo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogicalPair {
    pub primary: Photo,
    pub paired: Photo,
    pub match_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Photo {
    pub id: String,
    pub is_kept: bool,
    pub absolute_path: String,
    pub filename: String,
    pub extension: String,
    pub file_size: i64,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub created_at: Option<String>,
    pub modified_at: Option<String>,
    pub capture_time: Option<String>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub orientation: Option<i64>,
    pub gps_available: bool,
    pub sha256: Option<String>,
    pub perceptual_hash: Option<String>,
    pub thumbnail_path: Option<String>,
    pub analysis_status: String,
    pub moment_id: Option<String>,
    pub sharpness: Option<f64>,
    pub exposure: Option<f64>,
    pub contrast: Option<f64>,
    pub highlight_clipping: Option<f64>,
    pub shadow_clipping: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DuplicateGroup {
    pub sha256: String,
    pub count: i64,
    pub kind: String,
    pub photos: Vec<Photo>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct PhotoIndex {
    pub id: String,
    pub absolute_path: String,
    pub filename: String,
    pub file_size: i64,
    pub created_at: Option<String>,
    pub capture_time: Option<String>,
    pub sha256: Option<String>,
    pub perceptual_hash: Option<String>,
    pub thumbnail_path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct MomentGroup {
    pub id: String,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub photo_count: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DashboardStats {
    pub total_photos: i64,
    pub total_bytes: i64,
    pub exact_duplicates: i64,
    pub exact_duplicates_bytes: i64,
    pub recommended_removals: i64,
    pub recommended_removals_bytes: i64,
    pub needs_review: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MomentGroupWithPhotos {
    pub moment: MomentGroup,
    pub photos: Vec<Photo>,
    pub recommendations: Vec<Recommendation>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct Recommendation {
    pub photo_id: String,
    pub moment_id: Option<String>,
    pub decision: String,
    pub confidence: f64,
    pub reasoning: String, // Stored as JSON string in DB
    pub compared_to: Option<String>,
    #[serde(default)]
    pub reason_codes: Option<String>,
    #[serde(default)]
    pub best_of_group: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhotoFaceSummary {
    pub photo_id: String,
    pub face_count: usize,
    pub closed_eye_warning: bool,
    pub uncertain_eyes: bool,
    pub average_face_sharpness: Option<f32>,
    pub average_smile: Option<f32>,
}

#[derive(Debug, Serialize)]
pub struct AiAnalysis {
    pub photo: Photo,
    pub model: String,
    pub analysis_json: String,
    pub analyzed_at: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
pub struct AiAnalysisRow {
    pub id: String,
    pub is_kept: bool,
    pub absolute_path: String,
    pub filename: String,
    pub extension: String,
    pub file_size: i64,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub created_at: Option<String>,
    pub modified_at: Option<String>,
    pub capture_time: Option<String>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub orientation: Option<i64>,
    pub gps_available: bool,
    pub sha256: Option<String>,
    pub perceptual_hash: Option<String>,
    pub thumbnail_path: Option<String>,
    pub analysis_status: String,
    pub moment_id: Option<String>,
    pub sharpness: Option<f64>,
    pub exposure: Option<f64>,
    pub contrast: Option<f64>,
    pub highlight_clipping: Option<f64>,
    pub shadow_clipping: Option<f64>,
    pub analysis_json: String,
    pub model: String,
    pub analyzed_at: Option<String>,
}

impl AiAnalysisRow {
    pub fn into_analysis(self) -> AiAnalysis {
        let photo = Photo {
            id: self.id,
            is_kept: self.is_kept,
            absolute_path: self.absolute_path,
            filename: self.filename,
            extension: self.extension,
            file_size: self.file_size,
            width: self.width,
            height: self.height,
            created_at: self.created_at,
            modified_at: self.modified_at,
            capture_time: self.capture_time,
            camera_make: self.camera_make,
            camera_model: self.camera_model,
            orientation: self.orientation,
            gps_available: self.gps_available,
            sha256: self.sha256,
            perceptual_hash: self.perceptual_hash,
            thumbnail_path: self.thumbnail_path,
            analysis_status: self.analysis_status,
            moment_id: self.moment_id,
            sharpness: self.sharpness,
            exposure: self.exposure,
            contrast: self.contrast,
            highlight_clipping: self.highlight_clipping,
            shadow_clipping: self.shadow_clipping,
        };
        AiAnalysis {
            photo,
            model: self.model,
            analysis_json: self.analysis_json,
            analyzed_at: self.analyzed_at,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct BinPhoto {
    pub photo: Photo,
    pub original_path: String,
    pub deleted_at: String,
    pub available: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct FileActionReport {
    pub photo_ids: Vec<String>,
    pub bytes: i64,
    pub errors: Vec<String>,
}
