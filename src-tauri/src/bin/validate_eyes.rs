//! Eye detection validation tool (Baseline V1 — no threshold tuning).
//!
//! Usage:
//!   cargo run --bin validate_eyes --release -- \
//!     ../eye-validation/photos \
//!     --ground-truth ../eye-validation/ground_truth.json

use std::path::PathBuf;
use tauri_app_lib::eye_validation::{
    default_root, failure_review_path, ground_truth_path, photos_dir, results_dir,
};
use tauri_app_lib::eye_validation::metrics::{format_report, run_validation};
use tauri_app_lib::face_analysis::{yunet, EyeState};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let root = default_root();
    let mut photo_dir = photos_dir(&root);
    let mut ground_truth_path_arg: Option<PathBuf> = None;
    let mut debug_output_dir: Option<PathBuf> = None;
    let mut failure_path: Option<PathBuf> = None;
    let mut report_path: Option<PathBuf> = None;
    let mut i = 1;
    if args.len() > 1 && !args[1].starts_with("--") {
        photo_dir = PathBuf::from(&args[1]);
        i = 2;
    }
    while i < args.len() {
        match args[i].as_str() {
            "--ground-truth" if i + 1 < args.len() => {
                ground_truth_path_arg = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--debug-output" if i + 1 < args.len() => {
                debug_output_dir = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--failure-review" if i + 1 < args.len() => {
                failure_path = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--report" if i + 1 < args.len() => {
                report_path = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            _ => i += 1,
        }
    }

    let gt_path = ground_truth_path_arg.unwrap_or_else(|| {
        if photo_dir.ends_with("photos") {
            ground_truth_path(&photo_dir.parent().unwrap_or(photo_dir.as_path()).to_path_buf())
        } else {
            ground_truth_path(&root)
        }
    });
    let gt_exists = gt_path.is_file();
    let failure_path = failure_path.or_else(|| {
        gt_exists.then(|| {
            photo_dir
                .parent()
                .map(failure_review_path)
                .unwrap_or_else(|| failure_review_path(&root))
        })
    });
    let report_path = report_path.or_else(|| {
        gt_exists.then(|| {
            photo_dir
                .parent()
                .map(|p| results_dir(p).join("baseline_v1.txt"))
                .unwrap_or_else(|| results_dir(&root).join("baseline_v1.txt"))
        })
    });

    if let Some(ref debug_dir) = debug_output_dir {
        std::fs::create_dir_all(debug_dir)?;
    }

    println!("=== PhotoMind Eye Detection Validation ===\n");
    println!("Photos directory: {}", photo_dir.display());
    if gt_exists {
        println!("Ground truth: {}", gt_path.display());
    } else {
        println!("No ground truth file at {}", gt_path.display());
        println!("Label photos with: cargo run --bin label_eyes --release");
    }
    println!("ENABLE_EYE_BASED_RANKING: false");
    println!("Thresholds unchanged: OPEN>=0.52 CLOSED<=0.38 MIN_FACE=24px\n");

    print_per_photo(&photo_dir, gt_exists.then_some(gt_path.as_path()))?;

    let report = run_validation(
        &photo_dir,
        gt_exists.then_some(gt_path.as_path()),
        failure_path.as_deref(),
        report_path.as_deref(),
    )?;
    if gt_exists {
        print!("{}", format_report(&report));
        if let Some(path) = report_path {
            println!("\nReport saved: {}", path.display());
        }
        if let Some(path) = failure_path {
            println!("Failure review (local, gitignored): {}", path.display());
        }
        println!("\nBaseline V1 complete. Do not enable eye ranking from this run.");
    } else {
        println!(
            "\nImages processed: {}  failed: {}  faces: {}",
            report.images_processed, report.images_failed, report.faces_detected
        );
        println!("Run the labeler, then re-run with --ground-truth.");
    }
    Ok(())
}

fn print_per_photo(
    photo_dir: &std::path::Path,
    ground_truth: Option<&std::path::Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let gt = match ground_truth {
        Some(path) => Some(tauri_app_lib::eye_validation::load_ground_truth(path)?),
        None => None,
    };
    let images = tauri_app_lib::eye_validation::list_image_files(photo_dir)?;
    println!("Found {} image files\n", images.len());
    for img_path in &images {
        let filename = img_path.file_name().unwrap().to_string_lossy().to_string();
        println!("--- {} ---", filename);
        let rgb = match tauri_app_lib::ingest::load_normalized(img_path, 512) {
            Ok(img) => img.into_rgb8(),
            Err(e) => {
                println!("  ERROR: Could not load image: {}\n", e);
                continue;
            }
        };
        let analysis = match yunet::analyze_yunet(&yunet::AnalysisImage::from_rgb(rgb)) {
            Ok(a) => a,
            Err(e) => {
                println!("  ERROR: Face analysis failed: {}\n", e);
                continue;
            }
        };
        if analysis.faces.is_empty() {
            println!("  No faces detected\n");
            continue;
        }
        println!("  {} face(s) detected", analysis.faces.len());
        let gt_faces = gt.as_ref().and_then(|g| g.files.get(&filename));
        for (face_idx, face) in analysis.faces.iter().enumerate() {
            println!("\n  Face {}", face_idx + 1);
            println!("    Detection confidence: {:.2}", face.detection_confidence);
            println!(
                "    Bounding box: x={:.0} y={:.0} w={:.0} h={:.0}",
                face.bbox.x, face.bbox.y, face.bbox.width, face.bbox.height
            );
            let gt_face = gt_faces.and_then(|faces| faces.iter().find(|f| f.face == face_idx));
            report_eye("SUBJECT'S LEFT", &face.left_eye, gt_face.map(|f| f.left_eye.as_str()));
            report_eye(
                "SUBJECT'S RIGHT",
                &face.right_eye,
                gt_face.map(|f| f.right_eye.as_str()),
            );
        }
        println!();
    }
    Ok(())
}

fn report_eye(side: &str, eye: &Option<tauri_app_lib::face_analysis::EyeAnalysis>, gt: Option<&str>) {
    println!("    {} EYE:", side);
    if let Some(eye_data) = eye {
        println!("      State: {:?}", eye_data.state);
        println!(
            "      Openness score: {}",
            eye_data
                .openness_score
                .map(|s| format!("{:.3}", s))
                .unwrap_or_else(|| "None".to_string())
        );
        println!("      Confidence: {:.2}", eye_data.confidence);
        if let Some(gt) = gt {
            println!("      Ground truth: {}", gt);
            match (gt, &eye_data.state) {
                ("OPEN", EyeState::Open) | ("CLOSED", EyeState::Closed) => println!("      ✓ CORRECT"),
                ("OPEN", EyeState::Closed) => println!("      ✗ FALSE-CLOSED (DANGEROUS)"),
                ("CLOSED", EyeState::Open) => println!("      ✗ FALSE-OPEN"),
                _ => {}
            }
        }
    } else {
        println!("      No eye data");
    }
}
