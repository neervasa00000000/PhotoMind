use crate::models::Photo;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    SqlitePool,
};
use std::path::Path;

pub async fn init_db(app_data_dir: &Path) -> Result<SqlitePool, Box<dyn std::error::Error>> {
    let db_path = app_data_dir.join("photomind.db");

    // Create directory if it doesn't exist
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // WAL permits readers alongside the bounded scan workers; wait for the short
    // write transactions instead of surfacing transient "database is locked" errors.
    let options = SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(std::time::Duration::from_secs(10))
        .foreign_keys(true)
        .pragma("journal_size_limit", "67108864");
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    // Connection-scoped settings above apply to every pooled connection. The
    // journal size limit governs checkpoint truncation, not live WAL growth.

    // Run schema creation
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS photos (
            id TEXT PRIMARY KEY,
            absolute_path TEXT UNIQUE,
            filename TEXT,
            extension TEXT,
            file_size INTEGER,
            width INTEGER,
            height INTEGER,
            created_at DATETIME,
            modified_at DATETIME,
            capture_time DATETIME,
            camera_make TEXT,
            camera_model TEXT,
            orientation INTEGER,
            gps_available BOOLEAN,
            sha256 TEXT,
            perceptual_hash TEXT,
            thumbnail_path TEXT,
            analysis_status TEXT,
            moment_id TEXT
        );
        CREATE TABLE IF NOT EXISTS moment_groups (
            id TEXT PRIMARY KEY,
            start_time DATETIME,
            end_time DATETIME,
            photo_count INTEGER
        );
        CREATE TABLE IF NOT EXISTS analysis (
            photo_id TEXT PRIMARY KEY,
            sharpness REAL,
            motion_blur REAL,
            exposure REAL,
            highlight_clipping REAL,
            shadow_clipping REAL,
            contrast REAL,
            noise_estimate REAL,
            scene_type TEXT,
            face_count INTEGER,
            aesthetic_score REAL,
            mean_saturation REAL,
            flat_fraction REAL,
            junk_category TEXT,
            junk_confidence REAL,
            FOREIGN KEY(photo_id) REFERENCES photos(id)
        );
        CREATE TABLE IF NOT EXISTS scan_sessions (
            id INTEGER PRIMARY KEY,
            started_at DATETIME,
            completed_at DATETIME,
            status TEXT,
            total_files INTEGER,
            processed_files INTEGER,
            phase TEXT,
            scan_path TEXT
        );
        CREATE TABLE IF NOT EXISTS scan_errors (
            id INTEGER PRIMARY KEY,
            session_id INTEGER,
            absolute_path TEXT,
            stage TEXT,
            reason TEXT,
            error_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY(session_id) REFERENCES scan_sessions(id)
        );
        CREATE INDEX IF NOT EXISTS idx_scan_errors_session ON scan_errors(session_id);
        CREATE TABLE IF NOT EXISTS logical_photos (
            primary_id TEXT PRIMARY KEY,
            paired_id TEXT UNIQUE,
            match_reason TEXT,
            FOREIGN KEY(primary_id) REFERENCES photos(id) ON DELETE CASCADE,
            FOREIGN KEY(paired_id) REFERENCES photos(id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS user_decisions (
            id INTEGER PRIMARY KEY,
            photo_id TEXT,
            recommended_decision TEXT,
            actual_decision TEXT,
            timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY(photo_id) REFERENCES photos(id)
        );
        CREATE TABLE IF NOT EXISTS recommendations (
            photo_id TEXT PRIMARY KEY,
            moment_id TEXT,
            decision TEXT,
            confidence REAL,
            reasoning TEXT,
            compared_to TEXT,
            FOREIGN KEY(photo_id) REFERENCES photos(id)
        );
        CREATE TABLE IF NOT EXISTS bin_entries (
            photo_id TEXT PRIMARY KEY,
            original_path TEXT NOT NULL,
            trash_path TEXT,
            deleted_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            state TEXT NOT NULL DEFAULT 'pending',
            FOREIGN KEY(photo_id) REFERENCES photos(id)
        );
        CREATE TABLE IF NOT EXISTS protected_photos (
            photo_id TEXT PRIMARY KEY,
            FOREIGN KEY(photo_id) REFERENCES photos(id)
        );
        CREATE TABLE IF NOT EXISTS photo_ai_results (
            photo_id TEXT PRIMARY KEY,
            model TEXT,
            analysis_json TEXT NOT NULL,
            analyzed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY(photo_id) REFERENCES photos(id)
        );
        CREATE INDEX IF NOT EXISTS idx_photos_sha256 ON photos(sha256);
        CREATE INDEX IF NOT EXISTS idx_photos_capture_time ON photos(capture_time);
        CREATE INDEX IF NOT EXISTS idx_photos_moment_id ON photos(moment_id);
        CREATE TABLE IF NOT EXISTS photo_face_analysis (
            photo_id TEXT PRIMARY KEY,
            face_count INTEGER NOT NULL DEFAULT 0,
            analyzer_version TEXT NOT NULL,
            analyzed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY(photo_id) REFERENCES photos(id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS photo_faces (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            photo_id TEXT NOT NULL,
            face_index INTEGER NOT NULL,
            bbox_x REAL NOT NULL,
            bbox_y REAL NOT NULL,
            bbox_width REAL NOT NULL,
            bbox_height REAL NOT NULL,
            detection_confidence REAL NOT NULL,
            face_sharpness REAL,
            left_eye_state TEXT,
            left_eye_confidence REAL,
            left_eye_openness REAL,
            left_eye_sharpness REAL,
            right_eye_state TEXT,
            right_eye_confidence REAL,
            right_eye_openness REAL,
            right_eye_sharpness REAL,
            smile_score REAL,
            expression_confidence REAL,
            FOREIGN KEY(photo_id) REFERENCES photos(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_photo_faces_photo ON photo_faces(photo_id);
        "#,
    )
    .execute(&pool)
    .await?;

    // Idempotent migrations for databases created before a column existed.
    let junk_column = ensure_analysis_column(&pool, "junk_category").await?;
    ensure_analysis_column(&pool, "junk_confidence").await?;
    for column in [
        "sharpness",
        "motion_blur",
        "exposure",
        "highlight_clipping",
        "shadow_clipping",
        "contrast",
        "noise_estimate",
        "scene_type",
        "face_count",
        "aesthetic_score",
        "mean_saturation",
        "flat_fraction",
    ] {
        ensure_analysis_column(&pool, column).await?;
    }
    if junk_column {
        sqlx::query(
            "DELETE FROM analysis WHERE junk_category IS NOT NULL AND junk_confidence IS NULL",
        )
        .execute(&pool)
        .await?;
    }
    ensure_scan_session_column(&pool, "phase").await?;
    ensure_scan_session_column(&pool, "scan_path").await?;
    ensure_recommendation_column(&pool, "reason_codes").await?;
    ensure_recommendation_column(&pool, "best_of_group").await?;
    fix_best_of_group_column_type(&pool).await?;

    Ok(pool)
}

async fn fix_best_of_group_column_type(pool: &SqlitePool) -> Result<(), Box<dyn std::error::Error>> {
    // Check if best_of_group exists and is TEXT instead of INTEGER
    let col_type: Option<String> = sqlx::query_scalar(
        "SELECT type FROM pragma_table_info('recommendations') WHERE name = 'best_of_group'"
    )
    .fetch_optional(pool)
    .await?;
    
    if col_type.as_deref() == Some("TEXT") {
        // Column exists but has wrong type; SQLite doesn't support ALTER COLUMN,
        // so we need to recreate the recommendations table
        let mut tx = pool.begin().await?;
        
        // Create new table with correct schema
        sqlx::query(
            "CREATE TABLE recommendations_new (
                photo_id TEXT PRIMARY KEY,
                moment_id TEXT,
                decision TEXT,
                confidence REAL,
                reasoning TEXT,
                compared_to TEXT,
                reason_codes TEXT,
                best_of_group INTEGER,
                FOREIGN KEY(photo_id) REFERENCES photos(id)
            )"
        )
        .execute(&mut *tx)
        .await?;
        
        // Copy data, converting TEXT to INTEGER for best_of_group
        sqlx::query(
            "INSERT INTO recommendations_new 
             SELECT photo_id, moment_id, decision, confidence, reasoning, compared_to, 
                    reason_codes, 
                    CASE WHEN best_of_group IS NOT NULL AND best_of_group != '' 
                         THEN CAST(best_of_group AS INTEGER) 
                         ELSE NULL 
                    END
             FROM recommendations"
        )
        .execute(&mut *tx)
        .await?;
        
        // Drop old table and rename new one
        sqlx::query("DROP TABLE recommendations").execute(&mut *tx).await?;
        sqlx::query("ALTER TABLE recommendations_new RENAME TO recommendations")
            .execute(&mut *tx)
            .await?;
        
        tx.commit().await?;
    }
    Ok(())
}

async fn ensure_recommendation_column(
    pool: &SqlitePool,
    column: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('recommendations') WHERE name = ?",
    )
    .bind(column)
    .fetch_one(pool)
    .await?;
    if exists == 0 {
        let col_type = if column == "best_of_group" { "INTEGER" } else { "TEXT" };
        let sql = format!("ALTER TABLE recommendations ADD COLUMN {column} {col_type}");
        sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
            .execute(pool)
            .await?;
    }
    Ok(())
}

async fn ensure_scan_session_column(
    pool: &SqlitePool,
    column: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('scan_sessions') WHERE name = ?",
    )
    .bind(column)
    .fetch_one(pool)
    .await?;
    if exists == 0 {
        let sql = format!("ALTER TABLE scan_sessions ADD COLUMN {column} TEXT");
        // Column names are drawn from a hardcoded allow-list above, never user input.
        sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// A scan left in 'running' after a crash/immediate quit cannot resume its
/// in-flight batch safely. Mark it interrupted so the UI never shows a
/// half-finished scan as complete; the next scan continues on unchanged files.
pub async fn mark_interrupted_sessions(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE scan_sessions SET status = 'interrupted' WHERE status = 'running'")
        .execute(pool)
        .await?;
    Ok(())
}

/// Persist why an individual file could not be indexed. Never aborts a scan:
/// failures are recorded per file so a corrupt image cannot hide in silence.
pub async fn record_scan_error(
    pool: &SqlitePool,
    session_id: i64,
    absolute_path: &str,
    stage: &str,
    reason: &str,
) -> Result<(), sqlx::Error> {
    let reason = reason.chars().take(500).collect::<String>();
    sqlx::query(
        "INSERT INTO scan_errors (session_id, absolute_path, stage, reason) VALUES (?, ?, ?, ?)",
    )
    .bind(session_id)
    .bind(absolute_path)
    .bind(stage)
    .bind(reason)
    .execute(pool)
    .await?;
    Ok(())
}

/// Conservative RAW+JPEG pairing: a camera often writes both an identical-shot
/// JPEG and its RAW into the same folder. Rather than asking the user to cull
/// the same frame twice, the two files become ONE logical photo (RAW primary).
///
/// A pair is only formed when ALL of these hold:
///   - one file is a camera RAW, the other a JPEG
///   - same folder and identical filename stem (case-insensitive)
///   - different file sizes (so an identical copy is never "paired" with its duplicate)
///   - capture times (when both are known) are within a 120-second window
///
/// Deterministic given `photos` order. No photo is ever in more than one pair.
pub fn conservative_pairs(photos: &[Photo]) -> Vec<(String, String, String)> {
    use std::collections::{BTreeMap, HashSet};
    let mut by_group: BTreeMap<(String, String), Vec<&Photo>> = BTreeMap::new();
    for photo in photos {
        let path = std::path::Path::new(&photo.absolute_path);
        let dir = path
            .parent()
            .map(|d| d.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !dir.is_empty() && !stem.is_empty() {
            by_group.entry((dir, stem)).or_default().push(photo);
        }
    }
    let mut pairs = Vec::new();
    let mut used: HashSet<&str> = HashSet::new();
    for group in by_group.values() {
        for raw in group.iter().filter(|p| is_raw_extension(&p.extension)) {
            if used.contains(raw.id.as_str()) {
                continue;
            }
            let paired = group.iter().find(|j| {
                j.id != raw.id
                    && is_jpeg_extension(&j.extension)
                    && !used.contains(j.id.as_str())
                    && raw.file_size != j.file_size
                    && capture_times_compatible(raw, j)
            });
            if let Some(paired) = paired {
                let reason = if raw.capture_time.is_some() && paired.capture_time.is_some() {
                    "Same folder, identical filename stem (RAW + JPEG), capture times within 120s"
                } else {
                    "Same folder and identical filename stem (RAW + JPEG)"
                };
                pairs.push((raw.id.clone(), paired.id.clone(), reason.to_string()));
                used.insert(raw.id.as_str());
                used.insert(paired.id.as_str());
            }
        }
    }
    pairs
}

fn is_raw_extension(extension: &str) -> bool {
    crate::ingest::Format::from_extension(extension).is_some_and(|f| f.is_raw())
}

fn is_jpeg_extension(extension: &str) -> bool {
    matches!(&extension.to_lowercase()[..], "jpg" | "jpeg")
}

fn capture_times_compatible(a: &Photo, b: &Photo) -> bool {
    match (&a.capture_time, &b.capture_time) {
        (Some(x), Some(y)) => {
            match (
                chrono::DateTime::parse_from_rfc3339(x),
                chrono::DateTime::parse_from_rfc3339(y),
            ) {
                (Ok(x), Ok(y)) => (x - y).num_seconds().abs() <= 120,
                _ => true, // unparseable timestamps never block a pairing
            }
        }
        _ => true,
    }
}

/// Rebuild the pairing table from the current library. Also drops pairs whose
/// photos have been binned or re-indexed away, so the table never goes stale.
pub async fn build_logical_pairs(pool: &SqlitePool) -> Result<usize, sqlx::Error> {
    let photos = all_photos(pool).await?;
    let pairs = conservative_pairs(&photos);
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM logical_photos")
        .execute(&mut *tx)
        .await?;
    for (primary_id, paired_id, reason) in &pairs {
        sqlx::query(
            "INSERT INTO logical_photos (primary_id, paired_id, match_reason) VALUES (?, ?, ?)",
        )
        .bind(primary_id)
        .bind(paired_id)
        .bind(reason)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(pairs.len())
}

/// Both photo ids that take part in any logical pair (primary AND paired).
pub async fn logical_pair_photo_ids(
    pool: &SqlitePool,
) -> Result<std::collections::HashSet<String>, sqlx::Error> {
    let ids: Vec<(String, String)> =
        sqlx::query_as("SELECT primary_id, paired_id FROM logical_photos")
            .fetch_all(pool)
            .await?;
    Ok(ids.into_iter().flat_map(|(a, b)| [a, b]).collect())
}

/// Full pair records with both decoded `Photo` rows joined from the library.
pub async fn get_logical_pairs(
    pool: &SqlitePool,
) -> Result<Vec<crate::models::LogicalPair>, sqlx::Error> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT primary_id, paired_id, match_reason FROM logical_photos ORDER BY primary_id",
    )
    .fetch_all(pool)
    .await?;
    let mut pairs = Vec::with_capacity(rows.len());
    for (primary_id, paired_id, reason) in rows {
        let (Some(primary), Some(paired)) = (
            photo_by_id(pool, &primary_id).await?,
            photo_by_id(pool, &paired_id).await?,
        ) else {
            continue;
        };
        pairs.push(crate::models::LogicalPair {
            primary,
            paired,
            match_reason: reason,
        });
    }
    Ok(pairs)
}

async fn photo_by_id(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<crate::models::Photo>, sqlx::Error> {
    sqlx::QueryBuilder::<sqlx::Sqlite>::new(PHOTO_SELECT)
        .push(" WHERE p.id = ?")
        .build_query_as::<crate::models::Photo>()
        .bind(id)
        .fetch_optional(pool)
        .await
}

async fn ensure_analysis_column(
    pool: &SqlitePool,
    column: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    let exists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info('analysis') WHERE name = ?")
            .bind(column)
            .fetch_one(pool)
            .await?;
    if exists == 0 {
        let sql = format!(
            "ALTER TABLE analysis ADD COLUMN {column} {}",
            if column == "junk_category" || column == "scene_type" {
                "TEXT"
            } else if column == "face_count" {
                "INTEGER"
            } else {
                "REAL"
            }
        );
        // Column names are drawn from a hardcoded allow-list above, never user input.
        sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
            .execute(pool)
            .await?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Prune stale metadata without deleting originals or touching recoverable Bin entries.
/// An unmounted external volume is unavailable, rather than proof of deletion.
pub async fn reconcile_active_index(pool: &SqlitePool) -> Result<(), Box<dyn std::error::Error>> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT id, absolute_path FROM photos WHERE absolute_path IS NOT NULL AND NOT EXISTS (SELECT 1 FROM bin_entries b WHERE b.photo_id = photos.id)")
        .fetch_all(pool).await?;
    let stale = tokio::task::spawn_blocking(move || {
        rows.into_iter()
            .filter_map(|(id, path)| {
                let path = Path::new(&path);
                if path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("._"))
                {
                    return Some(id);
                }
                #[cfg(target_os = "macos")]
                if path.starts_with("/Volumes") {
                    if let Some(volume) = path.components().nth(2) {
                        if !Path::new("/Volumes").join(volume.as_os_str()).exists() {
                            return None;
                        }
                    }
                }
                match std::fs::metadata(path) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(id),
                    _ => None,
                }
            })
            .collect::<Vec<_>>()
    })
    .await?;
    let mut tx = pool.begin().await?;
    for id in stale {
        sqlx::query("DELETE FROM recommendations WHERE photo_id = ? OR moment_id IN (SELECT moment_id FROM photos WHERE id = ?)")
            .bind(&id).bind(&id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM logical_photos WHERE primary_id = ? OR paired_id = ?")
            .bind(&id)
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        for sql in [
            "DELETE FROM user_decisions WHERE photo_id = ?",
            "DELETE FROM analysis WHERE photo_id = ?",
            "DELETE FROM photo_ai_results WHERE photo_id = ?",
            "DELETE FROM protected_photos WHERE photo_id = ?",
            "DELETE FROM photos WHERE id = ?",
        ] {
            sqlx::query(sql).bind(&id).execute(&mut *tx).await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Explicit "start over" action: clear the current scan library and analysis
/// without losing Bin recovery records or originals. Kept as an explicit reset operation.
pub async fn reset_scan_library(
    pool: &SqlitePool,
    thumb_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM logical_photos")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM recommendations")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "DELETE FROM user_decisions WHERE photo_id NOT IN (SELECT photo_id FROM bin_entries)",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM analysis WHERE photo_id NOT IN (SELECT photo_id FROM bin_entries)")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "DELETE FROM photo_ai_results WHERE photo_id NOT IN (SELECT photo_id FROM bin_entries)",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM protected_photos WHERE photo_id NOT IN (SELECT photo_id FROM bin_entries)",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM photos WHERE id NOT IN (SELECT photo_id FROM bin_entries)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE photos SET moment_id = NULL")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM moment_groups")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM scan_errors")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM scan_sessions")
        .execute(&mut *tx)
        .await?;
    let retained: Vec<String> =
        sqlx::query_scalar("SELECT thumbnail_path FROM photos WHERE thumbnail_path IS NOT NULL")
            .fetch_all(&mut *tx)
            .await?;
    tx.commit().await?;
    let retained: std::collections::HashSet<std::path::PathBuf> = retained
        .into_iter()
        .map(|p| std::fs::canonicalize(&p).unwrap_or_else(|_| p.into()))
        .collect();
    // Only remove generated files inside the app's thumbnail directory. Shared Bin
    // thumbnails survive even when their active-library copy has been cleared.
    if thumb_dir.is_dir() {
        for entry in std::fs::read_dir(thumb_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && !retained
                    .contains(&std::fs::canonicalize(entry.path()).unwrap_or_else(|_| entry.path()))
            {
                std::fs::remove_file(entry.path())?;
            }
        }
    }
    Ok(())
}

/// Remove cached thumbnail files no longer referenced by any indexed photo.
/// The library itself (photos, analyses, AI results, user decisions) is kept
/// so scans resume and learning survives restarts.
pub async fn gc_orphaned_thumbnails(
    pool: &SqlitePool,
    thumb_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let retained: std::collections::HashSet<std::path::PathBuf> =
        sqlx::query_scalar("SELECT thumbnail_path FROM photos WHERE thumbnail_path IS NOT NULL")
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|p: String| std::fs::canonicalize(&p).unwrap_or_else(|_| p.into()))
            .collect();
    if !thumb_dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(thumb_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        let key = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !retained.contains(&key) {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

pub const PHOTO_SELECT: &str = r#"SELECT
    p.id, p.absolute_path, p.filename, p.extension, p.file_size, p.width, p.height,
    p.created_at, p.modified_at, p.capture_time, p.camera_make, p.camera_model,
    p.orientation, p.gps_available, p.sha256, p.perceptual_hash, p.thumbnail_path,
    p.analysis_status, p.moment_id,
    a.sharpness, a.exposure, a.contrast, a.highlight_clipping, a.shadow_clipping,
    EXISTS(SELECT 1 FROM protected_photos k WHERE k.photo_id = p.id) AS is_kept
    FROM photos p LEFT JOIN analysis a ON p.id = a.photo_id"#;

/// Bounded pages with a unique tie breaker so equal timestamps never skip photos.
pub async fn photo_page(
    pool: &SqlitePool,
    offset: i64,
    limit: i64,
) -> Result<Vec<crate::models::Photo>, sqlx::Error> {
    sqlx::QueryBuilder::<sqlx::Sqlite>::new(PHOTO_SELECT).push(" WHERE NOT EXISTS (SELECT 1 FROM bin_entries b WHERE b.photo_id = p.id) ORDER BY COALESCE(p.capture_time, p.created_at) DESC, p.id ASC LIMIT ? OFFSET ?").build_query_as()
        .bind(limit.clamp(1, 500)).bind(offset.max(0)).fetch_all(pool).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn init_db_migrates_a_legacy_analysis_table_with_junk_columns() {
        let dir = std::env::temp_dir().join(format!("photomind-migrate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("photomind.db");
        {
            let legacy = SqlitePool::connect(&format!("sqlite:{}?mode=rwc", db_path.display()))
                .await
                .unwrap();
            sqlx::query("CREATE TABLE analysis (photo_id TEXT PRIMARY KEY, sharpness REAL)")
                .execute(&legacy)
                .await
                .unwrap();
            sqlx::query("INSERT INTO analysis (photo_id, sharpness) VALUES ('p1', 0.5)")
                .execute(&legacy)
                .await
                .unwrap();
            legacy.close().await;
        }
        let pool = init_db(&dir).await.unwrap();
        let cols: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('analysis') ORDER BY name")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(
            cols.contains(&"junk_category".to_string()),
            "missing junk_category: {cols:?}"
        );
        assert!(
            cols.contains(&"junk_confidence".to_string()),
            "missing junk_confidence: {cols:?}"
        );
        for expected in ["mean_saturation", "flat_fraction", "exposure", "contrast"] {
            assert!(cols.iter().any(|c| c == expected), "missing {expected}");
        }
        let image_path = dir.join("migration-photo.png");
        image::RgbImage::from_pixel(100, 80, image::Rgb([90, 120, 70]))
            .save(&image_path)
            .unwrap();
        crate::scanner::refresh_photo(&pool, &image_path, &dir.join("thumbs"))
            .await
            .unwrap();
        assert_eq!(all_photos(&pool).await.unwrap().len(), 1);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM analysis")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 2);
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn fresh_launch_clears_scan_results_but_preserves_bin_and_originals() {
        let dir = std::env::temp_dir().join(format!("photomind-reset-{}", uuid::Uuid::new_v4()));
        let pool = init_db(&dir).await.unwrap();
        let thumbs = dir.join("thumbnails");
        std::fs::create_dir_all(&thumbs).unwrap();
        let shared = thumbs.join("shared.jpg");
        let stale = thumbs.join("stale.jpg");
        let original = dir.join("original.jpg");
        std::fs::write(&shared, b"shared preview").unwrap();
        std::fs::write(&stale, b"stale preview").unwrap();
        std::fs::write(&original, b"original photo").unwrap();
        for (id, preview) in [("active", &shared), ("binned", &shared), ("stale", &stale)] {
            sqlx::query("INSERT INTO photos (id, absolute_path, thumbnail_path, moment_id) VALUES (?, ?, ?, 'old-moment')")
                .bind(id).bind(format!("/{id}.jpg")).bind(std::fs::canonicalize(preview).unwrap().to_string_lossy().as_ref()).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO analysis (photo_id) VALUES (?)")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO protected_photos (photo_id) VALUES (?)")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO recommendations (photo_id) VALUES (?)")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO bin_entries (photo_id, original_path, trash_path, state) VALUES ('binned', '/original-location.jpg', '/trash-location.jpg', 'active')").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO moment_groups (id) VALUES ('old-moment')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO scan_sessions (status) VALUES ('running')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO scan_errors (session_id, reason) SELECT id, 'unsupported file' FROM scan_sessions").execute(&pool).await.unwrap();
        reset_scan_library(&pool, &thumbs).await.unwrap();
        assert!(all_photos(&pool).await.unwrap().is_empty());
        let remaining: Vec<String> = sqlx::query_scalar("SELECT id FROM photos")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, vec!["binned"]);
        let entry: (String, String) =
            sqlx::query_as("SELECT original_path, trash_path FROM bin_entries")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            entry,
            (
                "/original-location.jpg".into(),
                "/trash-location.jpg".into()
            )
        );
        let counts: (i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM recommendations), (SELECT COUNT(*) FROM moment_groups), (SELECT COUNT(*) FROM scan_sessions), (SELECT COUNT(*) FROM analysis)").fetch_one(&pool).await.unwrap();
        assert_eq!(counts, (0, 0, 0, 1));
        assert!(shared.is_file());
        assert!(!stale.exists());
        assert_eq!(std::fs::read(&original).unwrap(), b"original photo");
        reset_scan_library(&pool, &thumbs).await.unwrap();
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn legacy_scan_sessions_gain_checkpoint_columns_and_run_errors_are_recorded() {
        let dir = std::env::temp_dir().join(format!("photomind-scanckpt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("photomind.db");
        {
            let legacy = SqlitePool::connect(&format!("sqlite:{}?mode=rwc", db_path.display()))
                .await
                .unwrap();
            sqlx::query("CREATE TABLE scan_sessions (id INTEGER PRIMARY KEY, started_at DATETIME, completed_at DATETIME, status TEXT, total_files INTEGER, processed_files INTEGER)").execute(&legacy).await.unwrap();
            sqlx::query("INSERT INTO scan_sessions (status) VALUES ('running')")
                .execute(&legacy)
                .await
                .unwrap();
            sqlx::query("INSERT INTO scan_sessions (status) VALUES ('completed')")
                .execute(&legacy)
                .await
                .unwrap();
            legacy.close().await;
        }
        let pool = init_db(&dir).await.unwrap();
        let cols: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('scan_sessions') ORDER BY name")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(
            cols.contains(&"phase".to_string()),
            "missing phase: {cols:?}"
        );
        assert!(
            cols.contains(&"scan_path".to_string()),
            "missing scan_path: {cols:?}"
        );
        sqlx::query("INSERT INTO scan_sessions (started_at, status, phase) VALUES (CURRENT_TIMESTAMP, 'running', 'indexing')")
            .execute(&pool).await.unwrap();
        mark_interrupted_sessions(&pool).await.unwrap();
        let statuses: Vec<String> =
            sqlx::query_scalar("SELECT status FROM scan_sessions ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(statuses, vec!["interrupted", "completed", "interrupted"]);
        let running_id: i64 =
            sqlx::query_scalar("SELECT id FROM scan_sessions WHERE phase = 'indexing'")
                .fetch_one(&pool)
                .await
                .unwrap();
        record_scan_error(
            &pool,
            running_id,
            "/pic/IMG_1.CR2",
            "index",
            "Could not decode image: unsupported format",
        )
        .await
        .unwrap();
        let (stage, reason): (String, String) =
            sqlx::query_as("SELECT stage, reason FROM scan_errors")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stage, "index");
        assert!(reason.starts_with("Could not decode image"));
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn library_pages_include_every_photo_beyond_one_thousand() {
        let dir = std::env::temp_dir().join(format!("photomind-pages-{}", uuid::Uuid::new_v4()));
        let pool = init_db(&dir).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        for i in 0..1005 {
            let id = format!("photo-{i:04}");
            sqlx::query("INSERT INTO photos (id, absolute_path, filename, extension, file_size, gps_available, analysis_status, created_at) VALUES (?, ?, ?, 'jpg', 1, 0, 'pending', '2026-09-17T00:00:00Z')")
                .bind(&id).bind(&id).bind(&id).execute(&mut *tx).await.unwrap();
        }
        tx.commit().await.unwrap();
        let mut ids = Vec::new();
        loop {
            let page = photo_page(&pool, ids.len() as i64, 200).await.unwrap();
            if page.is_empty() {
                break;
            }
            ids.extend(page.into_iter().map(|p| p.id));
        }
        assert_eq!(ids.len(), 1005);
        assert_eq!(
            ids.iter().collect::<std::collections::HashSet<_>>().len(),
            1005
        );
        assert_eq!(ids.last().unwrap(), "photo-1004");
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn refresh_prunes_deleted_results_and_previews_but_retains_bin_and_keepers() {
        let dir = std::env::temp_dir().join(format!("photomind-refresh-{}", uuid::Uuid::new_v4()));
        let pool = init_db(&dir).await.unwrap();
        let thumbs = dir.join("thumbnails");
        std::fs::create_dir_all(&thumbs).unwrap();
        for id in ["alive", "deleted", "binned"] {
            let original = dir.join(format!("{id}.jpg"));
            let thumb = thumbs.join(format!("{id}.jpg"));
            std::fs::write(&thumb, b"generated preview").unwrap();
            if id == "alive" {
                std::fs::write(&original, b"original bytes").unwrap();
            }
            sqlx::query("INSERT INTO photos(id, absolute_path, thumbnail_path) VALUES (?, ?, ?)")
                .bind(id)
                .bind(original.to_string_lossy().as_ref())
                .bind(thumb.to_string_lossy().as_ref())
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO protected_photos(photo_id) VALUES (?)")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO recommendations(photo_id, decision) VALUES (?, 'KEEP')")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO bin_entries(photo_id, original_path) VALUES ('binned', 'missing-original')").execute(&pool).await.unwrap();
        reconcile_active_index(&pool).await.unwrap();
        gc_orphaned_thumbnails(&pool, &thumbs).await.unwrap();
        let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM photos ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(ids, vec!["alive", "binned"]);
        assert!(!thumbs.join("deleted.jpg").exists());
        assert!(thumbs.join("alive.jpg").exists());
        assert!(thumbs.join("binned.jpg").exists());
        assert_eq!(
            std::fs::read(dir.join("alive.jpg")).unwrap(),
            b"original bytes"
        );
        let protected: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM protected_photos")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(protected, 2);
        let stale: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM recommendations WHERE photo_id='deleted'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stale, 0);
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn gc_removes_only_orphaned_thumbnails_and_keeps_the_library() {
        let dir = std::env::temp_dir().join(format!("photomind-gc-{}", uuid::Uuid::new_v4()));
        let pool = init_db(&dir).await.unwrap();
        let thumbs = dir.join("thumbnails");
        std::fs::create_dir_all(&thumbs).unwrap();
        let kept = thumbs.join("kept.jpg");
        let orphan = thumbs.join("orphan.jpg");
        let unrelated = thumbs.join(".DS_Store");
        std::fs::write(&kept, b"kept preview").unwrap();
        let kept_str = std::fs::canonicalize(&kept)
            .unwrap()
            .to_string_lossy()
            .to_string();
        std::fs::write(&orphan, b"orphan preview").unwrap();
        std::fs::write(&unrelated, b"junk").unwrap();
        sqlx::query("INSERT INTO photos (id, absolute_path, thumbnail_path, analysis_status) VALUES ('p1', '/p1.jpg', ?, 'pending')")
            .bind(&kept_str).execute(&pool).await.unwrap();
        // A user decision for the photo must survive the GC.
        sqlx::query("INSERT INTO user_decisions (photo_id, recommended_decision, actual_decision) VALUES ('p1', 'REMOVE', 'KEEP')").execute(&pool).await.unwrap();
        gc_orphaned_thumbnails(&pool, &thumbs).await.unwrap();
        assert!(kept.is_file());
        assert!(!orphan.exists());
        assert!(!unrelated.exists());
        let decision: String =
            sqlx::query_scalar("SELECT actual_decision FROM user_decisions WHERE photo_id = 'p1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(decision, "KEEP");
        let photos = all_photos(&pool).await.unwrap();
        assert_eq!(photos.len(), 1);
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn photo(
        id: &str,
        absolute_path: &str,
        extension: &str,
        size: i64,
        capture_time: Option<&str>,
    ) -> Photo {
        Photo {
            id: id.into(),
            is_kept: false,
            absolute_path: absolute_path.into(),
            filename: absolute_path.rsplit('/').next().unwrap_or(id).into(),
            extension: extension.into(),
            file_size: size,
            width: None,
            height: None,
            created_at: None,
            modified_at: None,
            capture_time: capture_time.map(str::to_string),
            camera_make: None,
            camera_model: None,
            orientation: None,
            gps_available: false,
            sha256: None,
            perceptual_hash: None,
            thumbnail_path: None,
            analysis_status: "pending".into(),
            moment_id: None,
            sharpness: None,
            exposure: None,
            contrast: None,
            highlight_clipping: None,
            shadow_clipping: None,
        }
    }

    #[test]
    fn conservative_pairs_only_join_identical_shot_raw_jpeg_files() {
        let t = "2026-09-17T12:00:00Z";
        let pairs = conservative_pairs(&[
            photo("p1", "/photos/IMG_0043.NEF", "nef", 12_000_000, Some(t)),
            photo("p2", "/photos/IMG_0043.JPG", "jpg", 3_000_000, Some(t)),
            photo(
                "p3",
                "/photos/IMG_0044.NEF",
                "nef",
                12_000_000,
                Some("2026-09-17T12:00:00Z"),
            ),
            photo(
                "p4",
                "/photos/IMG_0044.JPG",
                "jpg",
                3_000_000,
                Some("2026-09-17T12:03:00Z"),
            ), // 3 minutes apart
            photo("p5", "/photos/IMG_0045.NEF", "nef", 3_000_000, Some(t)), // same size as a JPEG: a copy, not a pair
            photo("p6", "/photos/IMG_0045.JPG", "jpg", 3_000_000, Some(t)),
            photo("p7", "/photos/IMG_0046.NEF", "nef", 12_000_000, Some(t)),
            photo("p8", "/other/IMG_0046.JPG", "jpg", 3_000_000, Some(t)), // different folder
            photo("p9", "/photos/IMG_0047.CR2", "cr2", 12_000_000, Some(t)),
            photo("p10", "/photos/IMG_0047.jpg", "jpg", 3_000_000, Some(t)),
            photo("p11", "/photos/IMG_0048.NEF", "nef", 12_000_000, None), // no capture time anywhere
            photo("p12", "/photos/IMG_0048.JPG", "jpg", 3_000_000, None),
        ]);
        let ids: Vec<&String> = pairs.iter().flat_map(|(a, b, _)| [a, b]).collect();
        assert!(
            pairs
                .iter()
                .any(|(a, b, _)| (a.as_str(), b.as_str()) == ("p1", "p2")),
            "exact stem + compatible time must pair: {pairs:?}"
        );
        assert!(
            pairs
                .iter()
                .any(|(a, b, _)| (a.as_str(), b.as_str()) == ("p11", "p12")),
            "missing times must not block a same-folder same-stem pair: {pairs:?}"
        );
        assert!(
            !pairs.iter().any(|(a, _, _)| a == "p3"),
            "capture times 180s apart must not pair"
        );
        assert!(
            !pairs.iter().any(|(a, _, _)| a == "p5"),
            "identical file sizes are copies, never a RAW+JPEG pair"
        );
        assert!(
            !pairs.iter().any(|(a, _, _)| a == "p7"),
            "different folders must not pair"
        );
        assert!(
            pairs
                .iter()
                .any(|(a, b, _)| (a.as_str(), b.as_str()) == ("p9", "p10")),
            "case-insensitive JPEG extension must pair with CR2"
        );
        assert_eq!(
            ids.iter().collect::<std::collections::HashSet<_>>().len(),
            ids.len(),
            "no photo may appear in two pairs"
        );
    }

    #[tokio::test]
    async fn logical_pairs_are_built_pruned_and_returned_with_both_photos() {
        let dir = std::env::temp_dir().join(format!("photomind-pairs-{}", uuid::Uuid::new_v4()));
        let pool = init_db(&dir).await.unwrap();
        for (id, path, ext, size) in [
            ("p1", "/photos/IMG_0050.NEF", "nef", 12_000_000),
            ("p2", "/photos/IMG_0050.JPG", "jpg", 3_000_000),
            ("p3", "/photos/IMG_0051.NEF", "nef", 12_000_000),
        ] {
            sqlx::query("INSERT INTO photos (id, absolute_path, filename, extension, file_size, capture_time, analysis_status) VALUES (?, ?, ?, ?, ?, '2026-09-17T12:00:00Z', 'pending')")
                .bind(id).bind(path).bind(path).bind(ext).bind(size).execute(&pool).await.unwrap();
        }
        assert_eq!(build_logical_pairs(&pool).await.unwrap(), 1);
        assert_eq!(logical_pair_photo_ids(&pool).await.unwrap().len(), 2);
        let pairs = get_logical_pairs(&pool).await.unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].primary.id, "p1");
        assert_eq!(pairs[0].paired.id, "p2");
        assert!(pairs[0].match_reason.contains("120s"));
        // Delete the RAW: its pair must not survive the next rebuild.
        sqlx::query("DELETE FROM photos WHERE id = 'p1'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(build_logical_pairs(&pool).await.unwrap(), 0);
        assert!(get_logical_pairs(&pool).await.unwrap().is_empty());
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }
}

/// Duplicate detection uses the entire index, independently of library pagination.
pub async fn all_photos(pool: &SqlitePool) -> Result<Vec<crate::models::Photo>, sqlx::Error> {
    sqlx::QueryBuilder::<sqlx::Sqlite>::new(PHOTO_SELECT).push(" WHERE NOT EXISTS (SELECT 1 FROM bin_entries b WHERE b.photo_id = p.id) ORDER BY p.absolute_path, p.id").build_query_as()
        .fetch_all(pool).await
}

/// Reuse AI only for identical bytes with the same model, never visual similarity.
pub async fn reusable_ai_result(
    pool: &SqlitePool,
    hash: Option<&str>,
    model: &str,
) -> Result<Option<String>, sqlx::Error> {
    let Some(hash) = hash.filter(|h| !h.is_empty()) else {
        return Ok(None);
    };
    sqlx::query_scalar("SELECT r.analysis_json FROM photo_ai_results r JOIN photos p ON p.id = r.photo_id WHERE p.sha256 = ? AND r.model = ? ORDER BY r.analyzed_at DESC LIMIT 1").bind(hash).bind(model).fetch_optional(pool).await
}

pub async fn save_ai_result(
    pool: &SqlitePool,
    id: &str,
    hash: Option<&str>,
    model: &str,
    result: &str,
) -> Result<bool, sqlx::Error> {
    let saved = sqlx::query("INSERT OR REPLACE INTO photo_ai_results (photo_id, model, analysis_json, analyzed_at) SELECT id, ?, ?, CURRENT_TIMESTAMP FROM photos WHERE id = ? AND sha256 IS ? AND NOT EXISTS (SELECT 1 FROM bin_entries b WHERE b.photo_id = photos.id)").bind(model).bind(result).bind(id).bind(hash).execute(pool).await?;
    Ok(saved.rows_affected() > 0)
}

#[cfg(test)]
mod ai_cache_tests {
    use super::*;
    #[tokio::test]
    async fn ai_cache_requires_identical_contents_and_model_and_rejects_stale_writes() {
        let dir = std::env::temp_dir().join(format!("photomind-ai-cache-{}", uuid::Uuid::new_v4()));
        let pool = init_db(&dir).await.unwrap();
        for (id, hash) in [("a", "same"), ("b", "same"), ("c", "different")] {
            sqlx::query("INSERT INTO photos (id, sha256) VALUES (?, ?)")
                .bind(id)
                .bind(hash)
                .execute(&pool)
                .await
                .unwrap();
        }
        assert!(save_ai_result(&pool, "a", Some("same"), "vision", "{}")
            .await
            .unwrap());
        assert_eq!(
            reusable_ai_result(&pool, Some("same"), "vision")
                .await
                .unwrap(),
            Some("{}".into())
        );
        assert!(reusable_ai_result(&pool, Some("different"), "vision")
            .await
            .unwrap()
            .is_none());
        assert!(reusable_ai_result(&pool, Some("same"), "another-model")
            .await
            .unwrap()
            .is_none());
        assert!(reusable_ai_result(&pool, None, "vision")
            .await
            .unwrap()
            .is_none());
        assert!(save_ai_result(&pool, "b", Some("same"), "vision", "{}")
            .await
            .unwrap());
        sqlx::query("UPDATE photos SET sha256 = 'edited' WHERE id = 'c'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            !save_ai_result(&pool, "c", Some("different"), "vision", "{}")
                .await
                .unwrap()
        );
        sqlx::query("INSERT INTO bin_entries (photo_id, original_path) VALUES ('b', '/b.jpg')")
            .execute(&pool)
            .await
            .unwrap();
        assert!(!save_ai_result(&pool, "b", Some("same"), "vision", "{}")
            .await
            .unwrap());
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }
}

/// Phase 0 test: after a fresh `init_db`, the foreign key and WAL
/// pragmas are enforced, confirming the stability infrastructure is live.
#[tokio::test]
async fn fresh_db_enforces_foreign_keys_and_wal_checkpoint() {
    let dir = std::env::temp_dir().join(format!("photomind-pragmas-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let pool = init_db(&dir).await.unwrap();
    let mut connections = Vec::new();
    for _ in 0..5 {
        connections.push(pool.acquire().await.unwrap());
    }
    for connection in &mut connections {
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&mut **connection)
            .await
            .unwrap();
        assert_eq!(fk, 1, "every connection must enforce foreign keys");
        let limit: i64 = sqlx::query_scalar("PRAGMA journal_size_limit")
            .fetch_one(&mut **connection)
            .await
            .unwrap();
        assert_eq!(limit, 67108864);
        assert!(
            sqlx::query("INSERT INTO analysis (photo_id) VALUES ('does-not-exist')")
                .execute(&mut **connection)
                .await
                .is_err()
        );
    }
    drop(connections);
    pool.close().await;
    std::fs::remove_dir_all(dir).unwrap();
}
