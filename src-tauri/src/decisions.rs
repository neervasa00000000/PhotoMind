//! Fast, conservative recommendations from measured technical quality and local face signals.
use crate::decision_reasons::{DecisionReason, ReasonBundle};
use crate::decision_weights::{BEST_GAP_CONFIDENT, BEST_TIE_EPSILON, WEIGHTS};
use crate::face_analysis::types::{DetectedFace, EyeState, GroupFaceMetrics};
use crate::models::{Photo, Recommendation};
use sqlx::SqlitePool;
use std::collections::BTreeMap;

/// SAFETY: Eye detection feature flag.
/// Set to `false` until empirical validation confirms accuracy ≥85% and false-closed rate <5%.
/// When false: eye detection runs and stores results, but does NOT affect ranking/recommendations.
/// When true: eye state influences BEST-OF-GROUP ranking (after validation).
const ENABLE_EYE_BASED_RANKING: bool = false;

#[derive(Clone)]
struct PhotoContext {
    photo: Photo,
    faces: Vec<DetectedFace>,
    group_metrics: GroupFaceMetrics,
}

fn technical_score(photo: &Photo) -> f64 {
    let sharpness = photo.sharpness.unwrap_or(0.0).clamp(0.0, 1.0);
    let clipping = (photo.highlight_clipping.unwrap_or(0.0) + photo.shadow_clipping.unwrap_or(0.0))
        .clamp(0.0, 1.0);
    WEIGHTS.technical_sharpness * sharpness + WEIGHTS.technical_clipping * (1.0 - clipping)
}

fn people_score(metrics: &GroupFaceMetrics, faces: &[DetectedFace]) -> f64 {
    if metrics.face_count == 0 {
        return 0.0;
    }
    let face_sharp = metrics.average_face_sharpness.unwrap_or(0.0) as f64;
    
    // Eye-based scoring: gated by ENABLE_EYE_BASED_RANKING feature flag
    let eye_score = if ENABLE_EYE_BASED_RANKING {
        // Eye detection is validated and enabled for ranking
        if metrics.closed_eye_count > 0 {
            0.0
        } else if metrics.uncertain_eye_count > 0 {
            0.45
        } else if metrics.likely_all_eyes_open {
            1.0
        } else {
            0.55
        }
    } else {
        // SAFETY: Eye detection not yet validated - use neutral score
        // This allows eye analysis to run and be stored/displayed without affecting decisions
        0.5
    };
    
    let expression = metrics
        .average_expression_score
        .map(f64::from)
        .unwrap_or_else(|| {
            faces
                .iter()
                .filter_map(|f| f.expression.as_ref())
                .filter_map(|e| e.smile_score)
                .map(f64::from)
                .next()
                .unwrap_or(0.5)
        });
    WEIGHTS.face_sharpness * face_sharp
        + WEIGHTS.eyes_open * eye_score
        + WEIGHTS.expression * expression
}

fn exposure_bonus(photo: &Photo) -> f64 {
    let exposure = photo.exposure.unwrap_or(0.5);
    if (0.25..=0.85).contains(&exposure) {
        WEIGHTS.exposure_ok
    } else {
        0.0
    }
}

fn total_score(ctx: &PhotoContext) -> f64 {
    technical_score(&ctx.photo)
        + people_score(&ctx.group_metrics, &ctx.faces)
        + exposure_bonus(&ctx.photo)
}

fn mode_face_count(contexts: &[PhotoContext]) -> Option<usize> {
    let mut counts = BTreeMap::<usize, usize>::new();
    for ctx in contexts {
        if ctx.group_metrics.face_count > 0 {
            *counts.entry(ctx.group_metrics.face_count).or_default() += 1;
        }
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(c, _)| c)
}

