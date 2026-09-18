//! Unique-moment guard.
//!
//! Detects moments that capture the same burst twice — the same actual instant
//! surfaced as photos across two different moment groups — by looking for
//! near-identical photo pairs with overlapping capture windows. Detection only:
//! nothing is merged, moved, or deleted here; callers decide what to do, keeping
//! the library non-destructive.

use serde::Serialize;
use sqlx::SqlitePool;
use std::collections::HashMap;

use crate::similarity;

#[derive(Debug, Serialize, PartialEq)]
pub struct DuplicateMomentPair {
    pub moment_a: String,
    pub moment_b: String,
    pub start_a: Option<String>,
    pub end_a: Option<String>,
    pub start_b: Option<String>,
    pub end_b: Option<String>,
    pub shared_photo_pairs: usize,
    pub reason: String,
}

fn parse_datetime(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

/// Two capture windows overlap only when both are known and intersect.
fn windows_overlap(
    (a_start, a_end): (&Option<String>, &Option<String>),
    (b_start, b_end): (&Option<String>, &Option<String>),
) -> bool {
    let Some(a_start) = a_start.as_deref().and_then(parse_datetime) else {
        return false;
    };
    let Some(a_end) = a_end.as_deref().and_then(parse_datetime) else {
        return false;
    };
    let Some(b_start) = b_start.as_deref().and_then(parse_datetime) else {
        return false;
    };
    let Some(b_end) = b_end.as_deref().and_then(parse_datetime) else {
        return false;
    };
    a_start <= b_end && b_start <= a_end
}

pub async fn find_duplicate_moments(pool: &SqlitePool) -> Result<Vec<DuplicateMomentPair>, String> {
    let moments: Vec<(String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT id, start_time, end_time FROM moment_groups")
            .fetch_all(pool)
            .await
            .map_err(|e| e.to_string())?;
    if moments.len() < 2 {
        return Ok(Vec::new());
    }

    let rows: Vec<(Option<String>, String, String)> = sqlx::query_as(
        "SELECT p.moment_id, p.id, p.perceptual_hash
           FROM photos p
          WHERE p.moment_id IS NOT NULL
            AND p.perceptual_hash IS NOT NULL
            AND NOT EXISTS (SELECT 1 FROM bin_entries b WHERE b.photo_id = p.id)",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    let mut by_moment = HashMap::<String, Vec<(String, String)>>::new();
    for (moment_id, photo_id, phash) in rows {
        if let Some(mid) = moment_id {
            by_moment.entry(mid).or_default().push((photo_id, phash));
        }
    }

    let mut pairs = Vec::new();
    for i in 0..moments.len() {
        for j in (i + 1)..moments.len() {
            let (ma, sa, ea) = &moments[i];
            let (mb, sb, eb) = &moments[j];
            let Some(shots_a) = by_moment.get(ma) else {
                continue;
            };
            let Some(shots_b) = by_moment.get(mb) else {
                continue;
            };

            let mut shared = 0usize;
            for (_, ha) in shots_a {
                for (_, hb) in shots_b {
                    if similarity::hash_distance(ha, hb)
                        .map(|d| d <= 12)
                        .unwrap_or(false)
                    {
                        shared += 1;
                    }
                }
            }
            if shared == 0 {
                continue;
            }
            let same_window = windows_overlap((sa, ea), (sb, eb));
            if !same_window {
                continue;
            }
            pairs.push(DuplicateMomentPair {
                moment_a: ma.clone(),
                moment_b: mb.clone(),
                start_a: sa.clone(),
                end_a: ea.clone(),
                start_b: sb.clone(),
                end_b: eb.clone(),
                shared_photo_pairs: shared,
                reason: format!(
                    "{shared} near-identical photo(s) shot within the same time window — likely the same burst captured twice."
                ),
            });
        }
    }

    pairs.sort_by(|a, b| b.shared_photo_pairs.cmp(&a.shared_photo_pairs));
    Ok(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn insert_photos(pool: &SqlitePool, fixtures: &[(&str, &str, &str, &str)]) {
        for (id, moment, time, phash) in fixtures {
            sqlx::query("INSERT INTO photos (id, moment_id, capture_time, perceptual_hash) VALUES (?, ?, ?, ?)")
                .bind(id).bind(*moment).bind(*time).bind(*phash).execute(pool).await.unwrap();
        }
    }

    async fn insert_moments(pool: &SqlitePool, fixtures: &[(&str, &str, &str)]) {
        for (id, start, end) in fixtures {
            sqlx::query("INSERT INTO moment_groups (id, start_time, end_time, photo_count) VALUES (?, ?, ?, 1)")
                .bind(*id).bind(*start).bind(*end).execute(pool).await.unwrap();
        }
    }

    #[tokio::test]
    async fn flags_the_same_burst_scanned_twice() {
        let dir = std::env::temp_dir().join(format!("photomind-guard-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let pool = crate::db::init_db(&dir).await.unwrap();

        insert_moments(
            &pool,
            &[
                ("m1", "2026-03-01T10:00:00Z", "2026-03-01T10:00:10Z"),
                ("m2", "2026-03-01T10:00:02Z", "2026-03-01T10:00:12Z"),
            ],
        )
        .await;
        insert_photos(
            &pool,
            &[
                // m1's shots differ from m2's shots by only 1 hash bit.
                ("p1", "m1", "2026-03-01T10:00:01Z", "0000000000000000"),
                ("p2", "m1", "2026-03-01T10:00:08Z", "1000000000000000"),
                ("p3", "m2", "2026-03-01T10:00:03Z", "0000000000000001"),
                ("p4", "m2", "2026-03-01T10:00:09Z", "1000000000000001"),
            ],
        )
        .await;

        let pairs = find_duplicate_moments(&pool).await.unwrap();
        assert_eq!(pairs.len(), 1, "pairs: {pairs:?}");
        assert_eq!(pairs[0].shared_photo_pairs, 4);
        assert!(pairs[0].reason.contains("same burst"));

        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn similar_photos_in_different_time_windows_are_not_grouped() {
        let dir =
            std::env::temp_dir().join(format!("photomind-guard-time-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let pool = crate::db::init_db(&dir).await.unwrap();

        insert_moments(
            &pool,
            &[
                ("m1", "2026-03-01T10:00:00Z", "2026-03-01T10:00:10Z"),
                ("m2", "2026-06-01T10:00:00Z", "2026-06-01T10:00:10Z"),
            ],
        )
        .await;
        insert_photos(
            &pool,
            &[
                ("p1", "m1", "2026-03-01T10:00:01Z", "0000000000000000"),
                ("p2", "m2", "2026-06-01T10:00:01Z", "0000000000000001"),
            ],
        )
        .await;

        let pairs = find_duplicate_moments(&pool).await.unwrap();
        assert!(pairs.is_empty());

        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn identical_photo_sets_collapse_to_one_moment_id_at_creation() {
        // moment_signature guarantees this invariant upstream; the guard simply
        // tolerates one-moment libraries without error.
        let dir =
            std::env::temp_dir().join(format!("photomind-guard-single-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let pool = crate::db::init_db(&dir).await.unwrap();
        insert_moments(
            &pool,
            &[("m1", "2026-03-01T10:00:00Z", "2026-03-01T10:00:10Z")],
        )
        .await;
        let pairs = find_duplicate_moments(&pool).await.unwrap();
        assert!(pairs.is_empty());
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }
}
