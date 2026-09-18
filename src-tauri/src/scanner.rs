use crate::face_analysis::ANALYZER_VERSION;
use crate::ingest;
use crate::models::{Photo, PhotoIndex};
use chrono::{DateTime, Utc};
use image::imageops::FilterType;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use tauri::Emitter;
use uuid::Uuid;
use walkdir::WalkDir;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub fn parse_photo_datetime(raw: &str) -> Option<chrono::DateTime<Utc>> {
    let cleaned = raw.trim().trim_matches('"').trim_matches('\'').trim();
    if cleaned.is_empty() {
        return None;
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(cleaned) {
        return Some(dt.with_timezone(&Utc));
    }
    let normalized = cleaned.replace('.', ":");
    let naive = chrono::NaiveDateTime::parse_from_str(&normalized, "%Y:%m:%d %H:%M:%S")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(cleaned, "%Y-%m-%d %H:%M:%S"))
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(cleaned, "%Y-%m-%d %H:%M:%S%.f"))
        .ok()?;
    Some(naive.and_utc())
}

pub fn extract_metadata(
    path: &Path,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    bool,
) {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return (None, None, None, None, false),
    };

    let mut bufreader = std::io::BufReader::new(file);
    let exifreader = exif::Reader::new();

    match exifreader.read_from_container(&mut bufreader) {
        Ok(exif) => {
            let capture_time = exif
                .get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)
                .or_else(|| exif.get_field(exif::Tag::DateTime, exif::In::PRIMARY))
                .and_then(|f| {
                    let raw = f.display_value().with_unit(&exif).to_string();
                    parse_photo_datetime(&raw).map(|dt| dt.to_rfc3339())
                });
            let camera_make = exif
                .get_field(exif::Tag::Make, exif::In::PRIMARY)
                .and_then(|f| Some(f.display_value().to_string().trim_matches('"').to_string()));
            let camera_model = exif
                .get_field(exif::Tag::Model, exif::In::PRIMARY)
                .and_then(|f| Some(f.display_value().to_string().trim_matches('"').to_string()));
            let orientation = exif
                .get_field(exif::Tag::Orientation, exif::In::PRIMARY)
                .and_then(|f| f.value.get_uint(0).map(|v| v as i64));

            let gps_available = exif
                .get_field(exif::Tag::GPSLatitude, exif::In::PRIMARY)
                .is_some();

            (
                capture_time,
                camera_make,
                camera_model,
                orientation,
                gps_available,
            )
        }
        Err(_) => (None, None, None, None, false),
    }
}

pub fn calculate_sha256(path: &Path) -> Result<String, std::io::Error> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub struct ImageMetrics {
    pub sharpness: f64,
    pub exposure: f64,
    pub contrast: f64,
    pub highlight_clipping: f64,
    pub shadow_clipping: f64,
    pub mean_saturation: f64,
    pub flat_fraction: f64,
}

impl ImageMetrics {
    /// Rule-based junk label (screenshot / document / overexposed / underexposed).
    pub fn junk(&self) -> Option<(String, f64)> {
        crate::junk::detect_junk(self).map(|h| (h.category.to_string(), h.confidence))
    }
}

pub fn generate_jpeg_thumbnail(
    path: &str,
    longest_edge: u32,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let img =
        ingest::load_normalized(Path::new(path), longest_edge).map_err(std::io::Error::other)?;
    let thumb = img.thumbnail(longest_edge, longest_edge);
    let mut cursor = std::io::Cursor::new(Vec::new());
    thumb
        .into_rgb8()
        .write_to(&mut cursor, image::ImageFormat::Jpeg)?;
    Ok(cursor.into_inner())
}

/// Result of processing one photo: a content-addressed JPEG thumbnail, the
/// dHash, technical metrics, and the full-resolution (post-rotation) size.
pub struct ProcessedImage {
    pub width: u32,
    pub height: u32,
    pub thumbnail_path: String,
    pub perceptual_hash: String,
    pub metrics: ImageMetrics,
}

pub fn process_image(
    path: &Path,
    thumb_dir: &Path,
    id: &str,
) -> Result<ProcessedImage, Box<dyn std::error::Error>> {
    let result: std::thread::Result<Result<ProcessedImage, Box<dyn std::error::Error>>> =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            process_image_inner(path, thumb_dir, id)
        }));
    match result {
        Ok(inner) => inner,
        Err(_) => Err("Image processing crashed on this file, which is unusual but not fatal; the file was skipped.".into()),
    }
}

fn process_image_inner(
    path: &Path,
    thumb_dir: &Path,
    id: &str,
) -> Result<ProcessedImage, Box<dyn std::error::Error>> {
    // Normalize once: decode through the ingest layer so EXIF rotation is
    // applied and analysis always runs on an upright, bounded image.
    // `load_normalized_info` also reports the full resolution so RAW/AVIF
    // photos store their true (post-rotation) dimensions.
    let (img, full_width, full_height) =
        ingest::load_normalized_info(path, 512).map_err(std::io::Error::other)?;

    // Create directory if it doesn't exist
    std::fs::create_dir_all(thumb_dir)?;

    // Reduce the full-resolution image once, then reuse the small working image.
    let working = img.thumbnail(512, 512);
    let thumb = working.thumbnail(256, 256);
    let thumb_name = format!("{}.jpg", id);
    let _thumb_path = thumb_dir.join(&thumb_name);

    // Ensure we're saving to a valid absolute path format
    let absolute_thumb_path = std::fs::canonicalize(thumb_dir)
        .unwrap_or_else(|_| thumb_dir.to_path_buf())
        .join(&thumb_name);

    thumb.into_rgb8().save(&absolute_thumb_path)?;

    // dHash and Analysis Image
    let analysis_img = working.to_luma8();

    // Cheap color/flatness signals that feed the junk detector.
    let (mean_saturation, flat_fraction) = crate::junk::compute_signals(&working);

    let hash_img = image::imageops::resize(&analysis_img, 9, 8, FilterType::Nearest);
    let mut hash_bytes = [0u8; 8];
    for y in 0..8 {
        let mut row_byte = 0u8;
        for x in 0..8 {
            let left = hash_img.get_pixel(x, y)[0];
            let right = hash_img.get_pixel(x + 1, y)[0];
            if left > right {
                row_byte |= 1 << x;
            }
        }
        hash_bytes[y as usize] = row_byte;
    }
    let phash = hex::encode(hash_bytes);

    // Technical Analysis
    let (width, height) = analysis_img.dimensions();
    let total_pixels = (width * height) as f64;

    let mut sum = 0.0;
    let mut shadow_count = 0;
    let mut highlight_count = 0;

    for p in analysis_img.pixels() {
        let v = p[0] as f64;
        sum += v;
        if v < 10.0 {
            shadow_count += 1;
        }
        if v > 245.0 {
            highlight_count += 1;
        }
    }

    let mean = sum / total_pixels;
    let exposure = (mean / 255.0).clamp(0.0, 1.0);
    let shadow_clipping = (shadow_count as f64 / total_pixels).clamp(0.0, 1.0);
    let highlight_clipping = (highlight_count as f64 / total_pixels).clamp(0.0, 1.0);

    let mut variance_sum = 0.0;
    for p in analysis_img.pixels() {
        let v = p[0] as f64;
        variance_sum += (v - mean) * (v - mean);
    }
    let contrast = ((variance_sum / total_pixels).sqrt() / 127.5).clamp(0.0, 1.0);

    // Sharpness: Variance of Laplacian
    let mut laplacian_sum = 0.0;
    let mut laplacian_sq_sum = 0.0;
    let mut laplacian_count = 0.0;

    if width > 2 && height > 2 {
        for y in 1..(height - 1) {
            for x in 1..(width - 1) {
                let v = analysis_img.get_pixel(x, y)[0] as f64 * -4.0
                    + analysis_img.get_pixel(x - 1, y)[0] as f64
                    + analysis_img.get_pixel(x + 1, y)[0] as f64
                    + analysis_img.get_pixel(x, y - 1)[0] as f64
                    + analysis_img.get_pixel(x, y + 1)[0] as f64;
                laplacian_sum += v;
                laplacian_sq_sum += v * v;
                laplacian_count += 1.0;
            }
        }
        let laplacian_mean = laplacian_sum / laplacian_count;
        let laplacian_var =
            (laplacian_sq_sum / laplacian_count) - (laplacian_mean * laplacian_mean);
        let sharpness = (laplacian_var / 1000.0).clamp(0.0, 1.0);

        let metrics = ImageMetrics {
            sharpness,
            exposure,
            contrast,
            highlight_clipping,
            shadow_clipping,
            mean_saturation,
            flat_fraction,
        };
        Ok(ProcessedImage {
            width: full_width,
            height: full_height,
            thumbnail_path: absolute_thumb_path.to_string_lossy().to_string(),
            perceptual_hash: phash,
            metrics,
        })
    } else {
        let metrics = ImageMetrics {
            sharpness: 0.0,
            exposure,
            contrast,
            highlight_clipping,
            shadow_clipping,
            mean_saturation,
            flat_fraction,
        };
        Ok(ProcessedImage {
            width: full_width,
            height: full_height,
            thumbnail_path: absolute_thumb_path.to_string_lossy().to_string(),
            perceptual_hash: phash,
            metrics,
        })
    }
}

