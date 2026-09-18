"""Native fresh-launch regression, using generated photos and an isolated Bin."""
from pathlib import Path
import json
import os
import sqlite3
import subprocess
import tempfile
import time
from PIL import Image

root = Path(__file__).resolve().parents[1]
qa = Path(tempfile.mkdtemp(prefix="photomind-refresh-"))
folder = qa / "library"
data = qa / "data"
folder.mkdir()
data.mkdir()
for name, color in [("keep", "red"), ("deleted", "blue")]:
    Image.new("RGB", (80, 60), color).save(folder / f"{name}.jpg")
db = data / "photomind.db"
exe = root / "src-tauri/target/release/bundle/macos/PhotoMind.app/Contents/MacOS/tauri-app"
env = dict(os.environ, PHOTOMIND_QA_DATA_DIR=str(data), PHOTOMIND_QA_SCAN_FOLDER=str(folder))

def run(label, expected_session, scan=True):
    launch_env = env.copy()
    if not scan: launch_env.pop("PHOTOMIND_QA_SCAN_FOLDER")
    with (qa / f"{label}.log").open("w") as log:
        process = subprocess.Popen([str(exe)], env=launch_env, stdout=log, stderr=log)
        try:
            for _ in range(300):
                assert process.poll() is None, "Native app exited unexpectedly"
                if db.exists():
                    with sqlite3.connect(db) as connection:
                        try:
                            row = connection.execute("SELECT id,status FROM scan_sessions ORDER BY id DESC LIMIT 1").fetchone()
                            if not scan and _ >= 40:
                                assert row is None, "Previous folder was scanned again"
                                assert connection.execute("SELECT COUNT(*) FROM photos WHERE NOT EXISTS (SELECT 1 FROM bin_entries b WHERE b.photo_id=photos.id)").fetchone()[0] == 0
                                return
                            if row and row[0] >= expected_session and row[1] == "completed":
                                return
                        except sqlite3.OperationalError:
                            pass
                time.sleep(0.1)
            raise TimeoutError("Native scan did not complete")
        finally:
            process.terminate()
            process.wait(timeout=10)

run("initial", 1)
with sqlite3.connect(db) as connection:
    kept = connection.execute("SELECT id FROM photos WHERE filename='keep.jpg'").fetchone()[0]
    connection.execute("INSERT INTO protected_photos(photo_id) VALUES (?)", (kept,))
    old_thumb = Path(connection.execute("SELECT thumbnail_path FROM photos WHERE filename='keep.jpg'").fetchone()[0])
    binned, bin_thumb = connection.execute("SELECT id,thumbnail_path FROM photos WHERE filename='deleted.jpg'").fetchone()
    bin_path = qa / "bin.jpg"
    (folder / "deleted.jpg").rename(bin_path)  # Only this script's generated original.
    connection.execute("INSERT INTO bin_entries(photo_id,original_path,trash_path,state) VALUES (?,?,?,'active')", (binned,str(folder / "deleted.jpg"),str(bin_path)))
orphan = data / "thumbnails/orphan.jpg"
orphan.write_bytes(b"unused generated test preview")
run("reopened", 2, scan=False)
with sqlite3.connect(db) as connection:
    names = connection.execute("SELECT filename FROM photos").fetchall()
    assert names == [("deleted.jpg",)], names
    assert connection.execute("SELECT COUNT(*) FROM protected_photos WHERE photo_id=?", (kept,)).fetchone()[0] == 0
    assert connection.execute("SELECT COUNT(*) FROM bin_entries WHERE photo_id=?", (binned,)).fetchone()[0] == 1
    for table in ["scan_sessions", "scan_errors", "recommendations", "moment_groups"]:
        assert connection.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0] == 0
assert not old_thumb.exists()
assert not orphan.exists()
assert (folder / "keep.jpg").is_file()
assert bin_path.is_file() and Path(bin_thumb).is_file()
print(json.dumps({"result": "PASS", "evidence": str(qa), "active_library_empty": True, "previous_folders_not_reloaded": True, "stale_previews_removed": True, "originals_and_bin_preserved": True}))
