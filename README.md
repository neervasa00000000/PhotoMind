# PhotoMind

Local desktop photo organisation and duplicate review, built with Tauri, React, Rust and SQLite. Core scanning, technical analysis, grouping and suggestions work automatically without Ollama or required model downloads. Advanced AI is optional and experimental.

## Run the optimised app

Build the macOS application with:

```sh
npm run tauri build -- --bundles app
```

Quit older PhotoMind instances, then open `src-tauri/target/release/bundle/macos/PhotoMind.app`. Use this release application for large photo libraries. `npm run tauri dev` uses an unoptimised Rust development build and image processing can be much slower.

## Reopening the application

Each normal launch starts fresh: previous scanned folders, library results, duplicate/moment groups, scan decisions and unused generated previews are cleared. Previous folders are not automatically rescanned. Choose a folder to generate new results. Original photos are untouched. Bin recovery records and their required previews survive, with confirmed Finder deletions reconciled.

Returning to the app also checks for deleted files and reconciles Bin. Visible windows check every 30 seconds (five seconds on Bin). Confirmed stale entries trigger a full results refresh and clear preview caches. Unreferenced generated previews are removed at startup, after scans and during these checks. See [cache refresh behavior](docs/CACHE_REFRESH.md).

## Scan and duplicate review

Choose a folder to index it and its nested folders. Scan other folders to include them in the same library. Duplicate matching covers the entire index, independently of filenames, capture dates, folder locations and library pagination.

- **Exact copies:** identical full-file SHA-256 hashes.
- **Visual matches:** local perceptual-hash candidates requiring review; they are not proof of identical file contents.
- **Moments:** similar photos close in capture time, with automatic local recommendations and large comparisons. The time restriction on Moments does not apply to duplicate matching.

Scanning uses at most two image workers, reduces each full image once, and reuses previews/quality metrics for exact copies. Every visited file is rehashed on rescan to detect changed contents and repair missing file hashes. Unchanged, fully indexed files reuse their existing image analysis. Previews are addressed by file content so editing an original cannot overwrite a preview still used by its copies.

Pause stops scheduling additional batches. Cancellation finishes the in-flight batch before releasing the scan lock. Completed indexing can be reused within the current app session; a new launch resets the active scan library. A filesystem lock prevents simultaneous scans by updated PhotoMind instances sharing the same app database.

## Large previews and Bin

Use **Compare large** to inspect a duplicate group side by side. Each photo has independent zoom; arrow keys switch photos and Escape closes the viewer. The green label identifies the suggested or chosen keeper. **Keep this photo** inside Compare protects your chosen photo from cleanup. **Keep suggested · Bin the rest** on a group protects its keeper and moves the remaining unprotected photos to Bin in one action, including visual-match groups. Choose another keeper to remove that protection.

**Move to Bin** sends originals to macOS Trash and records their location persistently. Open the **Bin** tab to inspect large previews, **Recover** to the original folder, or confirm **Delete permanently**. Recovery refuses to overwrite an existing file and requires the original folder or drive to be available. Emptying system Trash in Finder permanently removes those files too. PhotoMind reconciles Bin on startup, when returning to the app, and every five seconds while viewing Bin, removing records and unused previews for confirmed deletions. Finder recovery is reconciled when the original file bytes match. Unavailable drives and permission failures preserve recovery records. The Bin tracks removals made with this version of PhotoMind.

## Verification and performance measurement

```sh
npm run build
cargo test --offline --manifest-path src-tauri/Cargo.toml
cargo test --offline --release --manifest-path src-tauri/Cargo.toml benchmark_large_photo -- --ignored --nocapture
```

The manual benchmark creates a synthetic 6000 × 4000 JPEG and measures file hashing and image processing separately. In measurements on this machine, image processing took 4.17 seconds before reduced-image reuse and 3.14 seconds afterwards in a development build. The optimised release benchmark took 0.107 seconds after the change. The previous release build was not benchmarked. These are single-image measurements, not promised completion times for a real folder; disk speed, formats, image contents and cache state affect scanning.

Cross-platform CI (`.github/workflows/ci.yml`) runs the frontend build, the Rust test suite and the cleanup-planner regression on macOS, Ubuntu and Windows. Fixture-gated decoder tests skip cleanly when the dev-only `src-tauri/fixtures/` samples are absent.