pub async fn run_scan(
    path: String,
    pool: sqlx::SqlitePool,
    thumb_dir: PathBuf,
    app_handle: tauri::AppHandle,
    is_paused: Arc<AtomicBool>,
    is_cancelled: Arc<AtomicBool>,
) -> bool {
    let _ = app_handle.emit(
        "scan-progress",
        json!({
            "processed": 0,
            "total": 0,
            "phase": "discovering",
            "current_file": "Looking for photos..."
        }),
    );

    let session_id: i64 = match sqlx::query_scalar("INSERT INTO scan_sessions (started_at, status, total_files, processed_files, phase, scan_path) VALUES (CURRENT_TIMESTAMP, 'running', 0, 0, 'discovering', ?) RETURNING id")
        .bind(&path).fetch_one(&pool).await {
            Ok(id) => id,
            Err(error) => {
                let _ = app_handle.emit("scan-error", json!({"message":format!("Could not create scan session: {error}")}));
                return false;
            }
        };
    tracing::info!(
        scan_session_id = session_id,
        stage = "discovery",
        status = "started",
        "Scan started"
    );

    let reconciliation_error = crate::db::reconcile_active_index(&pool)
        .await
        .err()
        .map(|error| error.to_string());
    if let Some(error) = reconciliation_error {
        return fail_scan(
            &pool,
            &app_handle,
            session_id,
            format!("Could not reconcile library files: {error}"),
        )
        .await;
    }
    let mut image_paths: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(&path)
        .into_iter()
        .filter_entry(|e| {
            ![".Trash", ".Trashes", ".photomind-bin"]
                .contains(&e.file_name().to_string_lossy().as_ref())
        })
        .filter_map(|e| e.ok())
    {
        if is_cancelled.load(Ordering::SeqCst) {
            let _ = sqlx::query(
                "UPDATE scan_sessions SET status = 'cancelled', phase = 'cancelled' WHERE id = ?",
            )
            .bind(session_id)
            .execute(&pool)
            .await;
            let _ = app_handle.emit(
                "scan-complete",
                json!({ "processed": 0, "phase": "cancelled" }),
            );
            return false;
        }
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let is_image = ingest::is_photo_path(p);
        if is_image {
            image_paths.push(p.to_path_buf());
            if image_paths.len() % 25 == 0 {
                let _ = app_handle.emit(
                    "scan-progress",
                    json!({
                        "processed": 0,
                        "total": image_paths.len(),
                        "phase": "discovering",
                        "current_file": format!("Found {} photos...", image_paths.len())
                    }),
                );
            }
        }
    }

    let total = image_paths.len();
    if let Err(error) =
        sqlx::query("UPDATE scan_sessions SET total_files = ?, phase = 'indexing' WHERE id = ?")
            .bind(total as i64)
            .bind(session_id)
            .execute(&pool)
            .await
    {
        return fail_scan(
            &pool,
            &app_handle,
            session_id,
            format!("Could not save discovery checkpoint: {error}"),
        )
        .await;
    }
    let _ = app_handle.emit(
        "scan-progress",
        json!({
            "processed": 0,
            "total": total,
            "phase": "indexing",
            "current_file": format!("Indexing {} photos", total)
        }),
    );

    let mut processed = 0usize;
    let mut failed = 0usize;
    let mut last_group_update = std::time::Instant::now();
    // Two decoders bound memory usage while overlapping independent image work.
    for batch in image_paths.chunks(2) {
        while is_paused.load(Ordering::SeqCst) && !is_cancelled.load(Ordering::SeqCst) {
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        }
        if is_cancelled.load(Ordering::SeqCst) {
            break;
        }
        let mut workers = tokio::task::JoinSet::new();
        let mut persistence_error = None;
        for image_path in batch {
            let image_path = image_path.clone();
            let pool = pool.clone();
            let thumbs = thumb_dir.clone();
            workers.spawn(async move {
                use tracing::Instrument;
                let span = tracing::info_span!("photo_job", scan_session_id = session_id, filename = %image_path.file_name().unwrap_or_default().to_string_lossy());
                let result = refresh_photo(&pool, &image_path, &thumbs).instrument(span).await;
                (image_path, result)
            });
        }
        // Drain every worker before completion/cancellation: no writes outlive the scan.
        while let Some(result) = workers.join_next().await {
            match result {
                Ok((path, Ok(()))) => {
                    processed += 1;
                    let _ = app_handle.emit(
                        "scan-progress",
                        json!({
                            "processed": processed + failed, "total": total,
                            "phase": "indexing",
                            "current_file": path.file_name().unwrap_or_default().to_string_lossy()
                        }),
                    );
                }
                // Un-decodable or corrupt files are skipped and counted — and
                // recorded with their reason so the failure is never silent.
                Ok((path, Err(reason))) => {
                    failed += 1;
                    let reason = reason.chars().take(500).collect::<String>();
                    eprintln!(
                        "{}",
                        json!({"scan_session_id":session_id,"filename":path.file_name().unwrap_or_default().to_string_lossy(),"stage":"index","success":false,"error":reason})
                    );
                    if let Err(error) = crate::db::record_scan_error(
                        &pool,
                        session_id,
                        &path.to_string_lossy(),
                        "index",
                        &reason,
                    )
                    .await
                    {
                        persistence_error =
                            Some(format!("Could not record photo failure: {error}"));
                    }
                }
                Err(error) => {
                    failed += 1;
                    if let Err(record_error) = crate::db::record_scan_error(
                        &pool,
                        session_id,
                        "",
                        "worker",
                        &error.to_string(),
                    )
                    .await
                    {
                        persistence_error =
                            Some(format!("Could not record worker failure: {record_error}"));
                    }
                }
            }
        }
        if let Some(error) = persistence_error {
            return fail_scan(&pool, &app_handle, session_id, error).await;
        }
        let _ = app_handle.emit("scan-progress", json!({"processed": processed + failed, "total": total, "failed": failed, "phase": "indexing", "current_file": format!("{} indexed · {} skipped", processed, failed)}));
        if processed > 0 && last_group_update.elapsed() >= std::time::Duration::from_secs(3) {
            if let Err(error) = generate_moments(&pool).await {
                eprintln!("Interim grouping failed: {error}");
            } else if let Err(error) = crate::decisions::update(&pool, None).await {
                eprintln!("Interim recommendations failed: {error}");
            } else {
                let _ = app_handle.emit(
                    "local-results-ready",
                    json!({"scan_session_id": session_id}),
                );
            }
            last_group_update = std::time::Instant::now();
        }
        if let Err(error) = sqlx::query("UPDATE scan_sessions SET processed_files = ? WHERE id = ?")
            .bind((processed + failed) as i64)
            .bind(session_id)
            .execute(&pool)
            .await
        {
            return fail_scan(
                &pool,
                &app_handle,
                session_id,
                format!("Could not save scan checkpoint: {error}"),
            )
            .await;
        }
    }
    if is_cancelled.load(Ordering::SeqCst) {
        if let Err(error) = sqlx::query("UPDATE scan_sessions SET status = 'cancelled', completed_at = CURRENT_TIMESTAMP, phase = 'cancelled' WHERE id = ?")
            .bind(session_id).execute(&pool).await {
            return fail_scan(&pool, &app_handle, session_id, format!("Could not save cancellation: {error}")).await;
        }
        let _ = app_handle.emit(
            "scan-complete",
            json!({ "processed": processed, "phase": "cancelled" }),
        );
        return false;
    }

    if total > 0 && processed == 0 {
        let reason: Option<String> = sqlx::query_scalar(
            "SELECT reason FROM scan_errors WHERE session_id = ? ORDER BY id LIMIT 1",
        )
        .bind(session_id)
        .fetch_optional(&pool)
        .await
        .ok()
        .flatten();
        return fail_scan(
            &pool,
            &app_handle,
            session_id,
            format!(
                "None of the {total} photos could be indexed. {}",
                reason.unwrap_or_else(|| "The photo workers failed. Try scanning again.".into())
            ),
        )
        .await;
    }

    // RAW+JPEG same-shot pairs become one logical photo before grouping so the
    // pair is never shown as two frames to cull (or as a duplicate of itself).
    if let Err(error) = crate::db::build_logical_pairs(&pool).await {
        return fail_scan(
            &pool,
            &app_handle,
            session_id,
            format!("Could not save RAW/JPEG pairs: {error}"),
        )
        .await;
    }

    let _ = app_handle.emit(
        "scan-progress",
        json!({
            "processed": total,
            "total": total.max(1),
            "phase": "grouping",
            "current_file": "Grouping similar photos..."
        }),
    );
    if let Err(error) = generate_moments(&pool).await {
        return fail_scan(
            &pool,
            &app_handle,
            session_id,
            format!("Could not group photos: {error}"),
        )
        .await;
    }

    if let Err(error) = crate::decisions::update(&pool, None).await {
        return fail_scan(
            &pool,
            &app_handle,
            session_id,
            format!("Could not save local recommendations: {error}"),
        )
        .await;
    }
    if let Err(error) = sqlx::query("UPDATE scan_sessions SET status = 'completed', completed_at = CURRENT_TIMESTAMP, phase = 'completed' WHERE id = ?").bind(session_id).execute(&pool).await {
        return fail_scan(&pool, &app_handle, session_id, format!("Could not finalize scan: {error}")).await;
    }
    if let Err(error) = crate::db::gc_orphaned_thumbnails(&pool, &thumb_dir).await {
        tracing::warn!(scan_session_id = session_id, error = %error, "Could not clean unused previews");
    }
    tracing::info!(
        scan_session_id = session_id,
        stage = "scan",
        status = "completed",
        processed,
        failed,
        "Scan finished"
    );
    let _ = app_handle.emit(
        "scan-complete",
        json!({ "processed": processed, "failed": failed, "phase": "completed" }),
    );
    true
}

