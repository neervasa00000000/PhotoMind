# PhotoMind — Phase 0 stability report

**Later preference change:** the user explicitly requested an empty active library after closing/reopening. Normal startup now resets scan results, active scan decisions, groups, folder history and unused previews, preserving originals and Bin recovery. Persistent-launch/resume observations below describe the earlier tested behavior and are historical. See [current launch behavior](CACHE_REFRESH.md).

Date: 18 September 2026. Scope: existing application stability; no new models, embeddings, product features or interface redesign. Initial architecture, size, dependency and risk inspection: [PHASE0_CURRENT_STATE.md](PHASE0_CURRENT_STATE.md).

## Crash root cause and evidence

Two current malformed AVIF container cases were reproduced before fixes: `boxes` attempted a `16..8` slice when an extended box length was smaller than its header; `find_primary_nclx` sliced a one-byte metadata payload at offset four. The captured backtrace led through `rust_begin_unwind` and `slice_index_fail` to these functions. Both regression tests failed before the fix and pass afterwards.

Historical SIGSEGV reports identify `avif::copy_picture_planes` → `Vec::extend_from_slice` → `_platform_memmove`. Inspection found negative row strides cast to unsigned offsets, violating pointer-arithmetic requirements. Signed row offsets, checked dimensions/strides and explicit error returns now replace that path, with a negative-stride regression. This establishes a corrected defect, not proof of the exact historical pointer. Historical stripped SIGABRT reports cannot be conclusively attributed without their matching executable and output.

## Fixes and files changed

| Files/functions | Result |
|---|---|
| `src-tauri/src/avif.rs`: `boxes`, `find_primary_nclx`, `copy_picture_planes`, `drain_one_picture` | Validate container lengths, metadata slices, pixel layout/depth, dimensions and strides; checked allocations; unref decoded pictures on copy errors |
| `src-tauri/src/ingest.rs`: `detect_format`, built-in/RAW decode, `load_normalized_info` | Read a bounded 4 KiB header, recognize wrongly named JPEGs, retain compatible HEIC/HEIF and TIFF-based RAW hints; cap source size at 512 MiB and dimensions at 64 MP; built-in allocation budget 512 MiB; global two-decoder limit including preview/optional AI entry points |
| `src-tauri/src/scanner.rs`: `run_scan`, `refresh_photo`, `fail_scan` | Session/photo/stage logs; surface session/checkpoint/pairing/decision/error-record persistence failures; drain active workers before failing; reuse only complete cached technical metrics |
| `src-tauri/src/db.rs`: `init_db` | Configure foreign keys and journal retention per pooled connection; remove redundant startup truncation |
| `src-tauri/src/lib.rs`: setup/run, thumbnail command | Non-panicking logger initialization and setup error handling; offload preview regeneration; explicit isolated native QA data/folder overrides |
| `src-tauri/src/similarity.rs`, `decisions.rs` | Reject malformed hashes; uncertain focus/similarity differences become REVIEW, rather than heuristic rejection |
| `src/App.tsx`, `PhotoViewer.tsx` | Optional Ollama probe only when Advanced AI opens; bulk Bin confirmation; preserving unrelated Library keepers; additional refresh-generation checks |
| `.gitignore`, README, compatibility/inspection/report docs, `.qa/make_phase0_library.py`, `.qa/native_phase0.py` | Separate release size from development/model storage; reproducible isolated mixed-library and interruption checks |

## Database changes

No schema or model tables added by this Phase 0 work. Every pool connection enforces foreign keys and has a 64 MiB journal retention setting. That setting governs checkpoint retention, not a hard bound on a live WAL. Existing migration tests pass. Persisted technical rows must be complete before reuse; missing/incomplete analysis is recomputed. User decisions, protected keepers, library and Bin persistence remain in place.

## Tests added and passed

Four new regressions cover truncated AVIF metadata, undersized extended boxes, negative strides, and wrong extensions/oversized headers. Existing tests now check integrity settings on all five database connections, malformed hashes, and conservative REVIEW decisions.

- Full `cargo test --offline --manifest-path src-tauri/Cargo.toml`: **77 passed, 0 failed, 2 explicitly manual tests ignored, 0 filtered**, 96.74 seconds. Includes actual RAW camera-manifest and AVIF profile sweeps, built-in formats, corrupt/unsupported files, global duplicate detection, migrations, restart reuse, keeper/Bin protection and concurrency.
- `cargo check --offline --manifest-path src-tauri/Cargo.toml`: passed.
- `npm run build`: passed.
- `node --experimental-strip-types .qa/cleanup.test.ts`: passed. No separate React unit-test runner is configured.
- Offline Tauri release build with `--bundles app`: passed, optimized compilation 3 minutes 57 seconds.

The first post-fix full run exposed a HEIC/HEIF label regression: 76 passed, one failed. Compatible format hints were corrected and the complete suite rerun successfully. The earlier malformed-parser failures were intentional before-fix reproductions. No unresolved failure remains in the final suite. Manual Trash roundtrip and large-photo benchmark are the two ignored tests; they were not rerun in this Phase 0 pass.

## Actual native application checks

The packaged macOS executable was launched three times against an isolated **1,021-file / 53.9 MB** developer library. The app and its children had network access denied by macOS sandbox rules; PATH contained no Ollama executable. Core scanning therefore could not depend on an Ollama server or other network service. Ollama inference itself was not exercised.

