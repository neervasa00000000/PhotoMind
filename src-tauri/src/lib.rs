use serde_json::json;
use sqlx::SqlitePool;
use std::path::PathBuf;
use tauri::{Manager, State};
use tracing::info;

pub mod avif;
pub mod bin;
pub mod db;
pub mod decision_reasons;
pub mod decision_weights;
pub mod decisions;
pub mod eye_validation;
pub mod face_analysis;
pub mod face_db;
pub mod faces;
pub mod guard;
pub mod heic_macos;
pub mod ingest;
pub mod junk;
pub mod models;
pub mod people;
pub mod scanner;
pub mod similarity;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

struct AppState {
    db_pool: SqlitePool,
    thumb_dir: PathBuf,
    scan_lock_path: PathBuf,
    operation_gate: Arc<tokio::sync::Mutex<()>>,
    is_scanning: Arc<AtomicBool>,
    is_paused: Arc<AtomicBool>,
    is_cancelled: Arc<AtomicBool>,
}

fn acquire_scan_lock(path: &std::path::Path) -> Result<std::fs::File, String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| format!("Cannot open scan lock: {e}"))?;
    file.try_lock().map_err(|e| format!("Another PhotoMind instance may be scanning. Close it or wait for its scan to finish, then try again. ({e})"))?;
    Ok(file)
}

// Background Bin refreshes are short operations, not competing app instances.
// Queue foreground work behind them before acquiring the cross-process lock.
async fn acquire_operation_lock(
    path: &std::path::Path,
    gate: &Arc<tokio::sync::Mutex<()>>,
) -> Result<(tokio::sync::OwnedMutexGuard<()>, std::fs::File), String> {
    let guard = tokio::time::timeout(std::time::Duration::from_secs(5), gate.clone().lock_owned())
        .await
        .map_err(|_| {
            "PhotoMind is busy with another operation. Wait for it to finish and try again."
                .to_string()
        })?;
    for attempt in 0..40 {
        match acquire_scan_lock(path) {
            Ok(file) => return Ok((guard, file)),
            Err(error) if attempt == 39 => return Err(error),
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
        }
    }
    unreachable!()
}

async fn foreground_lock(
    state: &AppState,
) -> Result<(tokio::sync::OwnedMutexGuard<()>, std::fs::File), String> {
    if state.is_scanning.load(Ordering::SeqCst) {
        return Err("A scan is already running. Cancel it or wait for completion before starting another operation.".into());
    }
    acquire_operation_lock(&state.scan_lock_path, &state.operation_gate).await
}

#[tauri::command]
async fn start_scan(
    path: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let scan_path =
        std::fs::canonicalize(&path).map_err(|e| format!("Cannot open scan folder: {e}"))?;
    if !scan_path.is_dir() {
        return Err("Choose a folder to scan".to_string());
    }
    let scan_lock = foreground_lock(&state).await?;
    if state
        .is_scanning
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("A scan is already running".to_string());
    }
    let path = scan_path.to_string_lossy().to_string();
    let is_scanning = state.is_scanning.clone();
    let pool = state.db_pool.clone();
    let thumb_dir = state.thumb_dir.clone();

    // Reset flags
    state.is_paused.store(false, Ordering::SeqCst);
    state.is_cancelled.store(false, Ordering::SeqCst);

    let is_paused = state.is_paused.clone();
    let is_cancelled = state.is_cancelled.clone();

    // Spawn background task
    tauri::async_runtime::spawn(async move {
        let scan_result = tauri::async_runtime::spawn(scanner::run_scan(
            path,
            pool.clone(),
            thumb_dir,
            app_handle.clone(),
            is_paused,
            is_cancelled.clone(),
        ))
        .await;
        drop(scan_lock);
        is_scanning.store(false, Ordering::SeqCst);
        if let Err(error) = &scan_result {
            use tauri::Emitter;
            let _ = app_handle.emit("scan-error", json!({"message": format!("Scan stopped unexpectedly: {error}. You can scan again.")}));
            return;
        }
    });

    Ok(())
}