Regression tests cover pagination beyond 1,000 photos, global matching across separated filenames/folders/dates, changed/missing file hashes, concurrent indexing, immutable shared previews, moment preservation/rollback, AI-result validation and cross-instance scan locking.

Bin regression tests use generated images in isolated temporary folders and cover restart persistence, previews, keeper protection, failed moves, recovery collisions, changed files and permanent removal. A separate macOS integration test verifies a generated image can be moved to actual system Trash and recovered intact:

```sh
cargo test --offline --manifest-path src-tauri/Cargo.toml native_trash_roundtrip -- --ignored --nocapture
```

For isolated interface review, run the Vite development server and open `http://127.0.0.1:1420/.qa/review.html`. This fixture renders the real interface with synthetic photos and mocked file operations; it does not modify your photo library.

## Dashboard and interface patterns

Dashboard separates exact copies, visual-match candidates and completed AI recommendations. Potential cleanup combines unique candidate file sizes, without counting overlapping duplicate groups twice. Visual matches need review and can be cleaned up from Dashboard while retaining suggested/chosen keepers and explicitly protected photos. Group cleanup protects keepers before moving candidates to Bin.

Shared action styles in `src/App.css` define primary (blue), secondary (neutral), keep (green) and Bin/delete (red) controls. Dashboard, photo review and Bin use the same light surfaces and action styles; green labels indicate keeper state. Native dialog focus handling and keyboard navigation remain supported.

Run cleanup-planning regression checks with:

```sh
node --experimental-strip-types .qa/cleanup.test.ts
```

Foreground scans and file actions queue behind short automatic Bin refreshes using a local asynchronous operation gate. Bin refresh skips reconciliation while foreground work owns that gate. Cross-process file locking remains enabled, with a brief retry for transient contention from another updated instance. A regression test verifies scan acquisition waits for Bin refresh and still excludes competing locks.

## Reliability updates

Release builds use panic unwinding so ordinary decoder panics can be caught; signals and allocator aborts cannot be caught this way. Scan tasks are supervised. Individual failed files are recorded and skipped; database persistence failures surface a scan error. Local suggestions run automatically; optional Ollama is probed only when Advanced AI is opened. Thumbnail caches include content hashes so edited files show updated previews. Dashboard cleanup protects keepers, and bulk Bin moves require confirmation.

Phase 0 bounds decoding to two jobs across all entry points, rejects sources above 512 MiB and images above 64 megapixels, limits built-in decoder allocations, and validates AVIF container lengths and signed plane strides. These are safety limits, not a guarantee against every third-party decoder failure. HEIC/HEIF pixel analysis, calibrated face/eye analysis and embeddings are incomplete. See [the stability inspection](docs/PHASE0_CURRENT_STATE.md) and [format compatibility](docs/COMPATIBILITY.md).

Generate an isolated mixed-format stress library with `python3 .qa/make_phase0_library.py /tmp/photomind-test/library --count 1000` (Pillow is a developer-only dependency). For native QA, set both `PHOTOMIND_QA_DATA_DIR` and `PHOTOMIND_QA_SCAN_FOLDER` when launching the executable. The latter refuses to run without an isolated data directory. Never point QA at the user database. The manifest allows checking that analysis leaves original bytes unchanged.

Legacy databases migrate every analysis metric used by indexing, including mean saturation and flat fraction. Scan progress counts both indexed and skipped files; if every file fails, the scan emits an explicit error containing the recorded cause rather than reporting successful completion. The legacy migration regression indexes a generated image through the scanner after migration.

## AI throughput

Photo analysis requests concise structured results and keeps the Ollama model resident for 15 minutes. It reuses HTTP connections and prior results for SHA-identical photos analysed with the same model. Visually similar photos are analysed independently. Rescanning edited content invalidates its AI result, and writes from in-flight analysis are rejected if the photo changed or moved to Bin. Cancellation drops the current HTTP wait promptly. Three consecutive inference failures stop the run instead of retrying every remaining photo. Vision-capable models are required. Actual throughput depends on the installed model and hardware; no whole-library timing guarantee is made.

## License

PhotoMind source code is released under the MIT License. See LICENSE.

PhotoMind includes third-party components that retain their respective licenses. See THIRD_PARTY_LICENSES.md.
