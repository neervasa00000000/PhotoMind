//! Offline same-person suggestions using the bundled SFace descriptor model.
use image::imageops::FilterType;
use serde::Serialize;
use sqlx::{FromRow, SqlitePool};
use std::collections::HashSet;

const SIGNATURE_VERSION: &str = "sface-int8-v1";
// OpenCV's 1:1 cosine threshold is 0.363. Use a margin for automatic groups.
// A pairwise verification threshold is too permissive for a gallery: one
// mistaken edge can join two entire identities. Require a strong link and
// agreement between every pair of faces when extending a group.
const GROUP_LINK_THRESHOLD: f32 = 0.60;
const MIN_CROSS_GROUP_SCORE: f32 = 0.45;
const MEAN_CROSS_GROUP_SCORE: f32 = 0.50;
const MIN_GROUP_FACE_PIXELS: f64 = 40.0;

#[derive(Clone)]
struct Candidate {
    photo_id: String,
    face_index: i64,
    signature: Vec<f32>,
}

#[derive(Debug, FromRow)]
struct FaceSource {
    photo_id: String,
    face_index: i64,
    signature: String,
}

#[derive(Debug, Serialize)]
pub struct PersonGroupIds {
    pub key: String,
    pub faces: Vec<PersonFaceId>,
}

#[derive(Debug, Serialize)]
pub struct PersonFaceId {
    pub photo_id: String,
    pub face_index: i64,
}