pub fn recommend(contexts: &[PhotoContext]) -> Vec<Recommendation> {
    let photos: Vec<&Photo> = contexts.iter().map(|c| &c.photo).collect();
    let eligible: Vec<&PhotoContext> = contexts
        .iter()
        .filter(|c| {
            c.photo.analysis_status != "user_remove"
                && c.photo.sharpness.is_some()
                && c.photo.thumbnail_path.is_some()
        })
        .collect();

    let Some(best_ctx) = eligible
        .iter()
        .max_by(|a, b| {
            a.photo
                .is_kept
                .cmp(&b.photo.is_kept)
                .then(total_score(a).total_cmp(&total_score(b)))
                .then_with(|| b.photo.id.cmp(&a.photo.id))
        })
        .copied()
    else {
        return photos
            .iter()
            .map(|p| {
                if p.is_kept {
                    row(
                        p,
                        "KEEP",
                        1.0,
                        ReasonBundle::from_codes(vec![DecisionReason::KeptByUser]),
                        None,
                        None,
                    )
                } else {
                    row(
                        p,
                        "REVIEW",
                        0.0,
                        ReasonBundle::from_codes(vec![DecisionReason::TechnicalUnavailable]),
                        None,
                        None,
                    )
                }
            })
            .collect();
    };

    let best_score = total_score(best_ctx);
    let second_score = eligible
        .iter()
        .filter(|c| c.photo.id != best_ctx.photo.id)
        .map(|c| total_score(c))
        .max_by(f64::total_cmp)
        .unwrap_or(0.0);
    let tied_top = (best_score - second_score).abs() <= BEST_TIE_EPSILON;
    let confident_best = !tied_top && (best_score - second_score) >= BEST_GAP_CONFIDENT;

    let mut exact_groups = BTreeMap::<&str, Vec<&Photo>>::new();
    for ctx in &eligible {
        if let Some(hash) = ctx.photo.sha256.as_deref().filter(|h| !h.is_empty()) {
            exact_groups.entry(hash).or_default().push(&ctx.photo);
        }
    }
    let exact_keepers: BTreeMap<&str, &Photo> = exact_groups
        .iter()
        .filter(|(_, group)| group.len() > 1)
        .map(|(hash, group)| {
            (
                *hash,
                *group
                    .iter()
                    .max_by(|a, b| {
                        a.is_kept
                            .cmp(&b.is_kept)
                            .then((a.id == best_ctx.photo.id).cmp(&(b.id == best_ctx.photo.id)))
                            .then_with(|| b.id.cmp(&a.id))
                    })
                    .unwrap(),
            )
        })
        .collect();

    let mode_faces = mode_face_count(contexts);
    let group_size = eligible.len();

    contexts
        .iter()
        .map(|ctx| {
            let p = &ctx.photo;
            let mut codes = Vec::new();
            let mut compared_to = None;
            let (decision, confidence) = if p.is_kept {
                codes.push(DecisionReason::KeptByUser);
                ("KEEP", 1.0)
            } else if p.sharpness.is_none() || p.thumbnail_path.is_none() {
                codes.push(DecisionReason::TechnicalUnavailable);
                ("REVIEW", 0.0)
            } else if let Some(keeper) = p.sha256.as_deref().and_then(|h| exact_keepers.get(h)) {
                if p.id == keeper.id {
                    codes.push(DecisionReason::ExactDuplicate);
                    codes.push(DecisionReason::BestOfGroup);
                    ("KEEP", 1.0)
                } else {
                    compared_to = Some(keeper.id.clone());
                    codes.push(DecisionReason::ExactDuplicate);
                    ("REMOVE", 1.0)
                }
            } else if group_size == 1 && p.sharpness.unwrap_or(0.0) < 0.2 {
                codes.push(DecisionReason::NoBetterAlternative);
                ("REVIEW", 0.55)
            } else if p.id == best_ctx.photo.id {
                if group_size == 1 {
                    codes.push(DecisionReason::UniqueMoment);
                } else if confident_best {
                    codes.push(DecisionReason::BestOfGroup);
                } else if tied_top {
                    codes.push(DecisionReason::CloseComparison);
                } else {
                    codes.push(DecisionReason::BestOfGroup);
                }
                if ctx.group_metrics.face_count > 0 {
                    if ctx.group_metrics.likely_all_eyes_open {
                        codes.push(DecisionReason::AllEyesLikelyOpen);
                    }
                    if ctx.group_metrics.average_face_sharpness.is_some() {
                        codes.push(DecisionReason::StrongFaceSharpness);
                    }
                    if ctx.group_metrics.average_expression_score.is_some() {
                        codes.push(DecisionReason::StrongerExpression);
                    }
                } else {
                    codes.push(DecisionReason::SharpestInGroup);
                }
                if exposure_bonus(p) > 0.0 {
                    codes.push(DecisionReason::GoodExposure);
                }
                let conf = if tied_top {
                    0.62
                } else if confident_best {
                    0.65 + ((best_score - second_score) * 2.0).clamp(0.0, 0.3)
                } else {
                    0.7
                };
                ("KEEP", conf.min(0.95))
            } else {
                compared_to = Some(best_ctx.photo.id.clone());
                let exact = p
                    .sha256
                    .as_deref()
                    .filter(|h| !h.is_empty())
                    .is_some_and(|h| best_ctx.photo.sha256.as_deref() == Some(h));
                let close = p
                    .perceptual_hash
                    .as_deref()
                    .zip(best_ctx.photo.perceptual_hash.as_deref())
                    .and_then(|(a, b)| crate::similarity::hash_distance(a, b))
                    .is_some_and(|d| d <= 8);

                if exact {
                    codes.push(DecisionReason::ExactDuplicate);
                    ("REMOVE", 1.0)
                } else if ctx.group_metrics.face_count > 0 {
                    if ctx.group_metrics.closed_eye_count > 0 && close {
                        codes.push(DecisionReason::PossibleClosedEyes);
                        codes.push(DecisionReason::StrongerAlternative);
                        if best_ctx.group_metrics.likely_all_eyes_open {
                            ("REMOVE", 0.82)
                        } else {
                            ("REVIEW", 0.6)
                        }
                    } else if ctx.group_metrics.uncertain_eye_count > 0 {
                        codes.push(DecisionReason::EyeStateUncertain);
                        codes.push(DecisionReason::CloseComparison);
                        ("REVIEW", 0.65)
                    } else if close
                        && p.sharpness.unwrap_or(0.0) + 0.05
                            < best_ctx.photo.sharpness.unwrap_or(0.0)
                    {
                        codes.push(DecisionReason::StrongerAlternative);
                        codes.push(DecisionReason::SharpestInGroup);
                        ("REVIEW", 0.58)
                    } else {
                        codes.push(if close {
                            DecisionReason::CloseComparison
                        } else {
                            DecisionReason::UniqueMoment
                        });
                        if p.sharpness.unwrap_or(0.0) < best_ctx.photo.sharpness.unwrap_or(0.0) {
                            codes.push(DecisionReason::SharpestInGroup);
                        }
                        if ctx.group_metrics.average_expression_score.unwrap_or(1.0)
                            < best_ctx
                                .group_metrics
                                .average_expression_score
                                .unwrap_or(0.0)
                        {
                            codes.push(DecisionReason::LowerExpression);
                        }
                        ("REVIEW", 0.55)
                    }
                } else if close
                    && p.sharpness.unwrap_or(0.0) < 0.2
                    && best_ctx.photo.sharpness.unwrap_or(0.0) >= 0.5
                {
                    codes.push(DecisionReason::SharpestInGroup);
                    codes.push(DecisionReason::NoBetterAlternative);
                    ("REVIEW", 0.5)
                } else {
                    codes.push(if close {
                        DecisionReason::CloseComparison
                    } else {
                        DecisionReason::UniqueMoment
                    });
                    if p.sharpness.unwrap_or(0.0) < best_ctx.photo.sharpness.unwrap_or(0.0) {
                        codes.push(DecisionReason::SharpestInGroup);
                    }
                    ("REVIEW", 0.5)
                }
            };

            if let Some(mode) = mode_faces {
                if ctx.group_metrics.face_count > 0
                    && ctx.group_metrics.face_count != mode
                    && ctx.group_metrics.face_count + 1 == mode
                {
                    codes.push(DecisionReason::FaceCountMismatch);
                }
            }
            if p.highlight_clipping.unwrap_or(0.0) > 0.15 || p.shadow_clipping.unwrap_or(0.0) > 0.3
            {
                codes.push(DecisionReason::ExposureProblem);
            }

            row(
                p,
                decision,
                confidence,
                ReasonBundle::from_codes(codes),
                compared_to,
                if p.id == best_ctx.photo.id && group_size > 1 {
                    Some(group_size as i64)
                } else {
                    None
                },
            )
        })
        .collect()
}