#[tauri::command]
fn pause_scan(state: State<'_, AppState>) {
    state.is_paused.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn resume_scan(state: State<'_, AppState>) {
    state.is_paused.store(false, Ordering::SeqCst);
}

#[tauri::command]
fn cancel_scan(state: State<'_, AppState>) {
    state.is_cancelled.store(true, Ordering::SeqCst);
}

#[tauri::command]
async fn get_photos(
    state: State<'_, AppState>,
    offset: Option<i64>,
    limit: Option<i64>,
) -> Result<Vec<models::Photo>, String> {
    db::photo_page(&state.db_pool, offset.unwrap_or(0), limit.unwrap_or(200))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_duplicate_groups(
    state: State<'_, AppState>,
) -> Result<Vec<models::DuplicateGroup>, String> {
    // Keep pairings fresh (stale pairs pruned, new pairs built) so duplicates
    // never misrepresent a RAW+JPEG same-shot pair as two photos.
    db::build_logical_pairs(&state.db_pool)
        .await
        .map_err(|e| e.to_string())?;
    let photos = db::all_photos(&state.db_pool)
        .await
        .map_err(|e| e.to_string())?;
    let paired = db::logical_pair_photo_ids(&state.db_pool)
        .await
        .map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || scanner::duplicate_groups(&photos, &paired))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_logical_pairs(state: State<'_, AppState>) -> Result<Vec<models::LogicalPair>, String> {
    db::build_logical_pairs(&state.db_pool)
        .await
        .map_err(|e| e.to_string())?;
    db::get_logical_pairs(&state.db_pool)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_recommendations(
    state: State<'_, AppState>,
) -> Result<Vec<models::Recommendation>, String> {
    sqlx::query_as("SELECT r.photo_id, r.moment_id, r.decision, r.confidence, r.reasoning, r.compared_to, r.reason_codes, r.best_of_group FROM recommendations r JOIN photos p ON p.id = r.photo_id WHERE NOT EXISTS (SELECT 1 FROM bin_entries b WHERE b.photo_id = p.id)")
        .fetch_all(&state.db_pool).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_moment_groups(
    state: State<'_, AppState>,
) -> Result<Vec<models::MomentGroupWithPhotos>, String> {
    let moments: Vec<models::MomentGroup> = sqlx::query_as(
        "SELECT id, start_time, end_time, photo_count FROM moment_groups ORDER BY start_time DESC",
    )
    .fetch_all(&state.db_pool)
    .await
    .map_err(|e| e.to_string())?;

    // Three queries for the entire view instead of two extra queries per group.
    let photos = db::all_photos(&state.db_pool)
        .await
        .map_err(|e| e.to_string())?;
    let recommendations = get_recommendations(state).await?;
    let mut by_moment = std::collections::HashMap::<String, Vec<models::Photo>>::new();
    let mut recs_by_moment =
        std::collections::HashMap::<String, Vec<models::Recommendation>>::new();
    for p in photos {
        if let Some(id) = &p.moment_id {
            by_moment.entry(id.clone()).or_default().push(p);
        }
    }
    for rec in recommendations {
        if let Some(id) = &rec.moment_id {
            recs_by_moment.entry(id.clone()).or_default().push(rec);
        }
    }
    Ok(moments
        .into_iter()
        .map(|moment| {
            let mut photos = by_moment.remove(&moment.id).unwrap_or_default();
            photos.sort_by(|a, b| {
                a.capture_time
                    .as_ref()
                    .or(a.created_at.as_ref())
                    .cmp(&b.capture_time.as_ref().or(b.created_at.as_ref()))
                    .then(a.id.cmp(&b.id))
            });
            let recommendations = recs_by_moment.remove(&moment.id).unwrap_or_default();
            models::MomentGroupWithPhotos {
                moment,
                photos,
                recommendations,
            }
        })
        .collect())
}

#[tauri::command]
async fn get_moment_face_summary(
    moment_id: String,
    state: State<'_, AppState>,
) -> Result<faces::MomentFaceSummary, String> {
    faces::moment_face_summary(&state.db_pool, &moment_id).await
}

#[tauri::command]
async fn get_photo_face_summaries(
    state: State<'_, AppState>,
) -> Result<Vec<models::PhotoFaceSummary>, String> {
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT photo_id, face_count FROM photo_face_analysis")
            .fetch_all(&state.db_pool)
            .await
            .map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(rows.len());
    for (photo_id, face_count) in rows {
        let faces = face_db::load_faces_for_photo(&state.db_pool, &photo_id)
            .await
            .map_err(|e| e.to_string())?;
        let metrics = crate::face_analysis::types::GroupFaceMetrics::from_faces(&faces);
        let average_smile = metrics.average_expression_score;
        let average_face_sharpness = metrics.average_face_sharpness;
        out.push(models::PhotoFaceSummary {
            photo_id,
            face_count: face_count.max(0) as usize,
            closed_eye_warning: metrics.closed_eye_count > 0,
            uncertain_eyes: metrics.uncertain_eye_count > 0,
            average_face_sharpness,
            average_smile,
        });
    }
    Ok(out)
}

#[derive(serde::Serialize)]
struct PersonGroup {
    key: String,
    faces: Vec<PersonMatch>,
}

#[derive(serde::Serialize)]
struct PeopleOverview {
    groups: Vec<PersonGroup>,
    photos_checked: i64,
    faces_detected: i64,
    failed: usize,
}

#[derive(serde::Serialize)]
struct PersonMatch {
    face_index: i64,
    photo: models::Photo,
}

#[tauri::command]
async fn get_person_groups(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<PeopleOverview, String> {
    use tauri::Emitter;
    let pending: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT p.id, p.absolute_path, p.sha256 FROM photos p
         LEFT JOIN photo_face_analysis f ON f.photo_id = p.id
         WHERE NOT EXISTS (SELECT 1 FROM bin_entries b WHERE b.photo_id = p.id)
           AND p.sha256 IS NOT NULL
           AND (f.photo_id IS NULL OR f.analyzer_version != ?)
         ORDER BY p.absolute_path",
    )
    .bind(face_analysis::ANALYZER_VERSION)
    .fetch_all(&state.db_pool)
    .await
    .map_err(|e| e.to_string())?;
    let mut failed = 0;
    if !pending.is_empty() {
        let lock = foreground_lock(&state).await?;
        for (index, (id, path, sha256)) in pending.iter().enumerate() {
            let _ = app_handle.emit(
                "people-progress",
                json!({
                    "stage": "detecting", "processed": index, "total": pending.len()
                }),
            );
            if let Err(error) = scanner::analyze_faces_for_photo(
                &state.db_pool,
                id,
                std::path::Path::new(path),
                sha256,
            )
            .await
            {
                failed += 1;
                tracing::warn!(photo_id = %id, stage = "face", error = %error, "Face reanalysis failed");
            }
        }
        drop(lock);
    }
    let _ = app_handle.emit("people-progress", json!({ "stage": "grouping" }));
    let groups = people::likely_same_people(&state.db_pool).await?;
    let photos_checked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM photo_face_analysis f JOIN photos p ON p.id = f.photo_id
         WHERE f.analyzer_version = ? AND NOT EXISTS
         (SELECT 1 FROM bin_entries b WHERE b.photo_id = p.id)",
    )
    .bind(face_analysis::ANALYZER_VERSION)
    .fetch_one(&state.db_pool)
    .await
    .map_err(|e| e.to_string())?;
    let faces_detected: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM photo_faces f JOIN photo_face_analysis a ON a.photo_id = f.photo_id
         WHERE a.analyzer_version = ? AND NOT EXISTS
         (SELECT 1 FROM bin_entries b WHERE b.photo_id = f.photo_id)",
    )
    .bind(face_analysis::ANALYZER_VERSION)
    .fetch_one(&state.db_pool)
    .await
    .map_err(|e| e.to_string())?;
    let photos = db::all_photos(&state.db_pool)
        .await
        .map_err(|e| e.to_string())?;
    let by_id: std::collections::HashMap<_, _> =
        photos.into_iter().map(|p| (p.id.clone(), p)).collect();
    let groups = groups
        .into_iter()
        .filter_map(|group| {
            let faces: Vec<_> = group
                .faces
                .iter()
                .filter_map(|face| {
                    by_id.get(&face.photo_id).cloned().map(|photo| PersonMatch {
                        face_index: face.face_index,
                        photo,
                    })
                })
                .collect();
            (faces.len() >= 2).then_some(PersonGroup {
                key: group.key,
                faces,
            })
        })
        .collect();
    Ok(PeopleOverview {
        groups,
        photos_checked,
        faces_detected,
        failed,
    })
}

#[tauri::command]
async fn get_face_preview(
    photo_id: String,
    face_index: i64,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let jpeg = people::face_preview(&state.db_pool, &photo_id, face_index).await?;
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    Ok(format!("data:image/jpeg;base64,{}", STANDARD.encode(jpeg)))
}

#[tauri::command]
async fn get_duplicate_moments(
    state: State<'_, AppState>,
) -> Result<Vec<guard::DuplicateMomentPair>, String> {
    guard::find_duplicate_moments(&state.db_pool).await
}

#[tauri::command]
async fn get_stats(state: State<'_, AppState>) -> Result<models::DashboardStats, String> {
    let total: (i64, i64) = sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(file_size), 0) FROM photos WHERE NOT EXISTS (SELECT 1 FROM bin_entries b WHERE b.photo_id = photos.id)")
        .fetch_one(&state.db_pool).await.unwrap_or((0, 0));

    let duplicates: (i64, i64) = sqlx::query_as(
        r#"SELECT COALESCE(SUM(c - 1), 0), COALESCE(SUM(fs), 0) FROM (
            SELECT COUNT(*) as c, (COUNT(*) - 1) * MAX(file_size) as fs 
            FROM photos WHERE sha256 IS NOT NULL AND sha256 != '' AND NOT EXISTS (SELECT 1 FROM bin_entries b WHERE b.photo_id = photos.id) GROUP BY sha256 HAVING COUNT(*) > 1
        )"#
    )
    .fetch_one(&state.db_pool).await.unwrap_or((0, 0));

    let removals: (i64, i64) = sqlx::query_as(
        r#"SELECT COUNT(*), COALESCE(SUM(p.file_size), 0) 
           FROM recommendations r JOIN photos p ON r.photo_id = p.id 
           WHERE r.decision = 'REMOVE' AND NOT EXISTS (SELECT 1 FROM bin_entries b WHERE b.photo_id = p.id) AND NOT EXISTS (SELECT 1 FROM protected_photos k WHERE k.photo_id = p.id)"#
    )
    .fetch_one(&state.db_pool).await.unwrap_or((0, 0));

    let reviews: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM recommendations WHERE decision = 'REVIEW'")
            .fetch_one(&state.db_pool)
            .await
            .unwrap_or((0,));

    Ok(models::DashboardStats {
        total_photos: total.0,
        total_bytes: total.1,
        exact_duplicates: duplicates.0,
        exact_duplicates_bytes: duplicates.1,
        recommended_removals: removals.0,
        recommended_removals_bytes: removals.1,
        needs_review: reviews.0,
    })
}