pub async fn cache_signatures(
    pool: &SqlitePool,
    photo_id: &str,
    rgb: &image::RgbImage,
    faces: &[crate::face_analysis::types::DetectedFace],
) -> Result<(), String> {
    let image = rgb.clone();
    let faces = faces.to_vec();
    let signatures = tokio::task::spawn_blocking(move || {
        faces
            .iter()
            .enumerate()
            .map(|(index, face)| {
                crate::face_analysis::recognition::descriptor(&image, face)
                    .map(|feature| feature.map(|feature| (index as i64, feature)))
            })
            .collect::<Result<Vec<_>, _>>()
    })
    .await
    .map_err(|e| e.to_string())?;
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    for (face_index, feature) in signatures?.into_iter().flatten() {
        sqlx::query(
            "INSERT INTO face_signatures (photo_id, face_index, version, signature)
             VALUES (?, ?, ?, ?) ON CONFLICT(photo_id, face_index)
             DO UPDATE SET version = excluded.version, signature = excluded.signature",
        )
        .bind(photo_id)
        .bind(face_index)
        .bind(SIGNATURE_VERSION)
        .bind(serde_json::to_string(&feature).map_err(|e| e.to_string())?)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    }
    tx.commit().await.map_err(|e| e.to_string())
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != 128 || b.len() != 128 {
        return -1.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn group_candidates(candidates: &[Candidate]) -> Vec<PersonGroupIds> {
    let mut edges = Vec::new();
    for i in 0..candidates.len() {
        for j in i + 1..candidates.len() {
            if candidates[i].photo_id == candidates[j].photo_id {
                continue;
            }
            let score = cosine(&candidates[i].signature, &candidates[j].signature);
            if score >= GROUP_LINK_THRESHOLD {
                edges.push((score, i, j));
            }
        }
    }
    edges.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut parent: Vec<usize> = (0..candidates.len()).collect();
    let mut members: Vec<Vec<usize>> = (0..candidates.len()).map(|i| vec![i]).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for (_, i, j) in edges {
        let (a, b) = (find(&mut parent, i), find(&mut parent, j));
        if a == b {
            continue;
        }
        let photos: HashSet<&str> = members[a]
            .iter()
            .map(|&k| candidates[k].photo_id.as_str())
            .collect();
        if members[b]
            .iter()
            .any(|&k| photos.contains(candidates[k].photo_id.as_str()))
        {
            continue;
        }
        let mut minimum = 1.0_f32;
        let mut total = 0.0_f32;
        let mut pairs = 0;
        for &left in &members[a] {
            for &right in &members[b] {
                let score = cosine(&candidates[left].signature, &candidates[right].signature);
                minimum = minimum.min(score);
                total += score;
                pairs += 1;
            }
        }
        if minimum < MIN_CROSS_GROUP_SCORE || total / (pairs as f32) < MEAN_CROSS_GROUP_SCORE {
            continue;
        }
        parent[b] = a;
        let moved = std::mem::take(&mut members[b]);
        members[a].extend(moved);
    }
    let mut groups: Vec<_> = members
        .into_iter()
        .filter(|members| members.len() >= 2)
        .map(|members| {
            let mut faces: Vec<_> = members
                .into_iter()
                .map(|i| PersonFaceId {
                    photo_id: candidates[i].photo_id.clone(),
                    face_index: candidates[i].face_index,
                })
                .collect();
            faces.sort_by(|a, b| {
                a.photo_id
                    .cmp(&b.photo_id)
                    .then(a.face_index.cmp(&b.face_index))
            });
            PersonGroupIds {
                key: format!("{}:{}", faces[0].photo_id, faces[0].face_index),
                faces,
            }
        })
        .collect();
    groups.sort_by(|a, b| b.faces.len().cmp(&a.faces.len()).then(a.key.cmp(&b.key)));
    groups
}

pub async fn likely_same_people(pool: &SqlitePool) -> Result<Vec<PersonGroupIds>, String> {
    if !crate::face_analysis::recognition::model_path().is_file() {
        return Err("The bundled face recognition model is missing. Reinstall PhotoMind.".into());
    }
    let rows: Vec<FaceSource> = sqlx::query_as(
        "SELECT f.photo_id, f.face_index, s.signature
         FROM photo_faces f JOIN photos p ON p.id = f.photo_id
         JOIN face_signatures s ON s.photo_id = f.photo_id AND s.face_index = f.face_index
         WHERE s.version = ? AND MIN(f.bbox_width, f.bbox_height) >= ?
           AND NOT EXISTS (SELECT 1 FROM bin_entries b WHERE b.photo_id = p.id)
         ORDER BY p.absolute_path, f.face_index",
    )
    .bind(SIGNATURE_VERSION)
    .bind(MIN_GROUP_FACE_PIXELS)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    let candidates = rows
        .into_iter()
        .filter_map(|row| {
            let signature: Vec<f32> = serde_json::from_str(&row.signature).ok()?;
            if signature.len() != 128 || signature.iter().any(|v| !v.is_finite()) {
                return None;
            }
            Some(Candidate {
                photo_id: row.photo_id,
                face_index: row.face_index,
                signature,
            })
        })
        .collect::<Vec<_>>();
    tokio::task::spawn_blocking(move || group_candidates(&candidates))
        .await
        .map_err(|e| e.to_string())
}

pub async fn face_preview(
    pool: &SqlitePool,
    photo_id: &str,
    face_index: i64,
) -> Result<Vec<u8>, String> {
    let row: Option<(String, f64, f64, f64, f64)> = sqlx::query_as(
        "SELECT p.absolute_path, f.bbox_x, f.bbox_y, f.bbox_width, f.bbox_height
         FROM photo_faces f JOIN photos p ON p.id = f.photo_id
         WHERE f.photo_id = ? AND f.face_index = ? AND NOT EXISTS
           (SELECT 1 FROM bin_entries b WHERE b.photo_id = p.id)",
    )
    .bind(photo_id)
    .bind(face_index)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;
    let (path, x, y, width, height) = row.ok_or_else(|| "Face not found".to_string())?;
    tokio::task::spawn_blocking(move || {
        let image = crate::ingest::load_normalized(std::path::Path::new(&path), 1536)?;
        let (image_width, image_height) = (image.width(), image.height());
        let left = (x - width * 0.12).max(0.0) as u32;
        let top = (y - height * 0.12).max(0.0) as u32;
        let right = (x + width * 1.12).max(0.0) as u32;
        let bottom = (y + height * 1.12).max(0.0) as u32;
        let left = left.min(image_width);
        let top = top.min(image_height);
        let right = right.min(image_width);
        let bottom = bottom.min(image_height);
        if right <= left || bottom <= top {
            return Err("Invalid face crop".into());
        }
        let crop = image
            .crop_imm(left, top, right - left, bottom - top)
            .resize(160, 160, FilterType::Triangle);
        crate::ingest::encode_jpeg(&crop)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    fn feature(a: f32, b: f32) -> Vec<f32> {
        let norm = (a * a + b * b).sqrt();
        let mut values = vec![0.0; 128];
        values[0] = a / norm;
        values[1] = b / norm;
        values
    }
    #[test]
    fn one_identity_merges_once_without_mixing_two_people_in_one_photo() {
        let rows = vec![
            Candidate {
                photo_id: "a".into(),
                face_index: 0,
                signature: feature(1.0, 0.0),
            },
            Candidate {
                photo_id: "b".into(),
                face_index: 0,
                signature: feature(0.9, 0.1),
            },
            Candidate {
                photo_id: "c".into(),
                face_index: 0,
                signature: feature(0.8, 0.2),
            },
            Candidate {
                photo_id: "b".into(),
                face_index: 1,
                signature: feature(-1.0, 0.0),
            },
            Candidate {
                photo_id: "d".into(),
                face_index: 0,
                signature: feature(-0.9, 0.1),
            },
        ];
        let groups = group_candidates(&rows);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].faces.len(), 3);
        assert_eq!(groups[1].faces.len(), 2);
        assert!(groups.iter().all(|g| g
            .faces
            .iter()
            .map(|f| &f.photo_id)
            .collect::<HashSet<_>>()
            .len()
            == g.faces.len()));
    }
    #[test]
    fn a_bridge_match_cannot_merge_different_people() {
        let rows = vec![
            Candidate {
                photo_id: "a".into(),
                face_index: 0,
                signature: feature(1.0, 0.0),
            },
            Candidate {
                photo_id: "b".into(),
                face_index: 0,
                signature: feature(0.8, 0.6),
            },
            Candidate {
                photo_id: "c".into(),
                face_index: 0,
                signature: feature(0.28, 0.96),
            },
        ];
        let groups = group_candidates(&rows);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].faces.len(), 2);
    }
    #[tokio::test]
    async fn binned_faces_disappear_from_groups() {
        let dir = std::env::temp_dir().join(format!("photomind-people-{}", uuid::Uuid::new_v4()));
        let pool = crate::db::init_db(&dir).await.unwrap();
        let image = image::RgbImage::from_fn(128, 128, |x, y| image::Rgb([x as u8, y as u8, 0]));
        for id in ["a", "b"] {
            let path = dir.join(format!("{id}.png"));
            image.save(&path).unwrap();
            sqlx::query("INSERT INTO photos (id, absolute_path, filename, extension, file_size, analysis_status)
                         VALUES (?, ?, ?, 'png', 1, 'complete')")
                .bind(id).bind(path.to_string_lossy().as_ref()).bind(format!("{id}.png"))
                .execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO photo_faces
                         (photo_id, face_index, bbox_x, bbox_y, bbox_width, bbox_height, detection_confidence)
                         VALUES (?, 0, 20, 20, 80, 80, 0.99)")
                .bind(id).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO face_signatures (photo_id, face_index, version, signature) VALUES (?, 0, ?, ?)")
                .bind(id).bind(SIGNATURE_VERSION).bind(serde_json::to_string(&feature(1.0, 0.0)).unwrap())
                .execute(&pool).await.unwrap();
        }
        assert_eq!(likely_same_people(&pool).await.unwrap().len(), 1);
        assert!(face_preview(&pool, "a", 0)
            .await
            .unwrap()
            .starts_with(&[0xff, 0xd8]));
        sqlx::query("INSERT INTO bin_entries (photo_id, original_path) VALUES ('b', 'original')")
            .execute(&pool)
            .await
            .unwrap();
        assert!(likely_same_people(&pool).await.unwrap().is_empty());
        assert!(face_preview(&pool, "b", 0).await.is_err());
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }
}
