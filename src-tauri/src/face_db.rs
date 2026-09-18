use crate::face_analysis::types::{DetectedFace, EyeState, FaceAnalysis, ANALYZER_VERSION};
use sqlx::SqlitePool;

pub async fn face_analysis_complete(
    pool: &SqlitePool,
    photo_id: &str,
) -> Result<bool, sqlx::Error> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT analyzer_version FROM photo_face_analysis WHERE photo_id = ?")
            .bind(photo_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.is_some_and(|(v,)| v == ANALYZER_VERSION))
}

pub async fn copy_face_analysis_by_sha256(
    pool: &SqlitePool,
    target_photo_id: &str,
    sha256: &str,
) -> Result<bool, sqlx::Error> {
    let source: Option<String> = sqlx::query_scalar(
        "SELECT pfa.photo_id FROM photo_face_analysis pfa
         JOIN photos p ON p.id = pfa.photo_id
         WHERE p.sha256 = ? AND pfa.analyzer_version = ? AND p.id != ?
         LIMIT 1",
    )
    .bind(sha256)
    .bind(ANALYZER_VERSION)
    .bind(target_photo_id)
    .fetch_optional(pool)
    .await?;
    let Some(source_id) = source else {
        return Ok(false);
    };
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM photo_faces WHERE photo_id = ?")
        .bind(target_photo_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM photo_face_analysis WHERE photo_id = ?")
        .bind(target_photo_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO photo_face_analysis (photo_id, face_count, analyzer_version, analyzed_at)
         SELECT ?, face_count, analyzer_version, analyzed_at FROM photo_face_analysis WHERE photo_id = ?",
    )
    .bind(target_photo_id)
    .bind(&source_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO photo_faces (
            photo_id, face_index, bbox_x, bbox_y, bbox_width, bbox_height, detection_confidence,
            face_sharpness, left_eye_state, left_eye_confidence, left_eye_openness, left_eye_sharpness,
            right_eye_state, right_eye_confidence, right_eye_openness, right_eye_sharpness,
            smile_score, expression_confidence
        )
        SELECT ?, face_index, bbox_x, bbox_y, bbox_width, bbox_height, detection_confidence,
            face_sharpness, left_eye_state, left_eye_confidence, left_eye_openness, left_eye_sharpness,
            right_eye_state, right_eye_confidence, right_eye_openness, right_eye_sharpness,
            smile_score, expression_confidence
        FROM photo_faces WHERE photo_id = ?",
    )
    .bind(target_photo_id)
    .bind(&source_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE analysis SET face_count = (SELECT face_count FROM photo_face_analysis WHERE photo_id = ?) WHERE photo_id = ?")
        .bind(target_photo_id)
        .bind(target_photo_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

pub async fn save_face_analysis(
    pool: &SqlitePool,
    photo_id: &str,
    analysis: &FaceAnalysis,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM photo_faces WHERE photo_id = ?")
        .bind(photo_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM photo_face_analysis WHERE photo_id = ?")
        .bind(photo_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO photo_face_analysis (photo_id, face_count, analyzer_version) VALUES (?, ?, ?)",
    )
    .bind(photo_id)
    .bind(analysis.face_count as i64)
    .bind(&analysis.analyzer_version)
    .execute(&mut *tx)
    .await?;

    for (index, face) in analysis.faces.iter().enumerate() {
        insert_face_row(&mut tx, photo_id, index, face).await?;
    }

    sqlx::query("UPDATE analysis SET face_count = ? WHERE photo_id = ?")
        .bind(analysis.face_count as i64)
        .bind(photo_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

async fn insert_face_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    photo_id: &str,
    index: usize,
    face: &DetectedFace,
) -> Result<(), sqlx::Error> {
    let (le_state, le_conf, le_open, le_sharp) = eye_cols(&face.left_eye);
    let (re_state, re_conf, re_open, re_sharp) = eye_cols(&face.right_eye);
    let (smile, expr_conf) = face
        .expression
        .as_ref()
        .map(|e| (e.smile_score, Some(e.confidence)))
        .unwrap_or((None, None));

    sqlx::query(
        "INSERT INTO photo_faces (
            photo_id, face_index, bbox_x, bbox_y, bbox_width, bbox_height, detection_confidence,
            face_sharpness, left_eye_state, left_eye_confidence, left_eye_openness, left_eye_sharpness,
            right_eye_state, right_eye_confidence, right_eye_openness, right_eye_sharpness,
            smile_score, expression_confidence
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(photo_id)
    .bind(index as i64)
    .bind(face.bbox.x as f64)
    .bind(face.bbox.y as f64)
    .bind(face.bbox.width as f64)
    .bind(face.bbox.height as f64)
    .bind(face.detection_confidence as f64)
    .bind(face.face_sharpness.map(f64::from))
    .bind(le_state)
    .bind(le_conf)
    .bind(le_open)
    .bind(le_sharp)
    .bind(re_state)
    .bind(re_conf)
    .bind(re_open)
    .bind(re_sharp)
    .bind(smile.map(f64::from))
    .bind(expr_conf.map(f64::from))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn eye_cols(
    eye: &Option<crate::face_analysis::types::EyeAnalysis>,
) -> (Option<String>, Option<f64>, Option<f64>, Option<f64>) {
    eye.as_ref()
        .map(|e| {
            (
                Some(e.state.as_str().to_string()),
                Some(e.confidence as f64),
                e.openness_score.map(f64::from),
                e.region_sharpness.map(f64::from),
            )
        })
        .unwrap_or((None, None, None, None))
}

pub async fn load_faces_batch(
    pool: &SqlitePool,
    photo_ids: &[String],
) -> Result<std::collections::HashMap<String, Vec<DetectedFace>>, sqlx::Error> {
    if photo_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let mut builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT photo_id, face_index, bbox_x, bbox_y, bbox_width, bbox_height, detection_confidence,
                face_sharpness, left_eye_state, left_eye_confidence, left_eye_openness, left_eye_sharpness,
                right_eye_state, right_eye_confidence, right_eye_openness, right_eye_sharpness,
                smile_score, expression_confidence
         FROM photo_faces WHERE photo_id IN (",
    );
    let mut separated = builder.separated(", ");
    for id in photo_ids {
        separated.push_bind(id);
    }
    separated.push_unseparated(") ORDER BY photo_id, face_index ASC");
    let rows: Vec<FaceRowWithPhoto> = builder.build_query_as().fetch_all(pool).await?;
    let mut map = std::collections::HashMap::<String, Vec<DetectedFace>>::new();
    for row in rows {
        map.entry(row.photo_id.clone())
            .or_default()
            .push(row.into_detected());
    }
    Ok(map)
}

pub async fn load_faces_for_photo(
    pool: &SqlitePool,
    photo_id: &str,
) -> Result<Vec<DetectedFace>, sqlx::Error> {
    let rows: Vec<FaceRow> = sqlx::query_as(
        "SELECT face_index, bbox_x, bbox_y, bbox_width, bbox_height, detection_confidence,
                face_sharpness, left_eye_state, left_eye_confidence, left_eye_openness, left_eye_sharpness,
                right_eye_state, right_eye_confidence, right_eye_openness, right_eye_sharpness,
                smile_score, expression_confidence
         FROM photo_faces WHERE photo_id = ? ORDER BY face_index ASC",
    )
    .bind(photo_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(FaceRow::into_detected).collect())
}

pub async fn load_face_count(pool: &SqlitePool, photo_id: &str) -> Result<usize, sqlx::Error> {
    let count: Option<i64> =
        sqlx::query_scalar("SELECT face_count FROM photo_face_analysis WHERE photo_id = ?")
            .bind(photo_id)
            .fetch_optional(pool)
            .await?;
    Ok(count.unwrap_or(0).max(0) as usize)
}

#[derive(sqlx::FromRow)]
struct FaceRowWithPhoto {
    photo_id: String,
    #[sqlx(rename = "face_index")]
    _face_index: i64,
    bbox_x: f64,
    bbox_y: f64,
    bbox_width: f64,
    bbox_height: f64,
    detection_confidence: f64,
    face_sharpness: Option<f64>,
    left_eye_state: Option<String>,
    left_eye_confidence: Option<f64>,
    left_eye_openness: Option<f64>,
    left_eye_sharpness: Option<f64>,
    right_eye_state: Option<String>,
    right_eye_confidence: Option<f64>,
    right_eye_openness: Option<f64>,
    right_eye_sharpness: Option<f64>,
    smile_score: Option<f64>,
    expression_confidence: Option<f64>,
}

impl FaceRowWithPhoto {
    fn into_detected(self) -> DetectedFace {
        FaceRow {
            bbox_x: self.bbox_x,
            bbox_y: self.bbox_y,
            bbox_width: self.bbox_width,
            bbox_height: self.bbox_height,
            detection_confidence: self.detection_confidence,
            face_sharpness: self.face_sharpness,
            left_eye_state: self.left_eye_state,
            left_eye_confidence: self.left_eye_confidence,
            left_eye_openness: self.left_eye_openness,
            left_eye_sharpness: self.left_eye_sharpness,
            right_eye_state: self.right_eye_state,
            right_eye_confidence: self.right_eye_confidence,
            right_eye_openness: self.right_eye_openness,
            right_eye_sharpness: self.right_eye_sharpness,
            smile_score: self.smile_score,
            expression_confidence: self.expression_confidence,
        }
        .into_detected()
    }
}

#[derive(sqlx::FromRow)]
struct FaceRow {
    bbox_x: f64,
    bbox_y: f64,
    bbox_width: f64,
    bbox_height: f64,
    detection_confidence: f64,
    face_sharpness: Option<f64>,
    left_eye_state: Option<String>,
    left_eye_confidence: Option<f64>,
    left_eye_openness: Option<f64>,
    left_eye_sharpness: Option<f64>,
    right_eye_state: Option<String>,
    right_eye_confidence: Option<f64>,
    right_eye_openness: Option<f64>,
    right_eye_sharpness: Option<f64>,
    smile_score: Option<f64>,
    expression_confidence: Option<f64>,
}

impl FaceRow {
    fn into_detected(self) -> DetectedFace {
        DetectedFace {
            bbox: crate::face_analysis::types::BoundingBox {
                x: self.bbox_x as f32,
                y: self.bbox_y as f32,
                width: self.bbox_width as f32,
                height: self.bbox_height as f32,
            },
            detection_confidence: self.detection_confidence as f32,
            face_sharpness: self.face_sharpness.map(|v| v as f32),
            left_eye: self.left_eye_state.as_ref().map(|state| {
                crate::face_analysis::types::EyeAnalysis {
                    visibility_confidence: self.left_eye_confidence.unwrap_or(0.0) as f32,
                    openness_score: self.left_eye_openness.map(|v| v as f32),
                    state: EyeState::from_db(state),
                    confidence: self.left_eye_confidence.unwrap_or(0.0) as f32,
                    region_sharpness: self.left_eye_sharpness.map(|v| v as f32),
                }
            }),
            right_eye: self.right_eye_state.as_ref().map(|state| {
                crate::face_analysis::types::EyeAnalysis {
                    visibility_confidence: self.right_eye_confidence.unwrap_or(0.0) as f32,
                    openness_score: self.right_eye_openness.map(|v| v as f32),
                    state: EyeState::from_db(state),
                    confidence: self.right_eye_confidence.unwrap_or(0.0) as f32,
                    region_sharpness: self.right_eye_sharpness.map(|v| v as f32),
                }
            }),
            expression: self.smile_score.map(|score| {
                crate::face_analysis::types::ExpressionAnalysis {
                    smile_score: Some(score as f32),
                    confidence: self.expression_confidence.unwrap_or(0.0) as f32,
                }
            }),
        }
    }
}