#[tauri::command]
async fn move_to_trash(
    photo_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<models::FileActionReport, String> {
    let _lock = foreground_lock(&state).await?;
    let mut report = models::FileActionReport::default();
    for id in photo_ids
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
    {
        let fallback = state.thumb_dir.parent().unwrap().join("bin");
        match bin::move_one(&state.db_pool, &id, move |path| {
            bin::system_trash(path, &fallback)
        })
        .await
        {
            Ok(bytes) => {
                report.bytes += bytes;
                report.photo_ids.push(id);
            }
            Err(error) => report.errors.push(error),
        }
    }
    if let Err(error) = scanner::generate_moments(&state.db_pool).await {
        report.errors.push(format!(
            "The file operation completed, but groups could not be refreshed: {error}"
        ));
    }
    Ok(report)
}

#[tauri::command]
async fn refresh_library(state: State<'_, AppState>) -> Result<bool, String> {
    // Skip foreground work; never collect a preview while a decoder is writing it.
    let Ok(_gate) = state.operation_gate.clone().try_lock_owned() else {
        return Ok(false);
    };
    let Ok(_lock) = acquire_scan_lock(&state.scan_lock_path) else {
        return Ok(false);
    };
    let before: (i64, i64) =
        sqlx::query_as("SELECT (SELECT COUNT(*) FROM photos), (SELECT COUNT(*) FROM bin_entries)")
            .fetch_one(&state.db_pool)
            .await
            .map_err(|e| e.to_string())?;
    bin::reconcile(&state.db_pool, &state.thumb_dir).await?;
    db::reconcile_active_index(&state.db_pool)
        .await
        .map_err(|e| e.to_string())?;
    let after: (i64, i64) =
        sqlx::query_as("SELECT (SELECT COUNT(*) FROM photos), (SELECT COUNT(*) FROM bin_entries)")
            .fetch_one(&state.db_pool)
            .await
            .map_err(|e| e.to_string())?;
    if before != after {
        scanner::generate_moments(&state.db_pool)
            .await
            .map_err(|e| e.to_string())?;
        decisions::update(&state.db_pool, None)
            .await
            .map_err(|e| e.to_string())?;
    }
    db::gc_orphaned_thumbnails(&state.db_pool, &state.thumb_dir)
        .await
        .map_err(|e| e.to_string())?;
    Ok(before != after)
}

#[tauri::command]
async fn get_bin(state: State<'_, AppState>) -> Result<Vec<models::BinPhoto>, String> {
    if let Ok(_gate) = state.operation_gate.clone().try_lock_owned() {
        if let Ok(_lock) = acquire_scan_lock(&state.scan_lock_path) {
            bin::reconcile(&state.db_pool, &state.thumb_dir).await?;
        }
    }
    bin::list(&state.db_pool).await
}

#[tauri::command]
async fn recover_photos(
    photo_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<models::FileActionReport, String> {
    let _lock = foreground_lock(&state).await?;
    let mut report = models::FileActionReport::default();
    for id in photo_ids
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
    {
        match bin::recover_one(&state.db_pool, &id).await {
            Ok(()) => report.photo_ids.push(id),
            Err(error) => report.errors.push(error),
        }
    }
    if let Err(error) = scanner::generate_moments(&state.db_pool).await {
        report.errors.push(format!(
            "The file operation completed, but groups could not be refreshed: {error}"
        ));
    }
    Ok(report)
}

#[tauri::command]
async fn delete_permanently(
    photo_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<models::FileActionReport, String> {
    let _lock = foreground_lock(&state).await?;
    let mut report = models::FileActionReport::default();
    for id in photo_ids
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
    {
        match bin::purge_one(&state.db_pool, &id).await {
            Ok(()) => report.photo_ids.push(id),
            Err(error) => report.errors.push(error),
        }
    }
    Ok(report)
}

#[tauri::command]
async fn choose_keeper(
    photo_id: String,
    group_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let _lock = foreground_lock(&state).await?;
    bin::choose_keeper(&state.db_pool, &photo_id, &group_ids).await
}

#[tauri::command]
async fn get_photo_preview(id: String, state: State<'_, AppState>) -> Result<String, String> {
    let path: String = sqlx::query_scalar("SELECT COALESCE(b.trash_path, p.absolute_path) FROM photos p LEFT JOIN bin_entries b ON b.photo_id = p.id WHERE p.id = ?")
        .bind(&id).fetch_one(&state.db_pool).await.map_err(|e| e.to_string())?;
    let jpeg = tokio::task::spawn_blocking(move || bin::large_preview(&path))
        .await
        .map_err(|e| e.to_string())??;
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    Ok(format!("data:image/jpeg;base64,{}", STANDARD.encode(jpeg)))
}

#[tauri::command]
async fn get_thumbnail_data_url(id: String, state: State<'_, AppState>) -> Result<String, String> {
    let row: Option<(Option<String>, String)> =
        sqlx::query_as("SELECT thumbnail_path, absolute_path FROM photos WHERE id = ?")
            .bind(&id)
            .fetch_optional(&state.db_pool)
            .await
            .map_err(|e| e.to_string())?;

    let (thumb_path, original_path) = row.ok_or_else(|| "Photo not found".to_string())?;

    let existing = thumb_path.as_ref().and_then(|path| {
        let p = std::path::Path::new(path);
        if p.is_file() {
            std::fs::read(p).ok()
        } else {
            None
        }
    });

    let jpeg_bytes = match existing {
        Some(bytes) => bytes,
        None => {
            let original = original_path.clone();
            let generated = tokio::task::spawn_blocking(move || {
                scanner::generate_jpeg_thumbnail(&original, 256).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| format!("Preview worker failed: {e}"))?
            .map_err(|e| format!("Could not read photo: {e}"))?;

            let dest = state.thumb_dir.join(format!("{id}.jpg"));
            let _ = std::fs::create_dir_all(&state.thumb_dir);
            let _ = std::fs::write(&dest, &generated);
            let dest_str = dest.to_string_lossy().to_string();
            let _ = sqlx::query("UPDATE photos SET thumbnail_path = ? WHERE id = ?")
                .bind(&dest_str)
                .bind(&id)
                .execute(&state.db_pool)
                .await;
            generated
        }
    };

    use base64::{engine::general_purpose::STANDARD, Engine as _};
    Ok(format!(
        "data:image/jpeg;base64,{}",
        STANDARD.encode(jpeg_bytes)
    ))
}

#[tauri::command]
async fn log_user_decision(
    photo_id: String,
    recommended: String,
    actual: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO user_decisions (photo_id, recommended_decision, actual_decision) VALUES (?, ?, ?)"
    )
    .bind(&photo_id)
    .bind(&recommended)
    .bind(&actual)
    .execute(&state.db_pool)
    .await
    .map_err(|e| e.to_string())?;

    // Apply the user's actual override locally
    sqlx::query("UPDATE recommendations SET decision = ? WHERE photo_id = ?")
        .bind(&actual)
        .bind(&photo_id)
        .execute(&state.db_pool)
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[cfg(target_os = "macos")]
use sys_info::{mem_info, MemInfo};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Structured logging for crash diagnosis
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(true)
        .with_file(true)
        .with_line_number(true)
        .json()
        .with_writer(std::io::stderr)
        .try_init()
        .ok();

    #[cfg(target_os = "macos")]
    {
        if let Ok(MemInfo {
            total: _, avail, ..
        }) = mem_info()
        {
            info!("Available memory: {} KB", avail);
        }
    }

    info!("PhotoMind starting");

    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let default_data_dir = app.path().app_data_dir()?;
            let qa_data_dir = std::env::var_os("PHOTOMIND_QA_DATA_DIR").map(PathBuf::from);
            let qa_folder = std::env::var_os("PHOTOMIND_QA_SCAN_FOLDER").map(PathBuf::from);
            if qa_folder.is_some() && qa_data_dir.is_none() {
                return Err(std::io::Error::other(
                    "QA scanning requires an isolated PHOTOMIND_QA_DATA_DIR",
                )
                .into());
            }
            let app_data_dir = qa_data_dir.unwrap_or(default_data_dir);
            let thumb_dir = app_data_dir.join("thumbnails");
            std::fs::create_dir_all(&thumb_dir).ok();

            std::fs::create_dir_all(&app_data_dir)?;
            let startup_lock = acquire_scan_lock(&app_data_dir.join("scan.lock"))
                .map_err(std::io::Error::other)?;
            let pool = tauri::async_runtime::block_on(async {
                let pool = db::init_db(&app_data_dir).await?;
                bin::reconcile(&pool, &thumb_dir)
                    .await
                    .map_err(std::io::Error::other)?;
                // Fresh launch is the user's requested default. Bin journals and
                // their previews survive; no original photo is modified.
                db::reset_scan_library(&pool, &thumb_dir).await?;
                Ok::<_, Box<dyn std::error::Error>>(pool)
            })?;
            drop(startup_lock);

            info!("Database initialized");

            if let Ok(resource_dir) = app.path().resource_dir() {
                std::env::set_var("PHOTOMIND_RESOURCE_DIR", resource_dir);
            }

            app.manage(AppState {
                db_pool: pool,
                thumb_dir,
                scan_lock_path: app_data_dir.join("scan.lock"),
                operation_gate: Arc::new(tokio::sync::Mutex::new(())),
                is_scanning: Arc::new(AtomicBool::new(false)),
                is_paused: Arc::new(AtomicBool::new(false)),
                is_cancelled: Arc::new(AtomicBool::new(false)),
            });

            // Only explicit isolated QA runs auto-scan. Normal launches never
            // reload a previous folder or recreate the cleared scan results.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let roots = if let Some(folder) = qa_folder {
                    vec![(folder.to_string_lossy().into_owned(), "qa".into())]
                } else {
                    Vec::<(String, String)>::new()
                };
                for (path, status) in roots {
                    if status == "cancelled" || !std::path::Path::new(&path).is_dir() {
                        continue;
                    }
                    // An explicit new scan takes precedence over startup verification.
                    if handle
                        .state::<AppState>()
                        .is_scanning
                        .load(Ordering::SeqCst)
                    {
                        return;
                    }
                    if let Err(error) =
                        start_scan(path, handle.state::<AppState>(), handle.clone()).await
                    {
                        eprintln!("Startup verification skipped: {error}");
                        return;
                    }
                    while handle
                        .state::<AppState>()
                        .is_scanning
                        .load(Ordering::SeqCst)
                    {
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                    if handle
                        .state::<AppState>()
                        .is_cancelled
                        .load(Ordering::SeqCst)
                    {
                        return;
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_scan,
            pause_scan,
            resume_scan,
            cancel_scan,
            get_photos,
            get_duplicate_groups,
            get_logical_pairs,
            get_moment_groups,
            get_recommendations,
            get_stats,
            move_to_trash,
            get_bin,
            refresh_library,
            recover_photos,
            delete_permanently,
            choose_keeper,
            get_photo_preview,
            log_user_decision,
            get_thumbnail_data_url,
            get_moment_face_summary,
            get_photo_face_summaries,
            get_person_groups,
            get_face_preview,
            get_duplicate_moments
        ])
        .run(tauri::generate_context!());
    if let Err(error) = result {
        tracing::error!(stage = "application", error = %error, "PhotoMind could not start");
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn foreground_waits_for_bin_refresh_then_acquires_scan_lock() {
        let path =
            std::env::temp_dir().join(format!("photomind-refresh-lock-{}", uuid::Uuid::new_v4()));
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        let refresh_gate = gate.clone().lock_owned().await;
        let refresh_file = acquire_scan_lock(&path).unwrap();
        let scan_path = path.clone();
        let scan_gate = gate.clone();
        let pending =
            tokio::spawn(async move { acquire_operation_lock(&scan_path, &scan_gate).await });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!pending.is_finished());
        drop(refresh_file);
        drop(refresh_gate);
        let scan = pending.await.unwrap().unwrap();
        assert!(gate.clone().try_lock_owned().is_err());
        assert!(acquire_scan_lock(&path).is_err());
        drop(scan);
        assert!(acquire_operation_lock(&path, &gate).await.is_ok());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn scan_lock_excludes_other_instances_and_releases_after_drop() {
        let path = std::env::temp_dir().join(format!("photomind-lock-{}", uuid::Uuid::new_v4()));
        let first = acquire_scan_lock(&path).unwrap();
        assert!(acquire_scan_lock(&path).is_err());
        drop(first);
        drop(acquire_scan_lock(&path).unwrap());
        std::fs::remove_file(path).unwrap();
    }
}
