//! Conservative, entirely local suggestions that two detected faces may belong
//! to the same person. These image descriptors are not identity embeddings.
use image::{imageops::FilterType, GrayImage, RgbImage};
use serde::Serialize;
use sqlx::{FromRow, SqlitePool};
use std::collections::{HashMap, HashSet};

const SIGNATURE_VERSION: &str = "face-crop-v1";
const SIDE: u32 = 20;

#[derive(Debug, FromRow)]
struct FaceSource {
    photo_id: String,
    face_index: i64,
    absolute_path: String,
    bbox_x: f64,
    bbox_y: f64,
    bbox_width: f64,
    bbox_height: f64,
    signature: Option<String>,
}

#[derive(Clone)]
struct Candidate {
    photo_id: String,
    face_index: i64,
    signature: Vec<f32>,
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

fn signature(gray: &GrayImage, row: &FaceSource) -> Option<Vec<f32>> {
    let (w, h) = gray.dimensions();
    let x0 = (row.bbox_x + row.bbox_width * 0.12).max(0.0) as u32;
    let y0 = (row.bbox_y + row.bbox_height * 0.08).max(0.0) as u32;
    let x1 = (row.bbox_x + row.bbox_width * 0.88).max(0.0) as u32;
    let y1 = (row.bbox_y + row.bbox_height * 0.92).max(0.0) as u32;
    let x0 = x0.min(w);
    let y0 = y0.min(h);
    let x1 = x1.min(w);
    let y1 = y1.min(h);
    if x1.saturating_sub(x0) < 24 || y1.saturating_sub(y0) < 24 {
        return None;
    }
    let crop = image::imageops::crop_imm(gray, x0, y0, x1 - x0, y1 - y0).to_image();
    let small = image::imageops::resize(&crop, SIDE, SIDE, FilterType::Triangle);
    let mut pixels: Vec<f32> = small.pixels().map(|p| p[0] as f32).collect();
    let mean = pixels.iter().sum::<f32>() / pixels.len() as f32;
    for pixel in &mut pixels {
        *pixel -= mean;
    }
    let norm = pixels.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm < 30.0 {
        return None;
    }
    for pixel in &mut pixels {
        *pixel /= norm;
    }
    Some(pixels)
}

pub async fn cache_signatures(
    pool: &SqlitePool,
    photo_id: &str,
    rgb: &RgbImage,
    faces: &[crate::face_analysis::types::DetectedFace],
) -> Result<(), String> {
    let gray = image::DynamicImage::ImageRgb8(rgb.clone()).to_luma8();
    for (index, face) in faces.iter().enumerate() {
        let row = FaceSource {
            photo_id: photo_id.to_owned(),
            face_index: index as i64,
            absolute_path: String::new(),
            bbox_x: face.bbox.x as f64,
            bbox_y: face.bbox.y as f64,
            bbox_width: face.bbox.width as f64,
            bbox_height: face.bbox.height as f64,
            signature: None,
        };
        if let Some(feature) = signature(&gray, &row) {
            let encoded = serde_json::to_string(&feature).map_err(|e| e.to_string())?;
            sqlx::query(
                "INSERT INTO face_signatures (photo_id, face_index, version, signature)
                         VALUES (?, ?, ?, ?) ON CONFLICT(photo_id, face_index)
                         DO UPDATE SET version = excluded.version, signature = excluded.signature",
            )
            .bind(photo_id)
            .bind(index as i64)
            .bind(SIGNATURE_VERSION)
            .bind(encoded)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.len() != (SIDE * SIDE) as usize {
        return -1.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn buckets(signature: &[f32]) -> [(u8, u16); 8] {
    let mut out = [(0, 0); 8];
    for bank in 0..8_u8 {
        let mut bits = 0_u16;
        for bit in 0..10_usize {
            let a = (bank as usize * 97 + bit * 53 + 11) % signature.len();
            let b = (bank as usize * 71 + bit * 89 + 193) % signature.len();
            if signature[a] > signature[b] {
                bits |= 1 << bit;
            }
        }
        out[bank as usize] = (bank, bits);
    }
    out
}

// Compare with every member so a chain of weak matches cannot merge two people.
fn group_candidates(candidates: &[Candidate]) -> Vec<PersonGroupIds> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut bucket_index: HashMap<(u8, u16), Vec<usize>> = HashMap::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let keys = buckets(&candidate.signature);
        let possible: HashSet<usize> = keys
            .iter()
            .flat_map(|key| bucket_index.get(key).into_iter().flatten().copied())
            .collect();
        let best = possible
            .into_iter()
            .filter_map(|group_index| {
                let members = &groups[group_index];
                if members
                    .iter()
                    .any(|&i| candidates[i].photo_id == candidate.photo_id)
                {
                    return None;
                }
                let minimum = members
                    .iter()
                    .map(|&i| similarity(&candidates[i].signature, &candidate.signature))
                    .fold(1.0_f32, f32::min);
                (minimum >= 0.965).then_some((group_index, minimum))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1));
        let group_index = if let Some((group_index, _)) = best {
            groups[group_index].push(index);
            group_index
        } else {
            groups.push(vec![index]);
            groups.len() - 1
        };
        for key in keys {
            bucket_index.entry(key).or_default().push(group_index);
        }
    }
    groups
        .into_iter()
        .filter(|members| members.len() >= 2)
        .map(|members| {
            let first = &candidates[members[0]];
            PersonGroupIds {
                key: format!("{}:{}", first.photo_id, first.face_index),
                faces: members
                    .iter()
                    .map(|&i| PersonFaceId {
                        photo_id: candidates[i].photo_id.clone(),
                        face_index: candidates[i].face_index,
                    })
                    .collect(),
            }
        })
        .collect()
}

pub async fn likely_same_people(pool: &SqlitePool) -> Result<Vec<PersonGroupIds>, String> {
    let rows: Vec<FaceSource> = sqlx::query_as(
        "SELECT f.photo_id, f.face_index, p.absolute_path, f.bbox_x, f.bbox_y,
                f.bbox_width, f.bbox_height, s.signature
         FROM photo_faces f JOIN photos p ON p.id = f.photo_id
         LEFT JOIN face_signatures s ON s.photo_id = f.photo_id
              AND s.face_index = f.face_index AND s.version = ?
         WHERE NOT EXISTS (SELECT 1 FROM bin_entries b WHERE b.photo_id = p.id)
         ORDER BY p.absolute_path, f.face_index",
    )
    .bind(SIGNATURE_VERSION)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    let mut candidates = Vec::new();
    let mut current_photo_id = String::new();
    let mut current_image: Option<GrayImage> = None;
    for row in rows {
        let cached = row
            .signature
            .as_deref()
            .and_then(|json| serde_json::from_str::<Vec<f32>>(json).ok())
            .filter(|values| {
                values.len() == (SIDE * SIDE) as usize && values.iter().all(|v| v.is_finite())
            });
        let feature = if let Some(feature) = cached {
            Some(feature)
        } else {
            if current_photo_id != row.photo_id {
                let path = row.absolute_path.clone();
                current_image = tokio::task::spawn_blocking(move || {
                    crate::ingest::load_normalized(std::path::Path::new(&path), 512)
                        .ok()
                        .map(|image| image.to_luma8())
                })
                .await
                .map_err(|e| e.to_string())?;
                current_photo_id = row.photo_id.clone();
            }
            let feature = current_image
                .as_ref()
                .and_then(|gray| signature(gray, &row));
            if let Some(ref feature) = feature {
                let encoded = serde_json::to_string(feature).map_err(|e| e.to_string())?;
                sqlx::query("INSERT INTO face_signatures (photo_id, face_index, version, signature)
                             VALUES (?, ?, ?, ?) ON CONFLICT(photo_id, face_index)
                             DO UPDATE SET version = excluded.version, signature = excluded.signature")
                    .bind(&row.photo_id).bind(row.face_index).bind(SIGNATURE_VERSION).bind(encoded)
                    .execute(pool).await.map_err(|e| e.to_string())?;
            }
            feature
        };
        if let Some(feature) = feature {
            candidates.push(Candidate {
                photo_id: row.photo_id,
                face_index: row.face_index,
                signature: feature,
            });
        }
    }
    // Exact copies are distinct files, but a person group should show each photo once.
    let mut groups = tokio::task::spawn_blocking(move || group_candidates(&candidates))
        .await
        .map_err(|e| e.to_string())?;
    for group in &mut groups {
        let mut seen = HashSet::new();
        group
            .faces
            .retain(|face| seen.insert(face.photo_id.clone()));
    }
    groups.sort_by(|a, b| b.faces.len().cmp(&a.faces.len()).then(a.key.cmp(&b.key)));
    Ok(groups)
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
        let image = crate::ingest::load_normalized(std::path::Path::new(&path), 512)?;
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
    #[test]
    fn only_strong_matches_group_and_one_face_per_photo() {
        let image = vec![0.05; 400];
        let opposite = vec![-0.05; 400];
        let rows = vec![
            Candidate {
                photo_id: "a".into(),
                face_index: 0,
                signature: image.clone(),
            },
            Candidate {
                photo_id: "b".into(),
                face_index: 0,
                signature: image.clone(),
            },
            Candidate {
                photo_id: "b".into(),
                face_index: 1,
                signature: image,
            },
            Candidate {
                photo_id: "c".into(),
                face_index: 0,
                signature: opposite,
            },
        ];
        let groups = group_candidates(&rows);
        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups[0]
                .faces
                .iter()
                .map(|face| face.photo_id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[tokio::test]
    async fn local_face_crops_are_cached_and_binned_photos_disappear() {
        let dir = std::env::temp_dir().join(format!("photomind-people-{}", uuid::Uuid::new_v4()));
        let pool = crate::db::init_db(&dir).await.unwrap();
        let image = image::RgbImage::from_fn(128, 128, |x, y| {
            image::Rgb([x as u8, y as u8, ((x + y) / 2) as u8])
        });
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
        }
        let groups = likely_same_people(&pool).await.unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].faces.len(), 2);
        assert!(face_preview(&pool, "a", 0)
            .await
            .unwrap()
            .starts_with(&[0xff, 0xd8]));
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM face_signatures")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 2);
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
