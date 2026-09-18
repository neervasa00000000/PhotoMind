"""Exercise the packaged Tauri backend with isolated fixtures and persisted state.

Requires permission to launch a native GUI. No user-library database is opened.
RSS is sampled for the main native process, not the separate webview processes.
Historical persistent-launch harness: current fresh-launch preference intentionally
supersedes its resume assertions. Use native_refresh.py for the current behavior.
"""
import hashlib
import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import time

root = Path(__file__).resolve().parents[1]
qa = root / ".qa/native-phase0"
library = qa / "library"
data = qa / "data"
data.mkdir(exist_ok=False)
exe = root / "src-tauri/target/release/bundle/macos/PhotoMind.app/Contents/MacOS/tauri-app"
env = dict(os.environ, PHOTOMIND_QA_DATA_DIR=str(data), PHOTOMIND_QA_SCAN_FOLDER=str(library), PATH="/usr/bin:/bin")
db = data / "photomind.db"
results = []

def query(sql):
    with sqlite3.connect(db, timeout=10) as connection:
        return connection.execute(sql).fetchall()

def launch(label, interrupt=False):
    begin = time.monotonic()
    peak = 0
    previous_id = query("SELECT COALESCE(MAX(id), 0) FROM scan_sessions")[0][0] if db.is_file() else 0
    with (qa / f"{label}.log").open("w") as log:
        # Deny network access to the app and its children, including Ollama HTTP.
        command = ["/usr/bin/sandbox-exec", "-p", "(version 1) (allow default) (deny network*)", str(exe)]
        process = subprocess.Popen(command, env=env, stdout=log, stderr=log)
        last_session = None
        while time.monotonic() - begin < 180:
            if process.poll() is not None:
                raise RuntimeError(f"Native app exited unexpectedly: {process.returncode}; see {label}.log")
            sample = subprocess.run(["ps", "-o", "rss=", "-p", str(process.pid)], capture_output=True, text=True)
            if sample.returncode == 0 and sample.stdout.strip():
                peak = max(peak, int(sample.stdout.strip()))
            try:
                rows = query("SELECT id, status, total_files, processed_files FROM scan_sessions ORDER BY id DESC LIMIT 1")
                if rows and last_session is None:
                    last_session = rows[0][0]
                if rows and rows[0][1] == "running" and interrupt and rows[0][3] >= 100:
                    record = {"run": label, "interrupted_at": rows[0][3], "session": rows[0], "elapsed_seconds": round(time.monotonic()-begin, 3), "sampled_main_rss_kib": peak}
                    break
                # A startup scan begins after two seconds: ignore old completion.
                if rows and rows[0][0] > previous_id and rows[0][1] == "completed":
                    record = {"run": label, "session": rows[0], "elapsed_seconds": round(time.monotonic()-begin, 3), "sampled_main_rss_kib": peak}
                    break
                if rows and rows[0][1] == "failed":
                    raise RuntimeError(f"Scan failed: {rows[0]}")
            except sqlite3.OperationalError:
                pass
            time.sleep(0.05)
        else:
            process.terminate(); process.wait(timeout=10)
            raise TimeoutError(label)
        # SIGTERM deliberately exercises interruption, including recovery of locks.
        process.send_signal(signal.SIGTERM)
        process.wait(timeout=10)
        results.append(record)
        print(json.dumps(record), flush=True)

launch("interrupted", interrupt=True)
before = {p.name: p.stat().st_mtime_ns for p in (data / "thumbnails").glob("*")}
launch("resumed")
after = {p.name: p.stat().st_mtime_ns for p in (data / "thumbnails").glob("*")}
assert all(after.get(name) == modified for name, modified in before.items()), "Completed previews were regenerated"
assert query("SELECT COUNT(*) FROM scan_sessions WHERE status='interrupted'")[0][0] >= 1
assert query("SELECT COUNT(*) FROM scan_errors")[0][0] >= 5
assert query("SELECT COUNT(*) FROM photo_ai_results")[0][0] == 0
assert query("SELECT COUNT(*) FROM photos WHERE sha256=(SELECT sha256 FROM photos WHERE filename='normal.jpg')")[0][0] >= 3
assert query("SELECT COUNT(*) FROM photos WHERE analysis_status='decision_ready'")[0][0] >= 1000
keeper = query("SELECT id FROM photos WHERE filename='normal.jpg'")[0][0]
with sqlite3.connect(db) as connection:
    connection.execute("INSERT OR IGNORE INTO protected_photos(photo_id) VALUES (?)", (keeper,))
    connection.execute("INSERT INTO user_decisions(photo_id, recommended_decision, actual_decision) VALUES (?, 'REVIEW', 'KEEP')", (keeper,))
launch("reopened")
assert query(f"SELECT COUNT(*) FROM protected_photos WHERE photo_id='{keeper}'")[0][0] == 1
assert query(f"SELECT COUNT(*) FROM user_decisions WHERE photo_id='{keeper}' AND actual_decision='KEEP'")[0][0] == 1
manifest = json.loads((qa / "library-manifest.json").read_text())
assert all(hashlib.sha256((library / name).read_bytes()).hexdigest() == digest for name, digest in manifest.items()), "An original changed"
report = {"runs": results, "network_access": "denied by macOS sandbox for app and children", "ollama_cli_in_path": False, "reused_previews_after_interruption": len(before), "photo_states": query("SELECT analysis_status, COUNT(*) FROM photos GROUP BY analysis_status"), "decisions": query("SELECT decision, COUNT(*) FROM recommendations GROUP BY decision"), "errors": query("SELECT stage, COUNT(*) FROM scan_errors GROUP BY stage"), "originals_unchanged": len(manifest), "required_models_bytes": 0, "ollama_results": 0, "database_bytes": db.stat().st_size, "preview_bytes": sum(p.stat().st_size for p in (data / "thumbnails").glob("*"))}
(qa / "result.json").write_text(json.dumps(report, indent=2))
print(json.dumps(report), flush=True)