async fn fail_scan(
    pool: &sqlx::SqlitePool,
    app: &tauri::AppHandle,
    session_id: i64,
    message: String,
) -> bool {
    tracing::error!(scan_session_id = session_id, stage = "scan", status = "failed", error = %message);
    if let Err(error) = sqlx::query(
        "UPDATE scan_sessions SET status = 'failed', completed_at = CURRENT_TIMESTAMP WHERE id = ?",
    )
    .bind(session_id)
    .execute(pool)
    .await
    {
        tracing::error!(error = %error, "Could not persist scan failure");
    }
    let _ = app.emit("scan-error", json!({"message":message}));
    false
}

/// Rehash every visited file, including already-indexed files and failed hashes.
/// Full image decoding is only repeated for changed or incompletely indexed photos.
pub async fn refresh_photo(
    pool: &sqlx::SqlitePool,
    path: &Path,
    thumb_dir: &Path,
) -> Result<(), String> {
    if !ingest::is_photo_path(path) {
        return Err("Not a photo: unsupported file or macOS metadata sidecar".into());
    }
    let absolute_path = path.to_string_lossy().to_string();
    let existing: Option<(String, Option<String>, Option<String>, Option<String>, bool, bool)> =
        sqlx::query_as("SELECT id, sha256, perceptual_hash, thumbnail_path, EXISTS(SELECT 1 FROM analysis a WHERE a.photo_id = photos.id AND a.sharpness IS NOT NULL AND a.exposure IS NOT NULL AND a.highlight_clipping IS NOT NULL AND a.shadow_clipping IS NOT NULL AND a.contrast IS NOT NULL), EXISTS(SELECT 1 FROM photo_face_analysis pfa WHERE pfa.photo_id = photos.id AND pfa.analyzer_version = ?) FROM photos WHERE absolute_path = ?")
            .bind(ANALYZER_VERSION)
            .bind(&absolute_path)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;
    let hash_path = path.to_path_buf();
    let sha256 = tokio::task::spawn_blocking(move || {
        calculate_sha256(&hash_path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    if let Some((id, Some(old_hash), Some(phash), Some(thumb), true, face_ready)) = &existing {
        if *old_hash == sha256 && !phash.is_empty() && Path::new(thumb).is_file() && *face_ready {
            tracing::info!(photo_id = %id, stage = "technical", status = "reused", "Verified cached analysis");
            return Ok(());
        }
    }
    let changed = existing
        .as_ref()
        .map(|(_, hash, _, _, _, _)| hash.as_deref() != Some(sha256.as_str()))
        .unwrap_or(false);
    let id = existing
        .as_ref()
        .map(|(id, _, _, _, _, _)| id.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    tracing::info!(photo_id = %id, stage = "technical", status = "started", "Preparing photo");
    // Exact copies share derived previews/metrics; still hash each file's full contents.
    let cached: Option<(Option<i64>, Option<i64>, String, String, f64, f64, f64, f64, f64, f64, f64)> = sqlx::query_as(
        "SELECT p.width, p.height, p.thumbnail_path, p.perceptual_hash, a.sharpness, a.exposure, a.contrast, a.highlight_clipping, a.shadow_clipping, a.mean_saturation, a.flat_fraction FROM photos p JOIN analysis a ON a.photo_id = p.id WHERE p.sha256 = ? AND p.perceptual_hash IS NOT NULL AND p.thumbnail_path IS NOT NULL AND a.sharpness IS NOT NULL AND a.exposure IS NOT NULL AND a.contrast IS NOT NULL AND a.highlight_clipping IS NOT NULL AND a.shadow_clipping IS NOT NULL AND a.mean_saturation IS NOT NULL AND a.flat_fraction IS NOT NULL LIMIT 1"
    ).bind(&sha256).fetch_optional(pool).await.map_err(|e| e.to_string())?;
    let cached = cached
        .filter(|(_, _, thumb, _, ..)| Path::new(thumb).is_file())
        .map(
            |(
                width,
                height,
                thumbnail,
                hash,
                sharpness,
                exposure,
                contrast,
                highlight_clipping,
                shadow_clipping,
                mean_saturation,
                flat_fraction,
            )| CachedImage {
                width,
                height,
                thumbnail,
                hash,
                metrics: ImageMetrics {
                    sharpness,
                    exposure,
                    contrast,
                    highlight_clipping,
                    shadow_clipping,
                    mean_saturation,
                    flat_fraction,
                },
            },
        );
    let index_path = path.to_path_buf();
    let thumbs = thumb_dir.to_path_buf();
    let sha256_for_face = sha256.clone();
    let row = tokio::task::spawn_blocking(move || {
        index_one_photo(&index_path, &thumbs, id, sha256, cached)
    })
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("Cannot read metadata for {absolute_path}"))?;
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    if changed {
        sqlx::query("DELETE FROM photo_ai_results WHERE photo_id = ?")
            .bind(&row.id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        sqlx::query("DELETE FROM photo_faces WHERE photo_id = ?")
            .bind(&row.id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        sqlx::query("DELETE FROM photo_face_analysis WHERE photo_id = ?")
            .bind(&row.id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        sqlx::query("DELETE FROM recommendations WHERE photo_id = ? OR moment_id IN (SELECT moment_id FROM photos WHERE id = ?)")
            .bind(&row.id).bind(&row.id).execute(&mut *tx).await.map_err(|e| e.to_string())?;
    }
    sqlx::query(r#"INSERT INTO photos (
        id, absolute_path, filename, extension, file_size, width, height, created_at, modified_at,
        capture_time, camera_make, camera_model, orientation, gps_available, sha256, perceptual_hash, thumbnail_path, analysis_status
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending')
    ON CONFLICT(absolute_path) DO UPDATE SET
        filename=excluded.filename, extension=excluded.extension, file_size=excluded.file_size,
        width=excluded.width, height=excluded.height, created_at=excluded.created_at, modified_at=excluded.modified_at,
        capture_time=excluded.capture_time, camera_make=excluded.camera_make, camera_model=excluded.camera_model,
        orientation=excluded.orientation, gps_available=excluded.gps_available, sha256=excluded.sha256,
        perceptual_hash=excluded.perceptual_hash, thumbnail_path=excluded.thumbnail_path, analysis_status=excluded.analysis_status"#)
        .bind(&row.id).bind(&row.absolute_path).bind(&row.filename).bind(&row.extension)
        .bind(row.file_size).bind(row.width).bind(row.height).bind(&row.created_at).bind(&row.modified_at)
        .bind(&row.capture_time).bind(&row.camera_make).bind(&row.camera_model).bind(row.orientation)
        .bind(row.gps_available).bind(&row.sha256).bind(&row.perceptual_hash).bind(&row.thumbnail_path)
        .execute(&mut *tx).await.map_err(|e| e.to_string())?;
    sqlx::query("DELETE FROM analysis WHERE photo_id = ?")
        .bind(&row.id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(m) = row.metrics {
        let (junk_category, junk_confidence) = m
            .junk()
            .map(|(c, conf)| (Some(c), Some(conf)))
            .unwrap_or((None, None));
        sqlx::query("INSERT INTO analysis (photo_id, sharpness, exposure, contrast, highlight_clipping, shadow_clipping, mean_saturation, flat_fraction, junk_category, junk_confidence) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&row.id).bind(m.sharpness).bind(m.exposure).bind(m.contrast).bind(m.highlight_clipping).bind(m.shadow_clipping)
            .bind(m.mean_saturation).bind(m.flat_fraction).bind(junk_category).bind(junk_confidence)
            .execute(&mut *tx).await.map_err(|e| e.to_string())?;
    }
    sqlx::query("UPDATE photos SET analysis_status = CASE WHEN perceptual_hash IS NOT NULL THEN 'technical_ready' ELSE 'error' END WHERE id = ?")
        .bind(&row.id).execute(&mut *tx).await.map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    if let Some(error) = row.decode_error {
        tracing::warn!(photo_id = %row.id, stage = "technical", status = "failed", error = %error);
        return Err(error);
    }
    tracing::info!(photo_id = %row.id, stage = "technical", status = "completed", "Photo ready");
    analyze_faces_for_photo(pool, &row.id, path, &sha256_for_face).await?;
    Ok(())
}

async fn analyze_faces_for_photo(
    pool: &sqlx::SqlitePool,
    photo_id: &str,
    path: &Path,
    sha256: &str,
) -> Result<(), String> {
    if crate::face_db::face_analysis_complete(pool, photo_id)
        .await
        .map_err(|e| e.to_string())?
    {
        return Ok(());
    }
    if crate::face_db::copy_face_analysis_by_sha256(pool, photo_id, sha256)
        .await
        .map_err(|e| e.to_string())?
    {
        tracing::info!(photo_id = %photo_id, stage = "face", status = "reused", "Reused face analysis from duplicate");
        return Ok(());
    }
    let path = path.to_path_buf();
    let analysis = tokio::task::spawn_blocking(move || {
        let rgb = ingest::load_normalized(&path, 512)
            .map_err(|e| e.to_string())?
            .into_rgb8();
        Ok::<_, String>(crate::face_analysis::analyze_rgb_preview(rgb))
    })
    .await
    .map_err(|e| e.to_string())??;

    crate::face_db::save_face_analysis(pool, photo_id, &analysis)
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!(photo_id = %photo_id, stage = "face", status = "completed", face_count = analysis.face_count);
    Ok(())
}

pub fn duplicate_groups(
    photos: &[Photo],
    paired_ids: &std::collections::HashSet<String>,
) -> Vec<crate::models::DuplicateGroup> {
    // A RAW+JPEG logical pair is ONE photo; neither side may ever be offered up
    // as a duplicate candidate of the other (or of anything else) in its stead.
    let photos: Vec<&Photo> = photos
        .iter()
        .filter(|p| !paired_ids.contains(&p.id))
        .collect();
    let mut exact = std::collections::BTreeMap::<String, Vec<Photo>>::new();
    let mut candidates = Vec::new();
    for photo in &photos {
        if let Some(hash) = photo.sha256.as_ref().filter(|h| !h.is_empty()) {
            exact
                .entry(hash.clone())
                .or_default()
                .push((*photo).clone());
        } else if photo.perceptual_hash.is_some() {
            candidates.push((*photo).clone());
        }
    }
    let mut groups = Vec::new();
    let mut copies = std::collections::HashMap::new();
    for (hash, mut group) in exact {
        group.sort_by(|a, b| {
            b.is_kept
                .cmp(&a.is_kept)
                .then(a.absolute_path.cmp(&b.absolute_path).then(a.id.cmp(&b.id)))
        });
        // Keep one representative per exact-copy set in global visual matching.
        // Expand it again when a differently encoded version matches the set.
        if let Some(representative) = group.iter().find(|p| p.perceptual_hash.is_some()) {
            copies.insert(representative.id.clone(), group.clone());
            candidates.push(representative.clone());
        }
        if group.len() > 1 {
            groups.push(crate::models::DuplicateGroup {
                sha256: hash,
                count: group.len() as i64,
                kind: "exact".into(),
                photos: group,
            });
        }
    }
    for cluster in cluster_similar_photos(&candidates) {
        let mut expanded: Vec<Photo> = cluster
            .into_iter()
            .flat_map(|p| copies.remove(&p.id).unwrap_or_else(|| vec![p]))
            .collect();
        expanded.sort_by(|a, b| a.absolute_path.cmp(&b.absolute_path).then(a.id.cmp(&b.id)));
        let key = format!(
            "similar-{}",
            moment_signature(&expanded.iter().map(|p| p.id.clone()).collect::<Vec<_>>())
        );
        groups.push(crate::models::DuplicateGroup {
            sha256: key,
            count: expanded.len() as i64,
            kind: "similar".into(),
            photos: expanded,
        });
    }
    groups.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then(b.count.cmp(&a.count))
            .then(a.sha256.cmp(&b.sha256))
    });
    groups
}

struct CachedImage {
    width: Option<i64>,
    height: Option<i64>,
    thumbnail: String,
    hash: String,
    metrics: ImageMetrics,
}

struct IndexedPhoto {
    id: String,
    absolute_path: String,
    filename: String,
    extension: String,
    file_size: i64,
    width: Option<i64>,
    height: Option<i64>,
    created_at: Option<String>,
    modified_at: Option<String>,
    capture_time: Option<String>,
    camera_make: Option<String>,
    camera_model: Option<String>,
    orientation: Option<i64>,
    gps_available: bool,
    sha256: Option<String>,
    perceptual_hash: Option<String>,
    thumbnail_path: Option<String>,
    metrics: Option<ImageMetrics>,
    decode_error: Option<String>,
}

fn index_one_photo(
    path: &Path,
    thumb_dir: &Path,
    id: String,
    sha256: String,
    cached: Option<CachedImage>,
) -> Option<IndexedPhoto> {
    let metadata = std::fs::metadata(path).ok()?;
    let filename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let extension = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
        .to_lowercase();
    let created_at = metadata
        .created()
        .ok()
        .map(|t| DateTime::<Utc>::from(t).to_rfc3339());
    let modified_at = metadata
        .modified()
        .ok()
        .map(|t| DateTime::<Utc>::from(t).to_rfc3339());
    let (capture_time, camera_make, camera_model, orientation, gps_available) =
        extract_metadata(path);
    let sha256 = Some(sha256);
    let mut decode_error = None;
    let (thumbnail_path, perceptual_hash, metrics, width, height) = if let Some(cached) = cached {
        (
            Some(cached.thumbnail),
            Some(cached.hash),
            Some(cached.metrics),
            cached.width,
            cached.height,
        )
    } else {
        // Content-addressed previews stay valid when an original with cached copies is edited.
        let processed = match process_image(path, thumb_dir, sha256.as_deref().unwrap_or(&id)) {
            Ok(image) => Some(image),
            Err(error) => {
                decode_error = Some(error.to_string());
                None
            }
        };
        let (thumbnail, hash, metrics, width, height) = match processed {
            Some(image) => (
                Some(image.thumbnail_path),
                Some(image.perceptual_hash),
                Some(image.metrics),
                Some(image.width as i64),
                Some(image.height as i64),
            ),
            None => (None, None, None, None, None),
        };
        (thumbnail, hash, metrics, width, height)
    };

    Some(IndexedPhoto {
        id,
        absolute_path: path.to_string_lossy().to_string(),
        filename,
        extension,
        file_size: metadata.len() as i64,
        width,
        height,
        created_at,
        modified_at,
        capture_time,
        camera_make,
        camera_model,
        orientation,
        gps_available,
        sha256,
        perceptual_hash,
        thumbnail_path,
        metrics,
        decode_error,
    })
}

fn hamming_distance(hex1: &str, hex2: &str) -> Option<u32> {
    crate::similarity::hash_distance(hex1, hex2)
}

pub async fn generate_moments(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let old_members: Vec<(String, String)> = sqlx::query_as(
        "SELECT moment_id, id FROM photos WHERE moment_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM bin_entries b WHERE b.photo_id = photos.id) ORDER BY id"
    ).fetch_all(&mut *tx).await?;
    let mut members = std::collections::HashMap::<String, Vec<String>>::new();
    for (moment, photo) in old_members {
        members.entry(moment).or_default().push(photo);
    }
    let old_counts: std::collections::HashMap<String, i64> =
        sqlx::query_as::<_, (String, i64)>("SELECT id, photo_count FROM moment_groups")
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .collect();
    let old_ids: std::collections::HashMap<String, String> = members
        .into_iter()
        .filter(|(id, photos)| old_counts.get(id).copied() == Some(photos.len() as i64))
        .map(|(id, photos)| (moment_signature(&photos), id))
        .collect();
    sqlx::query("UPDATE photos SET moment_id = NULL")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM moment_groups")
        .execute(&mut *tx)
        .await?;

    let photos: Vec<PhotoIndex> = sqlx::query_as(
        r#"SELECT id, absolute_path, filename, file_size, created_at, capture_time, sha256, perceptual_hash, thumbnail_path
           FROM photos WHERE NOT EXISTS (SELECT 1 FROM bin_entries b WHERE b.photo_id = photos.id) AND NOT EXISTS (SELECT 1 FROM logical_photos l WHERE l.paired_id = photos.id) ORDER BY COALESCE(capture_time, created_at) ASC, id ASC"#
    )
    .fetch_all(&mut *tx)
    .await?;

    let mut current_moment_photos: Vec<PhotoIndex> = Vec::new();
    let mut current_moment_start: Option<chrono::DateTime<Utc>> = None;

    for photo in photos {
        let time = photo
            .capture_time
            .as_deref()
            .or(photo.created_at.as_deref())
            .and_then(parse_photo_datetime);

        let mut belongs_to_moment = current_moment_photos.is_empty();

        if !belongs_to_moment {
            let last = current_moment_photos.last().unwrap();
            let similar = match (&photo.perceptual_hash, &last.perceptual_hash) {
                (Some(h1), Some(h2)) => hamming_distance(h1, h2).map(|d| d <= 16).unwrap_or(false),
                _ => false,
            };
            let close_in_time = match (current_moment_start, time) {
                (Some(t1), Some(t2)) => t2.signed_duration_since(t1).num_seconds().abs() <= 8,
                _ => false,
            };
            belongs_to_moment = close_in_time && similar;
        }

        if !belongs_to_moment && !current_moment_photos.is_empty() {
            save_moment(&mut tx, &current_moment_photos, &old_ids).await?;
            current_moment_photos.clear();
            current_moment_start = time;
        }

        if current_moment_photos.is_empty() {
            current_moment_start = time;
        }

        current_moment_photos.push(photo);
    }

    if !current_moment_photos.is_empty() {
        save_moment(&mut tx, &current_moment_photos, &old_ids).await?;
    }

    // Comparisons are valid only for the same set of photos. Clear stale results
    // when a moment gains/loses members or disappears, inside the same transaction.
    sqlx::query("DELETE FROM recommendations WHERE NOT EXISTS (SELECT 1 FROM photos p WHERE p.id = recommendations.photo_id AND p.moment_id = recommendations.moment_id)")
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

fn moment_signature(ids: &[String]) -> String {
    let mut ids = ids.to_vec();
    ids.sort();
    let mut hash = Sha256::new();
    for id in ids {
        hash.update((id.len() as u64).to_le_bytes());
        hash.update(id.as_bytes());
    }
    hex::encode(hash.finalize())
}

async fn save_moment(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    photos: &[PhotoIndex],
    old_ids: &std::collections::HashMap<String, String>,
) -> Result<(), sqlx::Error> {
    if photos.len() < 2 {
        return Ok(());
    }

    let signature = moment_signature(&photos.iter().map(|p| p.id.clone()).collect::<Vec<_>>());
    let moment_id = old_ids.get(&signature).cloned().unwrap_or(signature);
    let start = photos
        .first()
        .unwrap()
        .capture_time
        .as_ref()
        .or(photos.first().unwrap().created_at.as_ref())
        .cloned()
        .unwrap_or_default();
    let end = photos
        .last()
        .unwrap()
        .capture_time
        .as_ref()
        .or(photos.last().unwrap().created_at.as_ref())
        .cloned()
        .unwrap_or_default();

    sqlx::query(
        "INSERT INTO moment_groups (id, start_time, end_time, photo_count) VALUES (?, ?, ?, ?)",
    )
    .bind(&moment_id)
    .bind(&start)
    .bind(&end)
    .bind(photos.len() as i64)
    .execute(&mut **tx)
    .await?;

    for p in photos {
        sqlx::query("UPDATE photos SET moment_id = ? WHERE id = ?")
            .bind(&moment_id)
            .bind(&p.id)
            .execute(&mut **tx)
            .await?;
    }

    Ok(())
}

pub fn cluster_similar_photos(photos: &[Photo]) -> Vec<Vec<Photo>> {
    let n = photos.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let pa = find(parent, a);
        let pb = find(parent, b);
        if pa != pb {
            parent[pa] = pb;
        }
    }

    // Decode once per photo rather than twice for every pair.
    let hashes: Vec<Option<u64>> = photos
        .iter()
        .map(|p| {
            p.perceptual_hash
                .as_deref()
                .filter(|h| h.len() == 16)
                .and_then(|h| u64::from_str_radix(h, 16).ok())
        })
        .collect();
    for i in 0..n {
        let Some(h1) = hashes[i] else {
            continue;
        };
        for j in (i + 1)..n {
            let Some(h2) = hashes[j] else {
                continue;
            };
            // 12 bits: the "similar" tier of the shared similarity taxonomy.
            if (h1 ^ h2).count_ones() <= 12 {
                union(&mut parent, i, j);
            }
        }
    }

    let mut buckets: std::collections::HashMap<usize, Vec<Photo>> =
        std::collections::HashMap::new();
    for i in 0..n {
        buckets
            .entry(find(&mut parent, i))
            .or_default()
            .push(photos[i].clone());
    }
    buckets.into_values().filter(|g| g.len() >= 2).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("photomind-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
    }

    #[test]
    #[ignore = "manual performance measurement on a synthetic 24-megapixel JPEG"]
    fn benchmark_large_photo() {
        let dir = test_dir();
        let original = dir.join("24mp.jpg");
        image::RgbImage::from_fn(6000, 4000, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
        })
        .save(&original)
        .unwrap();
        let start = std::time::Instant::now();
        calculate_sha256(&original).unwrap();
        println!("SHA-256: {:?}", start.elapsed());
        let start = std::time::Instant::now();
        process_image(&original, &dir.join("thumbs"), "benchmark").unwrap();
        println!(
            "Decode + thumbnail + visual hash + quality: {:?}",
            start.elapsed()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn bounded_workers_index_all_photos_without_database_lock_errors() {
        let dir = test_dir();
        let pool = crate::db::init_db(&dir).await.unwrap();
        let thumbs = dir.join("thumbs");
        let mut paths = Vec::new();
        for i in 0..12 {
            let path = dir.join(format!("photo-{i}.png"));
            image::RgbImage::from_pixel(64, 48, image::Rgb([i, 100, 200]))
                .save(&path)
                .unwrap();
            paths.push(path);
        }
        for batch in paths.chunks(2) {
            let (a, b) = tokio::join!(
                refresh_photo(&pool, &batch[0], &thumbs),
                refresh_photo(&pool, &batch[1], &thumbs)
            );
            a.unwrap();
            b.unwrap();
        }
        assert_eq!(crate::db::all_photos(&pool).await.unwrap().len(), 12);
        let copy = dir.join("copy.png");
        std::fs::copy(&paths[0], &copy).unwrap();
        refresh_photo(&pool, &copy, &thumbs).await.unwrap();
        let rows = crate::db::all_photos(&pool).await.unwrap();
        let original = rows.iter().find(|p| p.filename == "photo-0.png").unwrap();
        let copied = rows.iter().find(|p| p.filename == "copy.png").unwrap();
        assert_eq!(original.thumbnail_path, copied.thumbnail_path);
        assert_eq!(original.perceptual_hash, copied.perceptual_hash);
        assert_eq!(original.sharpness, copied.sharpness);
        let shared_preview = copied.thumbnail_path.clone().unwrap();
        let before = std::fs::read(&shared_preview).unwrap();
        image::RgbImage::from_pixel(64, 48, image::Rgb([255, 10, 0]))
            .save(&paths[0])
            .unwrap();
        refresh_photo(&pool, &paths[0], &thumbs).await.unwrap();
        assert_eq!(std::fs::read(shared_preview).unwrap(), before);
        let changed = crate::db::all_photos(&pool).await.unwrap();
        assert_ne!(
            changed
                .iter()
                .find(|p| p.filename == "photo-0.png")
                .unwrap()
                .thumbnail_path,
            copied.thumbnail_path
        );
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn duplicates_match_globally_and_rescan_repairs_missing_and_changed_hashes() {
        let dir = test_dir();
        let pool = crate::db::init_db(&dir).await.unwrap();
        let first = dir.join("album-2001/IMG_0001.png");
        let second = dir.join("album-2026/subfolder/IMG_9999.png");
        let variant = dir.join("export/IMG_54321.tiff");
        for path in [&first, &second, &variant] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        }
        let image = image::RgbImage::from_fn(80, 64, |x, y| {
            image::Rgb([(x * 3) as u8, (y * 3) as u8, ((x + y) * 2) as u8])
        });
        image.save(&first).unwrap();
        std::fs::copy(&first, &second).unwrap();
        image.save(&variant).unwrap();
        let thumbs = dir.join("thumbs");
        for path in [&first, &second, &variant] {
            refresh_photo(&pool, path, &thumbs).await.unwrap();
        }
        sqlx::query("UPDATE photos SET capture_time = CASE WHEN filename = 'IMG_0001.png' THEN '2001-01-01T00:00:00Z' ELSE '2026-09-17T00:00:00Z' END")
            .execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        for i in 0..1005 {
            let id = format!("unrelated-{i}");
            sqlx::query("INSERT INTO photos (id, absolute_path, filename, extension, file_size, gps_available, analysis_status, sha256) VALUES (?, ?, ?, 'jpg', 1, 0, 'pending', ?)")
                .bind(&id).bind(&id).bind(&id).bind(&id).execute(&mut *tx).await.unwrap();
        }
        tx.commit().await.unwrap();
        let groups = duplicate_groups(
            &crate::db::all_photos(&pool).await.unwrap(),
            &std::collections::HashSet::new(),
        );
        let exact = groups.iter().find(|g| g.kind == "exact").unwrap();
        assert_eq!(exact.count, 2);
        assert!(exact.photos.iter().any(|p| p.filename == "IMG_0001.png"));
        assert!(exact.photos.iter().any(|p| p.filename == "IMG_9999.png"));
        let similar = groups.iter().find(|g| g.kind == "similar").unwrap();
        assert_eq!(similar.count, 3); // The exact-copy pair stays eligible for matching its TIFF export.

        let original_id: String =
            sqlx::query_scalar("SELECT id FROM photos WHERE filename = 'IMG_0001.png'")
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query("UPDATE photos SET sha256 = NULL WHERE id = ?")
            .bind(&original_id)
            .execute(&pool)
            .await
            .unwrap();
        refresh_photo(&pool, &first, &thumbs).await.unwrap();
        let repaired: String = sqlx::query_scalar("SELECT sha256 FROM photos WHERE id = ?")
            .bind(&original_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(repaired, calculate_sha256(&first).unwrap());
        image::RgbImage::from_pixel(80, 64, image::Rgb([200, 10, 40]))
            .save(&first)
            .unwrap();
        refresh_photo(&pool, &first, &thumbs).await.unwrap();
        let changed: String = sqlx::query_scalar("SELECT sha256 FROM photos WHERE id = ?")
            .bind(&original_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_ne!(changed, repaired);
        assert_eq!(changed, calculate_sha256(&first).unwrap());
        assert!(duplicate_groups(
            &crate::db::all_photos(&pool).await.unwrap(),
            &std::collections::HashSet::new()
        )
        .iter()
        .all(|g| g.kind != "exact"));
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn raw_fixtures_index_through_the_scan_pipeline_and_unsupported_heic_is_recorded_not_crashed(
    ) {
        let dng = fixture_dir().join("sample.dng");
        let nef = fixture_dir().join("sample.nef");
        if !dng.is_file() || !nef.is_file() {
            eprintln!("fixtures/sample.dng or fixtures/sample.nef not present; skipping end-to-end RAW scan verification");
            return;
        }
        let dir = test_dir();
        let library = dir.join("library");
        std::fs::create_dir_all(&library).unwrap();
        std::fs::copy(&dng, library.join("IMG_001.DNG")).unwrap();
        std::fs::copy(&nef, library.join("DSC_0001.NEF")).unwrap();
        let heic = fixture_dir().join("sample.heic");
        if heic.is_file() {
            std::fs::copy(&heic, library.join("IMG_0002.HEIC")).unwrap();
        }
        let pool = crate::db::init_db(&dir).await.unwrap();
        let thumbs = dir.join("thumbs");
        let nef_path = library.join("DSC_0001.NEF");
        let jpg_path = library.join("DSC_0001.jpg");
        if nef_path.is_file() && !jpg_path.is_file() {
            // Simulate the camera's identical-shot JPEG twin: same stem, close
            // to the RAW's picture, so without pairing it would be flagged as a
            // near-duplicate of the RAW's own decoded preview.
            let raw_image = crate::ingest::load_normalized(&nef_path, 1024).unwrap();
            std::fs::write(&jpg_path, crate::ingest::encode_jpeg(&raw_image).unwrap()).unwrap();
        }
        for path in [
            library.join("IMG_001.DNG"),
            library.join("DSC_0001.NEF"),
            library.join("IMG_0002.HEIC"),
            library.join("DSC_0001.jpg"),
        ] {
            if path.is_file() {
                let _ = refresh_photo(&pool, &path, &thumbs).await;
            }
        }
        let photos = crate::db::all_photos(&pool).await.unwrap();
        assert!(
            photos.iter().any(|p| p.filename == "IMG_001.DNG"),
            "Canon DNG must index through the RAW pipeline"
        );
        assert!(
            photos.iter().any(|p| p.filename == "DSC_0001.NEF"),
            "Nikon NEF must index through the RAW pipeline"
        );
        let dng = photos.iter().find(|p| p.filename == "IMG_001.DNG").unwrap();
        assert!(dng.width.is_some_and(|w| w > 0) && dng.height.is_some_and(|h| h > 0));
        assert!(!dng.thumbnail_path.as_deref().unwrap_or("").is_empty());
        // HEIC handling: on macOS it's fully decodable, on other platforms it's metadata only.
        let heic = photos.iter().find(|p| p.filename == "IMG_0002.HEIC");
        if heic.is_some() {
            #[cfg(target_os = "macos")]
            {
                assert!(
                    heic.unwrap().thumbnail_path.is_some(),
                    "HEIC should be fully decodable on macOS"
                );
            }
            #[cfg(not(target_os = "macos"))]
            {
                assert!(
                    heic.unwrap().thumbnail_path.is_none(),
                    "HEIC pixels are not decodable on non-macOS builds"
                );
            }
        } else {
            // If HEIC didn't index, check it was recorded as a scan error (non-macOS platforms)
            #[cfg(not(target_os = "macos"))]
            {
                let session_id: i64 =
                    sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM scan_sessions")
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                let reason: Option<String> =
                    sqlx::query_scalar("SELECT reason FROM scan_errors WHERE session_id = ? LIMIT 1")
                        .bind(session_id)
                        .fetch_optional(&pool)
                        .await
                        .unwrap();
                assert!(
                    reason.is_some_and(|r| r.contains("not supported")),
                    "unsupported HEIC must be recorded with its reason"
                );
            }
        }
        // The NEF and its same-stem JPEG become ONE logical photo. Without the
        // pairing they are near-duplicates; with it neither may be offered up.
        crate::db::build_logical_pairs(&pool).await.unwrap();
        let pairs = crate::db::get_logical_pairs(&pool).await.unwrap();
        let nef_photo = crate::db::all_photos(&pool)
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.filename == "DSC_0001.NEF")
            .unwrap();
        if let Some(pair) = pairs
            .iter()
            .find(|pair| pair.primary.id == nef_photo.id || pair.paired.id == nef_photo.id)
        {
            assert_eq!(
                pair.paired.filename, "DSC_0001.jpg",
                "the JPEG twin must be the paired side"
            );
            let paired_ids = crate::db::logical_pair_photo_ids(&pool).await.unwrap();
            let without = duplicate_groups(
                &crate::db::all_photos(&pool).await.unwrap(),
                &std::collections::HashSet::new(),
            );
            assert!(
                without
                    .iter()
                    .any(|g| g.photos.iter().any(|p| p.id == nef_photo.id)),
                "an unpaired RAW+JPEG twin is a visual duplicate"
            );
            let with = duplicate_groups(&crate::db::all_photos(&pool).await.unwrap(), &paired_ids);
            assert!(
                !with
                    .iter()
                    .any(|g| g.photos.iter().any(|p| paired_ids.contains(&p.id))),
                "paired sides are never duplicate candidates"
            );
        } else {
            panic!("NEF + same-stem JPEG in the same folder must form a logical pair");
        }
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn rescan_preserves_unchanged_recommendations_and_rolls_back_failures() {
        let dir = test_dir();
        let pool = crate::db::init_db(&dir).await.unwrap();
        for id in ["a", "b"] {
            sqlx::query("INSERT INTO photos (id, absolute_path, filename, file_size, created_at, perceptual_hash, moment_id) VALUES (?, ?, ?, 1, '2026-09-17T00:00:00Z', '0000000000000000', 'legacy-moment')")
                .bind(id).bind(id).bind(id).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO moment_groups VALUES ('legacy-moment', NULL, NULL, 2)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO recommendations (photo_id, moment_id, decision, confidence, reasoning, compared_to) VALUES ('b', 'legacy-moment', 'REMOVE', 0.85, '[]', 'a')").execute(&pool).await.unwrap();
        generate_moments(&pool).await.unwrap();
        generate_moments(&pool).await.unwrap();
        let moment: String = sqlx::query_scalar("SELECT moment_id FROM photos WHERE id = 'b'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(moment, "legacy-moment");
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM recommendations WHERE moment_id = 'legacy-moment'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);

        sqlx::query("CREATE TRIGGER fail_moments BEFORE INSERT ON moment_groups BEGIN SELECT RAISE(ABORT, 'test failure'); END").execute(&pool).await.unwrap();
        assert!(generate_moments(&pool).await.is_err());
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM photos WHERE moment_id = 'legacy-moment'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 2);
        sqlx::query("DROP TRIGGER fail_moments")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query("INSERT INTO photos (id, absolute_path, filename, file_size, created_at, perceptual_hash) VALUES ('c', 'c', 'c', 1, '2026-09-17T00:00:00Z', '0000000000000000')").execute(&pool).await.unwrap();
        generate_moments(&pool).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM recommendations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        let count: i64 = sqlx::query_scalar("SELECT photo_count FROM moment_groups")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 3);
        let old_moment: String = sqlx::query_scalar("SELECT id FROM moment_groups")
            .fetch_one(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO recommendations (photo_id, moment_id, decision, confidence, reasoning, compared_to) VALUES ('b', ?, 'REMOVE', 0.85, '[]', 'a')")
            .bind(&old_moment).execute(&pool).await.unwrap();
        sqlx::query("DELETE FROM photos WHERE id = 'c'")
            .execute(&pool)
            .await
            .unwrap();
        generate_moments(&pool).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM recommendations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn identical_files_have_identical_sha256() {
        let dir = test_dir();
        let original = dir.join("original.jpg");
        let copy = dir.join("copy.jpg");
        let image = image::RgbImage::from_pixel(32, 32, image::Rgb([30, 120, 220]));
        image.save(&original).unwrap();
        std::fs::copy(&original, &copy).unwrap();

        assert_eq!(
            calculate_sha256(&original).unwrap(),
            calculate_sha256(&copy).unwrap()
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn image_processing_creates_a_readable_thumbnail() {
        let dir = test_dir();
        let original = dir.join("photo.png");
        let thumbs = dir.join("thumbs");
        let image = image::RgbImage::from_fn(100, 80, |x, y| {
            image::Rgb([(x * 2) as u8, (y * 3) as u8, 90])
        });
        image.save(&original).unwrap();

        let processed = process_image(&original, &thumbs, "photo-id").unwrap();

        assert!(Path::new(&processed.thumbnail_path).is_file());
        assert!(image::open(&processed.thumbnail_path).is_ok());
        assert_eq!(processed.perceptual_hash.len(), 16);
        assert_eq!((processed.width, processed.height), (100, 80));

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn similar_hashes_have_small_hamming_distance() {
        assert_eq!(
            hamming_distance("0000000000000000", "0000000000000001"),
            Some(1)
        );
        assert_eq!(
            hamming_distance("ffffffffffffffff", "0000000000000000"),
            Some(64)
        );
    }

    /// Phase 0 test: a corrupt/truncated JPEG must not panic the process.
    /// The `process_image` wrapper catches the panic and returns an error.
    #[test]
    fn corrupt_file_does_not_crash() {
        let dir = test_dir();
        let corrupt = dir.join("corrupt.jpg");
        let thumbs = dir.join("thumbs");
        std::fs::write(&corrupt, b"this is not a valid jpeg file at all").unwrap();
        let result = process_image(&corrupt, &thumbs, "corrupt-id");
        assert!(
            result.is_err(),
            "corrupt file must produce an error, not a panic"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Phase 0 test: process_image_inner returns Ok for a valid image,
    /// confirming the catch_unwind wrapper does not mask success.
    #[test]
    fn valid_image_processes_through_catch_unwind_wrapper() {
        let dir = test_dir();
        let original = dir.join("photo.png");
        let thumbs = dir.join("thumbs");
        let image = image::RgbImage::from_fn(100, 80, |x, y| {
            image::Rgb([(x * 2) as u8, (y * 3) as u8, 90])
        });
        image.save(&original).unwrap();
        let result = process_image(&original, &thumbs, "photo-id");
        assert!(result.is_ok(), "valid image must process successfully");
        assert_eq!(result.unwrap().perceptual_hash.len(), 16);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
