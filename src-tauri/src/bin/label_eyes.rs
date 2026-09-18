//! Development-only local eye labeling UI.
//!
//! Usage (from src-tauri):
//!   cargo run --bin label_eyes --release
//!
//! Not packaged into PhotoMind releases. Binds to 127.0.0.1 only.

use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri_app_lib::eye_validation::{
    self, build_work_items, compute_progress, default_root, ensure_validation_dirs, face_labels,
    failure_review_path, first_unlabeled_index, ground_truth_path, list_image_files,
    load_ground_truth, mark_photo_reviewed, photos_dir, results_dir, save_ground_truth,
    upsert_face_label, GroundTruth, PreparedPhoto, WorkItem, WorkKind, COMPOSITION_CHECKLIST,
    MIN_USABLE_EYES,
};

const HTML: &str = include_str!("../eye_validation/labeler.html");

struct Session {
    root: PathBuf,
    photos_dir: PathBuf,
    gt_path: PathBuf,
    ground_truth: GroundTruth,
    cache: BTreeMap<String, PreparedPhoto>,
    items: Vec<WorkItem>,
    current: usize,
}

impl Session {
    fn photo_names(&self) -> Vec<String> {
        self.items
            .iter()
            .map(|i| i.filename.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn progress(&self) -> eye_validation::LabelProgress {
        compute_progress(&self.ground_truth, &self.photo_names())
    }

    fn session_json(&self) -> serde_json::Value {
        let progress = self.progress();
        json!({
            "photos_dir": self.photos_dir.display().to_string(),
            "ground_truth_path": self.gt_path.display().to_string(),
            "current": self.current,
            "total": self.items.len(),
            "progress": progress,
            "checklist": COMPOSITION_CHECKLIST,
        })
    }

    fn persist(&self) -> Result<(), String> {
        save_ground_truth(&self.gt_path, &self.ground_truth)
    }
}

fn boot(root: PathBuf) -> Result<Session, String> {
    ensure_validation_dirs(&root)?;
    let photos = photos_dir(&root);
    let gt_path = ground_truth_path(&root);
    let ground_truth = load_ground_truth(&gt_path)?;
    let photo_paths = list_image_files(&photos)?;
    println!("PhotoMind eye labeler (development only)");
    println!("Photos: {}", photos.display());
    println!("Labels: {}", gt_path.display());
    println!("Scanning {} image(s) for faces (no eye predictions shown)...", photo_paths.len());
    let mut cache = BTreeMap::new();
    let items = build_work_items(&photo_paths, &mut cache);
    let current = first_unlabeled_index(&items, &ground_truth);
    println!("{} labeling item(s). Resuming at {}.", items.len(), current + 1);
    Ok(Session {
        root,
        photos_dir: photos,
        gt_path,
        ground_truth,
        cache,
        items,
        current,
    })
}

fn item_json(session: &Session, index: usize) -> serde_json::Value {
    if session.items.is_empty() {
        return json!({"status": "empty", "message": "No photographs in eye-validation/photos/"});
    }
    let index = index.min(session.items.len() - 1);
    let item = &session.items[index];
    let prepared = session.cache.get(&item.filename);
    match &item.kind {
        WorkKind::Face {
            face_index,
            face_count,
        } => {
            let (jpeg, width, height, face) = match prepared {
                Some(PreparedPhoto::Faces {
                    jpeg,
                    width,
                    height,
                    faces,
                }) => (
                    jpeg.clone(),
                    *width,
                    *height,
                    faces.get(*face_index).cloned(),
                ),
                _ => (String::new(), 0, 0, None),
            };
            let labels = face_labels(&session.ground_truth, &item.filename, *face_index);
            json!({
                "index": index,
                "filename": item.filename,
                "status": "face",
                "jpeg": jpeg,
                "width": width,
                "height": height,
                "face_count": face_count,
                "face": serde_json::to_value(face).unwrap_or(serde_json::Value::Null),
                "left_eye": labels.and_then(|f| {
                    if f.left_eye.is_empty() { None } else { Some(f.left_eye.clone()) }
                }),
                "right_eye": labels.and_then(|f| {
                    if f.right_eye.is_empty() { None } else { Some(f.right_eye.clone()) }
                }),
            })
        }
        WorkKind::NoFaces => {
            let (jpeg, width, height) = match prepared {
                Some(PreparedPhoto::NoFaces { jpeg, width, height }) => {
                    (jpeg.clone(), *width, *height)
                }
                _ => (String::new(), 0, 0),
            };
            json!({
                "index": index,
                "filename": item.filename,
                "status": "nofaces",
                "message": "No face detected in this photograph. Press Skip photo.",
                "jpeg": jpeg,
                "width": width,
                "height": height,
            })
        }
        WorkKind::Missing => json!({
            "index": index,
            "filename": item.filename,
            "status": "missing",
            "message": "Image file is missing.",
        }),
        WorkKind::Unreadable { message } => json!({
            "index": index,
            "filename": item.filename,
            "status": "unreadable",
            "message": format!("Could not read this photograph: {message}"),
        }),
    }
}

#[derive(Deserialize)]
struct LabelBody {
    index: usize,
    left_eye: Option<String>,
    right_eye: Option<String>,
}

#[derive(Deserialize)]
struct IndexBody {
    index: usize,
}

fn handle(session: &Arc<Mutex<Session>>, mut stream: TcpStream) {
    stream.set_read_timeout(Some(Duration::from_secs(30))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(30))).ok();
    match read_request(&mut stream) {
        Ok((method, path, body)) => {
            let (status, content_type, payload) = route(session, &method, &path, &body);
            respond(&mut stream, status, content_type, &payload);
        }
        Err(error) => respond(
            &mut stream,
            400,
            "text/plain; charset=utf-8",
            error.as_bytes(),
        ),
    }
}

fn route(
    session: &Arc<Mutex<Session>>,
    method: &str,
    path: &str,
    body: &[u8],
) -> (u16, &'static str, Vec<u8>) {
    let (path_only, query) = split_query(path);
    match (method, path_only) {
        ("GET", "/") => (200, "text/html; charset=utf-8", HTML.as_bytes().to_vec()),
        ("GET", "/api/session") => json_ok(session.lock().unwrap().session_json()),
        ("GET", "/api/item") => {
            let i = query_i(query).unwrap_or_else(|| session.lock().unwrap().current);
            json_ok(item_json(&session.lock().unwrap(), i))
        }
        ("POST", "/api/label") => match serde_json::from_slice::<LabelBody>(body) {
            Ok(payload) => {
                let mut s = session.lock().unwrap();
                if let Some(item) = s.items.get(payload.index).cloned() {
                    if let WorkKind::Face { face_index, .. } = item.kind {
                        if let Err(e) = upsert_face_label(
                            &mut s.ground_truth,
                            &item.filename,
                            face_index,
                            payload.left_eye.as_deref(),
                            payload.right_eye.as_deref(),
                        ) {
                            return text_err(400, e);
                        }
                        if let Err(e) = s.persist() {
                            return text_err(500, e);
                        }
                    }
                }
                json_ok(s.session_json())
            }
            Err(e) => text_err(400, e.to_string()),
        },
        ("POST", "/api/goto") => match serde_json::from_slice::<IndexBody>(body) {
            Ok(payload) => {
                let mut s = session.lock().unwrap();
                if !s.items.is_empty() {
                    s.current = payload.index.min(s.items.len() - 1);
                }
                json_ok(s.session_json())
            }
            Err(e) => text_err(400, e.to_string()),
        },
        ("POST", "/api/save") => {
            let s = session.lock().unwrap();
            if let Err(e) = s.persist() {
                return text_err(500, e);
            }
            json_ok(s.session_json())
        }
        ("POST", "/api/skip-photo") => match serde_json::from_slice::<IndexBody>(body) {
            Ok(payload) => {
                let mut s = session.lock().unwrap();
                if let Some(item) = s.items.get(payload.index).cloned() {
                    match item.kind {
                        WorkKind::Face { .. } => {}
                        _ => {
                            mark_photo_reviewed(&mut s.ground_truth, &item.filename);
                            if let Err(e) = s.persist() {
                                return text_err(500, e);
                            }
                        }
                    }
                    let next = s
                        .items
                        .iter()
                        .enumerate()
                        .find(|(idx, other)| *idx > payload.index && other.filename != item.filename)
                        .map(|(idx, _)| idx)
                        .unwrap_or(payload.index);
                    s.current = next;
                }
                json_ok(s.session_json())
            }
            Err(e) => text_err(400, e.to_string()),
        },
        ("POST", "/api/validate") => {
            let s = session.lock().unwrap();
            let usable = s.progress().usable_eyes;
            if usable < MIN_USABLE_EYES {
                return json_ok(json!({
                    "ok": false,
                    "message": format!("Need at least {MIN_USABLE_EYES} usable labelled eyes (OPEN+CLOSED). Currently {usable}."),
                }));
            }
            let report_path = results_dir(&s.root).join("baseline_v1.txt");
            let failure_path = failure_review_path(&s.root);
            match tauri_app_lib::eye_validation::metrics::run_validation(
                &s.photos_dir,
                Some(&s.gt_path),
                Some(&failure_path),
                Some(&report_path),
            ) {
                Ok(_) => json_ok(json!({
                    "ok": true,
                    "message": format!(
                        "Baseline report written to {} and {}. Keep labeling if you want more diversity; do not enable ranking from this.",
                        report_path.display(),
                        failure_path.display()
                    ),
                })),
                Err(e) => text_err(500, e),
            }
        }
        _ => text_err(404, "not found".into()),
    }
}

fn json_ok(value: serde_json::Value) -> (u16, &'static str, Vec<u8>) {
    (
        200,
        "application/json; charset=utf-8",
        value.to_string().into_bytes(),
    )
}

fn text_err(status: u16, message: String) -> (u16, &'static str, Vec<u8>) {
    (status, "text/plain; charset=utf-8", message.into_bytes())
}

fn split_query(path: &str) -> (&str, &str) {
    match path.split_once('?') {
        Some((p, q)) => (p, q),
        None => (path, ""),
    }
}

fn query_i(query: &str) -> Option<usize> {
    for part in query.split('&') {
        if let Some(("i", v)) = part.split_once('=') {
            return v.parse().ok();
        }
    }
    None
}

fn read_request(stream: &mut TcpStream) -> Result<(String, String, Vec<u8>), String> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        let n = stream.read(&mut tmp).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(header_end) = find_double_crlf(&buf) {
            let header = String::from_utf8_lossy(&buf[..header_end]).to_string();
            let mut lines = header.split("\r\n");
            let request_line = lines.next().unwrap_or("");
            let mut parts = request_line.split_whitespace();
            let method = parts.next().unwrap_or("GET").to_string();
            let path = parts.next().unwrap_or("/").to_string();
            let mut content_length = 0usize;
            for line in lines {
                let lower = line.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
            }
            let body_start = header_end + 4;
            while buf.len() < body_start + content_length {
                let n = stream.read(&mut tmp).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            let body = buf.get(body_start..).unwrap_or(&[]).to_vec();
            return Ok((method, path, body));
        }
        if buf.len() > 2 * 1024 * 1024 {
            return Err("request too large".into());
        }
    }
    Err("incomplete request".into())
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", url])
        .spawn();
}

fn main() {
    let mut root = default_root();
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--root" if i + 1 < args.len() => {
                root = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            _ => i += 1,
        }
    }
    let session = match boot(root) {
        Ok(s) => Arc::new(Mutex::new(s)),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let listener = TcpListener::bind("127.0.0.1:8765")
        .or_else(|_| TcpListener::bind("127.0.0.1:0"))
        .expect("bind local labeler port");
    let addr = listener.local_addr().expect("local addr");
    let url = format!("http://127.0.0.1:{}", addr.port());
    println!("Open {url}");
    println!("This tool is local-only and is not part of the PhotoMind app.");
    open_browser(&url);
    for incoming in listener.incoming() {
        if let Ok(stream) = incoming {
            let session = session.clone();
            std::thread::spawn(move || handle(&session, stream));
        }
    }
}