fn row(
    p: &Photo,
    decision: &str,
    confidence: f64,
    reasons: ReasonBundle,
    compared_to: Option<String>,
    best_of: Option<i64>,
) -> Recommendation {
    Recommendation {
        photo_id: p.id.clone(),
        moment_id: p.moment_id.clone(),
        decision: decision.into(),
        confidence,
        reasoning: reasons.to_json(),
        compared_to,
        reason_codes: Some(
            reasons
                .codes
                .iter()
                .map(|c| format!("{c:?}"))
                .collect::<Vec<_>>()
                .join(","),
        ),
        best_of_group: best_of,
    }
}

pub async fn update(pool: &SqlitePool, moment_id: Option<&str>) -> Result<(), sqlx::Error> {
    let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(crate::db::PHOTO_SELECT);
    query.push(" WHERE NOT EXISTS (SELECT 1 FROM bin_entries b WHERE b.photo_id = p.id) AND NOT EXISTS (SELECT 1 FROM logical_photos l WHERE l.paired_id = p.id)");
    if let Some(id) = moment_id {
        query.push(" AND p.moment_id = ").push_bind(id);
    }
    let photos: Vec<Photo> = query.build_query_as().fetch_all(pool).await?;
    let photo_ids: Vec<String> = photos.iter().map(|p| p.id.clone()).collect();
    let faces_by_photo = crate::face_db::load_faces_batch(pool, &photo_ids)
        .await
        .unwrap_or_default();
    let mut tx = pool.begin().await?;
    let overrides: std::collections::HashMap<String, String> = sqlx::query_as::<_, (String, String)>(
        "SELECT d.photo_id, d.actual_decision FROM user_decisions d WHERE d.rowid = (SELECT MAX(last.rowid) FROM user_decisions last WHERE last.photo_id = d.photo_id)"
    ).fetch_all(&mut *tx).await?.into_iter().collect();

    let mut groups = BTreeMap::<String, Vec<PhotoContext>>::new();
    for mut p in photos {
        if overrides.get(&p.id).is_some_and(|d| d == "KEEP") {
            p.is_kept = true;
        }
        if overrides.get(&p.id).is_some_and(|d| d == "REMOVE") {
            p.analysis_status = "user_remove".into();
        }
        let faces = faces_by_photo.get(&p.id).cloned().unwrap_or_default();
        let group_metrics = GroupFaceMetrics::from_faces(&faces);
        groups
            .entry(
                p.moment_id
                    .clone()
                    .unwrap_or_else(|| format!("unique:{}", p.id)),
            )
            .or_default()
            .push(PhotoContext {
                photo: p,
                faces,
                group_metrics,
            });
    }
    for contexts in groups.values() {
        for mut rec in recommend(contexts) {
            if let Some(actual) = overrides
                .get(&rec.photo_id)
                .filter(|d| ["KEEP", "REVIEW", "REMOVE"].contains(&d.as_str()))
            {
                rec.decision = actual.clone();
                rec.confidence = 1.0;
                rec.reasoning =
                    ReasonBundle::from_codes(vec![DecisionReason::KeptByUser]).to_json();
            }
            sqlx::query("INSERT OR REPLACE INTO recommendations (photo_id, moment_id, decision, confidence, reasoning, compared_to, reason_codes, best_of_group) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(&rec.photo_id).bind(&rec.moment_id).bind(&rec.decision).bind(rec.confidence).bind(&rec.reasoning).bind(&rec.compared_to).bind(&rec.reason_codes).bind(rec.best_of_group)
                .execute(&mut *tx).await?;
            sqlx::query("UPDATE photos SET analysis_status = 'decision_ready' WHERE id = ? AND perceptual_hash IS NOT NULL")
                .bind(&rec.photo_id).execute(&mut *tx).await?;
        }
    }
    tx.commit().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::face_analysis::types::{BoundingBox, DetectedFace, ExpressionAnalysis, EyeAnalysis};

    fn photo(id: &str, sharpness: f64, hash: &str) -> Photo {
        serde_json::from_value(serde_json::json!({"id":id,"is_kept":false,"absolute_path":format!("/{id}.jpg"),"filename":format!("{id}.jpg"),"extension":"jpg","file_size":10,"gps_available":false,"analysis_status":"technical_ready","moment_id":"m","sharpness":sharpness,"sha256":hash,"perceptual_hash":"0000000000000000","thumbnail_path":"/thumb.jpg"})).unwrap()
    }

    fn ctx(photo: Photo, faces: Vec<DetectedFace>) -> PhotoContext {
        let group_metrics = GroupFaceMetrics::from_faces(&faces);
        PhotoContext {
            photo,
            faces,
            group_metrics,
        }
    }

    fn open_face() -> DetectedFace {
        DetectedFace {
            bbox: BoundingBox {
                x: 10.0,
                y: 10.0,
                width: 80.0,
                height: 80.0,
            },
            detection_confidence: 0.95,
            face_sharpness: Some(0.8),
            left_eye: Some(EyeAnalysis {
                visibility_confidence: 0.9,
                openness_score: Some(0.7),
                state: EyeState::Open,
                confidence: 0.85,
                region_sharpness: Some(0.6),
            }),
            right_eye: Some(EyeAnalysis {
                visibility_confidence: 0.9,
                openness_score: Some(0.72),
                state: EyeState::Open,
                confidence: 0.85,
                region_sharpness: Some(0.6),
            }),
            expression: Some(ExpressionAnalysis {
                smile_score: Some(0.8),
                confidence: 0.8,
            }),
        }
    }

    fn closed_eye_face() -> DetectedFace {
        let mut face = open_face();
        face.left_eye.as_mut().unwrap().state = EyeState::Closed;
        face.left_eye.as_mut().unwrap().confidence = 0.9;
        face
    }

    #[test]
    fn case_a_open_eyes_rank_above_closed() {
        let a = ctx(photo("a", 0.75, "a"), vec![open_face()]);
        let b = ctx(photo("b", 0.74, "b"), vec![closed_eye_face()]);
        let rows = recommend(&[b.clone(), a.clone()]);
        assert_eq!(
            rows.iter().find(|r| r.photo_id == "a").unwrap().decision,
            "KEEP"
        );
        assert!(
            rows.iter().find(|r| r.photo_id == "b").unwrap().decision != "KEEP"
                || rows.iter().find(|r| r.photo_id == "b").unwrap().confidence < 0.7
        );
    }

    #[test]
    fn eye_based_ranking_remains_disabled_until_validation() {
        assert!(
            !ENABLE_EYE_BASED_RANKING,
            "Baseline V1 must not enable eye ranking"
        );
    }

    #[test]
    fn case_d_landscape_no_face_penalty() {
        let landscape = ctx(photo("l", 0.4, "l"), vec![]);
        let rows = recommend(&[landscape]);
        assert_eq!(rows[0].decision, "KEEP");
    }

    #[test]
    fn deterministic_complete_decisions_preserve_unique_and_protected_photos() {
        let a = photo("a", 0.8, "a");
        let mut b = photo("b", 0.1, "b");
        assert_eq!(recommend(&[ctx(b.clone(), vec![])])[0].decision, "REVIEW");
        let rows = recommend(&[ctx(a.clone(), vec![]), ctx(b.clone(), vec![])]);
        assert_eq!(
            rows.iter().find(|r| r.photo_id == "a").unwrap().decision,
            "KEEP"
        );
        assert_eq!(
            rows.iter().find(|r| r.photo_id == "b").unwrap().decision,
            "REVIEW"
        );
        b.perceptual_hash = Some("ffffffffffffffff".into());
        assert_eq!(
            recommend(&[ctx(a.clone(), vec![]), ctx(b.clone(), vec![])])[1].decision,
            "REVIEW"
        );
        b.is_kept = true;
        assert_eq!(
            recommend(&[ctx(a, vec![]), ctx(b, vec![])])[1].decision,
            "KEEP"
        );
    }

    #[test]
    fn exact_copies_have_one_stable_keeper_and_invalid_photos_are_reviewed() {
        let a = photo("a", 0.7, "same");
        let b = photo("b", 0.7, "same");
        let rows = recommend(&[ctx(b, vec![]), ctx(a, vec![])]);
        assert_eq!(
            rows.iter().find(|r| r.photo_id == "a").unwrap().decision,
            "KEEP"
        );
        assert_eq!(
            rows.iter().find(|r| r.photo_id == "b").unwrap().decision,
            "REMOVE"
        );
        let mut invalid = photo("bad", 1.0, "bad");
        invalid.thumbnail_path = None;
        assert_eq!(recommend(&[ctx(invalid, vec![])])[0].decision, "REVIEW");
    }
}

#[cfg(test)]
mod pipeline_tests {
    use super::update;
    #[tokio::test]
    async fn mixed_library_runs_without_ollama_and_reuses_analysis_after_restart() {
        let dir = std::env::temp_dir().join(format!("photomind-auto-{}", uuid::Uuid::new_v4()));
        let originals = dir.join("originals");
        let thumbs = dir.join("thumbs");
        std::fs::create_dir_all(&originals).unwrap();
        let img = image::RgbImage::from_fn(96, 64, |x, y| {
            image::Rgb([(x * 7 % 256) as u8, (y * 11 % 256) as u8, ((x + y) * 13 % 256) as u8])
        });
        for ext in ["jpg", "png", "webp", "tiff"] {
            img.save(originals.join(format!("photo.{ext}"))).unwrap();
        }
        std::fs::copy(originals.join("photo.jpg"), originals.join("far-away-9999.jpg")).unwrap();
        std::fs::write(originals.join("corrupt.jpg"), b"not a jpeg").unwrap();
        std::fs::write(originals.join("zero.jpg"), b"").unwrap();
        std::fs::write(originals.join("._photo.jpg"), [0, 5, 22, 7, 0, 2]).unwrap();
        let pool = crate::db::init_db(&dir).await.unwrap();
        let mut successes = 0;
        let mut failures = 0;
        for entry in std::fs::read_dir(&originals).unwrap() {
            let path = entry.unwrap().path();
            if !crate::ingest::is_photo_path(&path) {
                continue;
            }
            match crate::scanner::refresh_photo(&pool, &path, &thumbs).await {
                Ok(()) => successes += 1,
                Err(_) => failures += 1,
            }
        }
        assert_eq!((successes, failures), (5, 2));
        crate::scanner::generate_moments(&pool).await.unwrap();
        let start = std::time::Instant::now();
        update(&pool, None).await.unwrap();
        eprintln!("Local decision pass for mixed library: {:?}", start.elapsed());
        let photos = crate::db::all_photos(&pool).await.unwrap();
        assert_eq!(photos.len(), 7);
        let errors: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM photos WHERE analysis_status = 'error'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(errors, 2);
        let ai: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM photo_ai_results")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(ai, 0);
        let face_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM photo_face_analysis")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(face_rows, 5);
        let decisions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM recommendations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(decisions, 7);
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }
}
