//! Group face intelligence from persisted local face analysis (not Ollama).

use crate::face_analysis::types::{EyeState, GroupFaceMetrics};
use crate::face_db;
use serde::Serialize;
use sqlx::SqlitePool;

#[derive(Debug, Serialize, Default, PartialEq)]
pub struct FaceTally {
    pub total_faces: usize,
    pub open_eyes: usize,
    pub closed_eyes: usize,
    pub partial_eyes: usize,
    pub unclear_eyes: usize,
    pub looking_at_camera: usize,
    pub min_quality: Option<f64>,
    pub max_quality: Option<f64>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct MomentFaceSummary {
    pub moment_id: String,
    pub photos_with_results: usize,
    pub faces: FaceTally,
    pub best_group_shot: Option<String>,
}

impl FaceTally {
    fn add_metrics(
        &mut self,
        metrics: &GroupFaceMetrics,
        faces: &[crate::face_analysis::types::DetectedFace],
    ) {
        self.total_faces += metrics.face_count;
        for face in faces {
            for eye in [&face.left_eye, &face.right_eye].into_iter().flatten() {
                match eye.state {
                    EyeState::Open => self.open_eyes += 1,
                    EyeState::Closed => self.closed_eyes += 1,
                    EyeState::Uncertain => self.unclear_eyes += 1,
                }
            }
            if let Some(q) = face.face_sharpness {
                let q = f64::from(q);
                self.min_quality = Some(self.min_quality.map_or(q, |m| m.min(q)));
                self.max_quality = Some(self.max_quality.map_or(q, |m| m.max(q)));
            }
        }
    }
}

fn group_shot_score(
    metrics: &GroupFaceMetrics,
    faces: &[crate::face_analysis::types::DetectedFace],
) -> f64 {
    let mut score = metrics.open_eye_face_count as f64 * 3.0;
    score -= metrics.closed_eye_count as f64 * 2.0;
    score += metrics.face_count as f64;
    if let Some(s) = metrics.average_face_sharpness {
        score += f64::from(s) * 2.0;
    }
    if let Some(s) = metrics.average_expression_score {
        score += f64::from(s);
    }
    let _ = faces;
    score
}

pub async fn moment_face_summary(
    pool: &SqlitePool,
    moment_id: &str,
) -> Result<MomentFaceSummary, String> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT p.id FROM photos p
          WHERE p.moment_id = ? AND NOT EXISTS (SELECT 1 FROM bin_entries b WHERE b.photo_id = p.id)",
    )
    .bind(moment_id)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    let mut tally = FaceTally::default();
    let mut best: Option<(String, f64)> = None;
    let mut parsed = 0usize;

    for photo_id in rows {
        let faces = face_db::load_faces_for_photo(pool, &photo_id)
            .await
            .map_err(|e| e.to_string())?;
        let has_row = face_db::load_face_count(pool, &photo_id)
            .await
            .map_err(|e| e.to_string())?;
        if has_row == 0 && faces.is_empty() {
            let complete: Option<i64> =
                sqlx::query_scalar("SELECT 1 FROM photo_face_analysis WHERE photo_id = ? LIMIT 1")
                    .bind(&photo_id)
                    .fetch_optional(pool)
                    .await
                    .map_err(|e| e.to_string())?;
            if complete.is_none() {
                continue;
            }
        }
        parsed += 1;
        let metrics = GroupFaceMetrics::from_faces(&faces);
        if metrics.face_count == 0 {
            continue;
        }
        tally.add_metrics(&metrics, &faces);
        let score = group_shot_score(&metrics, &faces);
        if best.as_ref().map(|(_, s)| score > *s).unwrap_or(true) {
            best = Some((photo_id, score));
        }
    }

    Ok(MomentFaceSummary {
        moment_id: moment_id.to_string(),
        photos_with_results: parsed,
        faces: tally,
        best_group_shot: best.map(|(id, _)| id),
    })
}