Fixtures included ordinary JPEG/PNG/WebP/TIFF/BMP, a 6000 × 4000 JPEG, CR3/NEF samples, a real AVIF, unsupported HEIC, separated exact copies, a JPEG named `.cr3`, blurred/dark/bright/black/white images, malformed AVIF/PNG, truncated RAW and a zero-byte JPEG. The 1,000 stress JPEGs are synthetic 160 × 120 images; these timings are **not** a benchmark for 1,000 camera-resolution photographs.

| Native run | Observation | Launch-to-checkpoint/completion |
|---|---|---:|
| Interrupted | SIGTERM during scan at 106 / 1,021; persisted completed work | 3.691 s |
| Reopened after interruption | Previous session marked interrupted; all 1,021 files visited; 106 preview timestamps unchanged | 7.442 s |
| Reopened completed library | Reused analysis; persisted seeded user KEEP and protected keeper | 2.582 s |

Final isolated state: **1,016 decision-ready, five ERROR**. Failed/unsupported files were recorded and scanning completed; subsequent verification recorded those same failures again. Decisions: 1,006 KEEP, 14 REVIEW, one REMOVE. Exact content matching found the separated JPEG copies and wrong-extension copy. Optional AI result count remained zero. SHA-256 verification confirmed **all 1,021 original files unchanged**.

The tested release was also launched with the existing user database. Startup initialized successfully and verified available saved folders; actual sessions completed without file failures. The resulting library contained **867 decision-ready photos**, with eight protected keepers retained. This was automatic verification of saved folders, including newly available originals, rather than a reset of the user library.

The final direct launch logged `Database initialized` and remained running during the follow-up check. Some earlier application sessions had already exited when later checked; one captured direct session exited with status zero. No new native crash report appeared. Those exits do not establish a crash, and their cause was not proven.

Native visual inspection could not complete: the Computer Use tool reported that macOS Accessibility/Screen Recording permissions were not granted. Folder-picker clicks, visible thumbnail rendering, Compare/zoom interactions and confirmation-dialog appearance therefore remain unverified in this pass. Native backend execution/persistence checks above were real packaged-app runs, not a browser mock. Dedicated real-face/closed-eye accuracy fixtures were not evaluated; lightweight face/eye CV does not exist yet.

Native evidence: `.qa/native-phase0/result.json` and per-run JSON logs. These generated files, test library and isolated database are excluded from source control and the bundle.

## Memory observations

System: Apple M4 MacBook Pro, 10 CPU cores, 16 GB RAM. Main-process RSS sampled every approximately 50 ms during native runs:

- Interrupted run: 104,288 KiB (~102 MiB).
- Resumed mixed-format run: **1,129,968 KiB (~1.08 GiB)**.
- Completed-library reopen: 149,200 KiB (~146 MiB).

These are maximum observed samples, not exact allocator peaks; separate webview processes are excluded. No leak was established. The mixed-format spike is material: RAW full-development buffers and third-party decoder allocations can be large despite two permits and image-size limits. Stage-level allocation profiling is needed before attributing the peak precisely. Source-size caps, bounded workers and checked AVIF copying reduce risk, but do not establish an overall application RAM ceiling.

The isolated final database was 1,265,664 bytes (~1.21 MiB); previews totaled 25,428,785 bytes (~24.25 MiB).

## Current size and model requirement

| Category | Measured/required size |
|---|---:|
| Tested macOS app bundle, disk allocation | **~23 MiB (~0.022 GiB)** |
| Required model weights | **0 bytes** |
| Required Ollama installation/download | **0 bytes** |
| Existing optional Ollama models on this machine | ~8.8 GiB, retained |
| Existing user database/previews at audit | ~25 MiB; varies with library |
| Debug build artifacts | ~20 GiB, developer only |
| Release build artifacts | ~3.8 GiB, developer only |
| Camera test fixtures | ~325 MiB, developer only |
| node_modules | ~148 MiB, developer only |

No model or build cache was blindly deleted. The distribution is already small. Zero required model size also means the planned face/eye/expression/embedding features are not implemented by small models yet.

## Known issues and next phase

Historical SIGABRT root cause remains unknown. A universal no-crash guarantee is not justified: native signals/OOM cannot be caught by Rust unwind handlers, and third-party RAW paths are not isolated in separate processes. RAW memory deserves further profiling. Global visual grouping remains quadratic and 10,000–50,000-photo scale was not measured. Frontend refresh races have additional guards but were not exhaustively stress-tested.

Reopening currently rediscovers/rehashes saved folders and reuses completed decoding; it does not resume an exact per-stage queue at the same numerical progress count. Algorithm-version selective invalidation, small face/eye/expression models, embeddings, calibrated confidence, learning and export remain future work. HEIC/HEIF pixels remain unavailable. Some optional face tests exercise supplied structured values rather than actual CV inference. Windows/Linux native releases and public redistribution/license obligations remain unvalidated.

**Stop here at Phase 0.** After remaining native UI checks and any required stability follow-up, Phase 1 is the persistent automatic pipeline with durable stage resume and progressive results. Model selection/license research belongs to Phase 2; no model was added now.
