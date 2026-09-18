use crate::{
    db,
    models::{BinPhoto, Photo},
};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
pub fn system_trash(path: &Path, _fallback: &Path) -> Result<PathBuf, String> {
    use objc2_foundation::{NSFileManager, NSString, NSURL};
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let mut resulting = None;
    NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, Some(&mut resulting))
        .map_err(|e| format!("Could not move photo to macOS Trash: {e}"))?;
    resulting.and_then(|url| url.path()).map(|path| PathBuf::from(path.to_string()))
        .ok_or_else(|| "macOS moved the photo but did not return its Trash location. Use Finder’s Put Back to recover it.".to_string())
}

#[cfg(not(target_os = "macos"))]
pub fn system_trash(path: &Path, fallback: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(fallback).map_err(|e| e.to_string())?;
    let target = fallback.join(format!(
        "{}-{}",
        uuid::Uuid::new_v4(),
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    move_no_replace(path, &target)?;
    Ok(target)
}

#[cfg(target_os = "macos")]
fn move_no_replace(source: &Path, target: &Path) -> Result<(), String> {
    use objc2_foundation::{NSFileManager, NSString, NSURL};
    let source = NSURL::fileURLWithPath(&NSString::from_str(&source.to_string_lossy()));
    let target = NSURL::fileURLWithPath(&NSString::from_str(&target.to_string_lossy()));
    NSFileManager::defaultManager()
        .moveItemAtURL_toURL_error(&source, &target)
        .map_err(|e| format!("Could not recover photo: {e}"))
}

#[cfg(not(target_os = "macos"))]
fn move_no_replace(source: &Path, target: &Path) -> Result<(), String> {
    // create_new never replaces an existing destination, including a racing file.
    use std::io::Write;
    let mut input = std::fs::File::open(source).map_err(|e| e.to_string())?;
    let mut output = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(target)
        .map_err(|e| e.to_string())?;
    let copied = (|| {
        std::io::copy(&mut input, &mut output)?;
        output.flush()?;
        output.sync_all()?;
        std::fs::set_permissions(target, input.metadata()?.permissions())?;
        Ok::<_, std::io::Error>(())
    })();
    if let Err(error) = copied {
        drop(output);
        let _ = std::fs::remove_file(target);
        return Err(error.to_string());
    }
    if crate::scanner::calculate_sha256(source).map_err(|e| e.to_string())?
        != crate::scanner::calculate_sha256(target).map_err(|e| e.to_string())?
    {
        let _ = std::fs::remove_file(target);
        return Err("The photo changed while moving. The source was preserved.".to_string());
    }
    std::fs::remove_file(source).map_err(|e| e.to_string())
}

// Only treat NotFound on an accessible directory as deletion. Offline drives and
// permission failures retain their recovery records.
fn definitely_missing(path: &Path) -> bool {
    matches!(std::fs::metadata(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
        && path.parent().is_some_and(Path::is_dir)
}

pub async fn reconcile(pool: &SqlitePool, thumb_dir: &Path) -> Result<(), String> {
    let thumb_dir = std::fs::canonicalize(thumb_dir).unwrap_or_else(|_| thumb_dir.to_path_buf());
    let entries: Vec<(String, String, Option<String>)> =
        sqlx::query_as("SELECT photo_id, original_path, trash_path FROM bin_entries")
            .fetch_all(pool)
            .await
            .map_err(|e| e.to_string())?;
    for (id, original, trash) in entries {
        let Some(trash) = trash else { continue }; // Preserve interrupted move journals.
        if !definitely_missing(Path::new(&trash)) {
            continue;
        }
        if Path::new(&original).is_file() {
            // Finder recovery is reconciled only if the original bytes still match.
            let _ = recover_one(pool, &id).await;
        } else if definitely_missing(Path::new(&original)) {
            let thumbnail: Option<String> =
                sqlx::query_scalar("SELECT thumbnail_path FROM photos WHERE id = ?")
                    .bind(&id)
                    .fetch_one(pool)
                    .await
                    .map_err(|e| e.to_string())?;
            let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
            for query in [
                "DELETE FROM recommendations WHERE photo_id = ?",
                "DELETE FROM user_decisions WHERE photo_id = ?",
                "DELETE FROM analysis WHERE photo_id = ?",
                "DELETE FROM photo_ai_results WHERE photo_id = ?",
                "DELETE FROM protected_photos WHERE photo_id = ?",
                "DELETE FROM bin_entries WHERE photo_id = ?",
                "DELETE FROM photos WHERE id = ?",
            ] {
                sqlx::query(query)
                    .bind(&id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            tx.commit().await.map_err(|e| e.to_string())?;
            if let Some(thumbnail) = thumbnail {
                let path = Path::new(&thumbnail);
                let references: i64 =
                    sqlx::query_scalar("SELECT COUNT(*) FROM photos WHERE thumbnail_path = ?")
                        .bind(&thumbnail)
                        .fetch_one(pool)
                        .await
                        .map_err(|e| e.to_string())?;
                if references == 0 && path.parent() == Some(thumb_dir.as_path()) && path.is_file() {
                    std::fs::remove_file(path).map_err(|e| e.to_string())?;
                }
            }
        }
    }
    Ok(())
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<BinPhoto>, String> {
    let photos: Vec<Photo> = sqlx::QueryBuilder::<sqlx::Sqlite>::new(db::PHOTO_SELECT)
        .push(" WHERE EXISTS (SELECT 1 FROM bin_entries b WHERE b.photo_id = p.id) ORDER BY p.id")
        .build_query_as()
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;
    let entries: Vec<(String, String, Option<String>, String)> = sqlx::query_as("SELECT photo_id, original_path, trash_path, deleted_at FROM bin_entries ORDER BY deleted_at DESC, photo_id")
        .fetch_all(pool).await.map_err(|e| e.to_string())?;
    let mut photos: std::collections::HashMap<_, _> =
        photos.into_iter().map(|p| (p.id.clone(), p)).collect();
    Ok(entries
        .into_iter()
        .filter_map(|(id, original_path, trash, deleted_at)| {
            let mut photo = photos.remove(&id)?;
            photo.absolute_path = trash.unwrap_or_else(|| original_path.clone());
            let available = Path::new(&photo.absolute_path).is_file();
            Some(BinPhoto {
                photo,
                original_path,
                deleted_at,
                available,
            })
        })
        .collect())
}

pub async fn choose_keeper(
    pool: &SqlitePool,
    id: &str,
    group_ids: &[String],
) -> Result<(), String> {
    if !group_ids.iter().any(|candidate| candidate == id) {
        return Err("The keeper must belong to this group.".into());
    }
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM photos WHERE id = ? AND NOT EXISTS (SELECT 1 FROM bin_entries b WHERE b.photo_id = photos.id)")
        .bind(id).fetch_one(&mut *tx).await.map_err(|e| e.to_string())?;
    if exists != 1 {
        return Err("This photo is no longer in the library. Refresh and choose again.".into());
    }
    for other in group_ids {
        sqlx::query("DELETE FROM protected_photos WHERE photo_id = ?")
            .bind(other)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
    }
    sqlx::query("INSERT OR IGNORE INTO protected_photos (photo_id) VALUES (?)")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())
}

pub async fn move_one<F>(pool: &SqlitePool, id: &str, mover: F) -> Result<i64, String>
where
    F: FnOnce(&Path) -> Result<PathBuf, String> + Send + 'static,
{
    let photo: Option<Photo> = sqlx::QueryBuilder::<sqlx::Sqlite>::new(db::PHOTO_SELECT)
        .push(
            " WHERE p.id = ? AND NOT EXISTS (SELECT 1 FROM bin_entries b WHERE b.photo_id = p.id)",
        )
        .build_query_as()
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;
    let photo = photo
        .ok_or_else(|| "This photo is already in the bin or is no longer indexed.".to_string())?;
    if photo.is_kept {
        return Err(format!(
            "{} is marked Keep. Choose another keeper before moving it to the bin.",
            photo.filename
        ));
    }
    // Commit the original location before touching the filesystem. Keep metadata for recovery.
    sqlx::query("INSERT INTO bin_entries (photo_id, original_path) VALUES (?, ?)")
        .bind(id)
        .bind(&photo.absolute_path)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    let original = PathBuf::from(&photo.absolute_path);
    let path = original.clone();
    let moved = tokio::task::spawn_blocking(move || mover(&path))
        .await
        .map_err(|e| e.to_string())?;
    let trash_path = match moved {
        Ok(path) => path.to_string_lossy().to_string(),
        Err(error) => {
            if original.is_file() {
                sqlx::query("DELETE FROM bin_entries WHERE photo_id = ?")
                    .bind(id)
                    .execute(pool)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            return Err(error);
        }
    };
    // Persist the returned location first, so recovery survives interruption while updating photos.
    sqlx::query("UPDATE bin_entries SET trash_path = ?, state = 'active' WHERE photo_id = ?")
        .bind(&trash_path).bind(id).execute(pool).await.map_err(|e| format!("Photo is in Trash at {trash_path}, but its recovery record could not be updated: {e}"))?;
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    sqlx::query("DELETE FROM recommendations WHERE photo_id = ? OR moment_id IN (SELECT moment_id FROM photos WHERE id = ?)")
        .bind(id).bind(id).execute(&mut *tx).await.map_err(|e| e.to_string())?;
    sqlx::query("UPDATE photos SET absolute_path = ?, moment_id = NULL WHERE id = ?")
        .bind(&trash_path)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(photo.file_size)
}

async fn entry(
    pool: &SqlitePool,
    id: &str,
) -> Result<(String, Option<String>, Option<String>), String> {
    sqlx::query_as("SELECT b.original_path, b.trash_path, p.sha256 FROM bin_entries b JOIN photos p ON p.id = b.photo_id WHERE b.photo_id = ?")
        .bind(id).fetch_optional(pool).await.map_err(|e| e.to_string())?
        .ok_or_else(|| "This photo is not in PhotoMind’s bin.".to_string())
}

fn verify_file(path: &Path, hash: Option<&str>) -> Result<(), String> {
    if !path.is_file() {
        return Err("The file is no longer in the bin. It may have been recovered or Trash may have been emptied in Finder.".into());
    }
    if let Some(hash) = hash {
        if crate::scanner::calculate_sha256(path).map_err(|e| e.to_string())? != hash {
            return Err(
                "The file in the bin has changed. It was preserved; inspect it in Finder.".into(),
            );
        }
    }
    Ok(())
}

pub async fn recover_one(pool: &SqlitePool, id: &str) -> Result<(), String> {
    let (original, trash, hash) = entry(pool, id).await?;
    let destination = PathBuf::from(&original);
    let source = trash.map(PathBuf::from);
    let hash_check = hash.clone();
    let recovered = tokio::task::spawn_blocking(move || {
        // Also reconcile a recovery completed in Finder or interrupted after moving the file.
        if source.as_ref().map(|p| !p.exists()).unwrap_or(true) {
            if hash_check.is_none() { return Err("Cannot verify the recovered original. Use Finder to inspect it.".into()); }
            return verify_file(&destination, hash_check.as_deref());
        }
        if destination.exists() { return Err("A file already exists at the original location. Move or rename that file in Finder, then recover again. Nothing was overwritten.".into()); }
        if !destination.parent().map(|p| p.is_dir()).unwrap_or(false) { return Err("The original folder is unavailable. Reconnect the drive or restore the folder, then recover again.".into()); }
        let source = source.unwrap();
        verify_file(&source, hash_check.as_deref())?;
        move_no_replace(&source, &destination)
    }).await.map_err(|e| e.to_string())?;
    recovered?;
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    sqlx::query("UPDATE photos SET absolute_path = ?, moment_id = NULL WHERE id = ?")
        .bind(&original)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    sqlx::query("DELETE FROM bin_entries WHERE photo_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())
}

pub async fn purge_one(pool: &SqlitePool, id: &str) -> Result<(), String> {
    let (_, trash, hash) = entry(pool, id).await?;
    let path = PathBuf::from(
        trash.ok_or("This move was interrupted. Recover or inspect it in Finder first.")?,
    );
    tokio::task::spawn_blocking(move || {
        verify_file(&path, hash.as_deref())?;
        std::fs::remove_file(&path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    for query in [
        "DELETE FROM recommendations WHERE photo_id = ?",
        "DELETE FROM user_decisions WHERE photo_id = ?",
        "DELETE FROM analysis WHERE photo_id = ?",
        "DELETE FROM photo_ai_results WHERE photo_id = ?",
        "DELETE FROM protected_photos WHERE photo_id = ?",
        "DELETE FROM bin_entries WHERE photo_id = ?",
    ] {
        sqlx::query(query)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
    }
    sqlx::query("DELETE FROM photos WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())
}

pub fn large_preview(path: &str) -> Result<Vec<u8>, String> {
    let image = crate::ingest::load_normalized(Path::new(path), 2400)?;
    let image = image.thumbnail(2400, 2400).into_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
        .encode_image(&image)
        .map_err(|e| e.to_string())?;
    Ok(jpeg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    async fn fixture() -> (PathBuf, SqlitePool, PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("photomind-bin-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("original.png");
        image::RgbImage::from_fn(100, 80, |x, y| image::Rgb([x as u8, y as u8, 90]))
            .save(&original)
            .unwrap();
        let pool = db::init_db(&dir).await.unwrap();
        crate::scanner::refresh_photo(&pool, &original, &dir.join("thumbs"))
            .await
            .unwrap();
        let id = db::all_photos(&pool).await.unwrap()[0].id.clone();
        sqlx::query("INSERT INTO photo_ai_results (photo_id, model, analysis_json) VALUES (?, 'fixture', '{}')").bind(&id).execute(&pool).await.unwrap();
        (dir, pool, original, id)
    }
    fn test_mover(target: PathBuf) -> impl FnOnce(&Path) -> Result<PathBuf, String> + Send {
        move |source| {
            std::fs::rename(source, &target).map_err(|e| e.to_string())?;
            Ok(target)
        }
    }
    #[tokio::test]
    async fn finder_trash_changes_remove_stale_records_and_previews() {
        let (dir, pool, original, id) = fixture().await;
        let preview = db::all_photos(&pool).await.unwrap()[0]
            .thumbnail_path
            .clone()
            .unwrap();
        let target = dir.join("trash-photo.png");
        move_one(&pool, &id, test_mover(target.clone()))
            .await
            .unwrap();
        // An unavailable Trash directory must not be mistaken for empty Trash.
        let offline = dir.join("offline");
        std::fs::create_dir(&offline).unwrap();
        let moved_target = offline.join("photo.png");
        std::fs::rename(&target, &moved_target).unwrap();
        sqlx::query("UPDATE bin_entries SET trash_path = ? WHERE photo_id = ?")
            .bind(
                dir.join("unavailable-drive/photo.png")
                    .to_string_lossy()
                    .as_ref(),
            )
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();
        reconcile(&pool, &dir.join("thumbs")).await.unwrap();
        assert_eq!(list(&pool).await.unwrap().len(), 1);
        sqlx::query("UPDATE bin_entries SET trash_path = ? WHERE photo_id = ?")
            .bind(moved_target.to_string_lossy().as_ref())
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();
        // Finder's Put Back preserves bytes and re-enters the library.
        std::fs::rename(&moved_target, &original).unwrap();
        reconcile(&pool, &dir.join("thumbs")).await.unwrap();
        assert!(list(&pool).await.unwrap().is_empty());
        assert_eq!(db::all_photos(&pool).await.unwrap()[0].id, id);
        assert!(Path::new(&preview).is_file());
        move_one(&pool, &id, test_mover(target.clone()))
            .await
            .unwrap();
        // Emptying Finder Trash removes both the journal and unused preview.
        std::fs::remove_file(&target).unwrap();
        reconcile(&pool, &dir.join("thumbs")).await.unwrap();
        assert!(list(&pool).await.unwrap().is_empty());
        assert!(!Path::new(&preview).exists());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM photos")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        reconcile(&pool, &dir.join("thumbs")).await.unwrap();
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "manual integration test: moves one generated image to macOS Trash and immediately recovers it"]
    async fn native_trash_roundtrip() {
        let (dir, pool, original, id) = fixture().await;
        let bytes = std::fs::read(&original).unwrap();
        move_one(&pool, &id, |path| system_trash(path, Path::new("unused")))
            .await
            .unwrap();
        let entries = list(&pool).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].available);
        assert!(!original.exists());
        assert_eq!(
            std::fs::read(&entries[0].photo.absolute_path).unwrap(),
            bytes
        );
        recover_one(&pool, &id).await.unwrap();
        assert_eq!(std::fs::read(&original).unwrap(), bytes);
        assert!(!Path::new(&entries[0].photo.absolute_path).exists());
        assert!(list(&pool).await.unwrap().is_empty());
        println!("macOS Trash returned a tracked location; previewed and recovered the generated image intact.");
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn bin_survives_restart_previews_recovers_and_never_overwrites_a_collision() {
        let (dir, pool, original, id) = fixture().await;
        let original_bytes = std::fs::read(&original).unwrap();
        let target = dir.join("trash-original.png");
        move_one(&pool, &id, test_mover(target.clone()))
            .await
            .unwrap();
        assert!(!original.exists());
        assert!(db::all_photos(&pool).await.unwrap().is_empty());
        assert!(db::photo_page(&pool, 0, 200).await.unwrap().is_empty());
        crate::scanner::generate_moments(&pool).await.unwrap();
        pool.close().await;
        let pool = db::init_db(&dir).await.unwrap();
        let entries = list(&pool).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].original_path, original.to_string_lossy());
        assert!(entries[0].available);
        assert!(
            image::load_from_memory(&large_preview(&entries[0].photo.absolute_path).unwrap())
                .is_ok()
        );
        std::fs::write(&original, b"a different file at the original name").unwrap();
        assert!(recover_one(&pool, &id)
            .await
            .unwrap_err()
            .contains("already exists"));
        assert_eq!(
            std::fs::read(&original).unwrap(),
            b"a different file at the original name"
        );
        assert_eq!(std::fs::read(&target).unwrap(), original_bytes);
        std::fs::remove_file(&original).unwrap();
        recover_one(&pool, &id).await.unwrap();
        assert_eq!(std::fs::read(&original).unwrap(), original_bytes);
        assert!(!target.exists());
        assert!(list(&pool).await.unwrap().is_empty());
        assert_eq!(db::all_photos(&pool).await.unwrap()[0].id, id);
        move_one(&pool, &id, test_mover(target.clone()))
            .await
            .unwrap();
        purge_one(&pool, &id).await.unwrap();
        assert!(!target.exists());
        assert!(list(&pool).await.unwrap().is_empty());
        assert!(db::all_photos(&pool).await.unwrap().is_empty());
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn keep_blocks_bin_and_failed_moves_preserve_the_library() {
        let (dir, pool, original, id) = fixture().await;
        choose_keeper(&pool, &id, &[id.clone()]).await.unwrap();
        let called = Arc::new(AtomicBool::new(false));
        let mover_called = called.clone();
        assert!(move_one(&pool, &id, move |_| {
            mover_called.store(true, Ordering::SeqCst);
            Err("should not run".into())
        })
        .await
        .is_err());
        assert!(!called.load(Ordering::SeqCst));
        assert!(original.exists());
        assert!(list(&pool).await.unwrap().is_empty());
        sqlx::query("DELETE FROM protected_photos")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            move_one(&pool, &id, |_| Err("simulated move failure".into()))
                .await
                .is_err()
        );
        assert!(original.exists());
        assert_eq!(db::all_photos(&pool).await.unwrap().len(), 1);
        assert!(list(&pool).await.unwrap().is_empty());
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn bin_rejects_changed_files_and_reconciles_an_interrupted_recovery() {
        let (dir, pool, original, id) = fixture().await;
        let target = dir.join("trash-original.png");
        move_one(&pool, &id, test_mover(target.clone()))
            .await
            .unwrap();
        let bytes = std::fs::read(&target).unwrap();
        std::fs::write(&target, b"replacement file").unwrap();
        assert!(purge_one(&pool, &id).await.is_err());
        assert!(recover_one(&pool, &id).await.is_err());
        assert!(target.exists());
        std::fs::write(&target, bytes).unwrap();
        // Simulate moving the file back before the recovery DB transaction could commit.
        std::fs::rename(&target, &original).unwrap();
        recover_one(&pool, &id).await.unwrap();
        assert_eq!(db::all_photos(&pool).await.unwrap()[0].id, id);
        assert!(list(&pool).await.unwrap().is_empty());
        assert!(purge_one(&pool, &id).await.is_err()); // Only tracked Bin entries can be purged.
        assert!(original.exists());
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }
}
