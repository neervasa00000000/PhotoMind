# PhotoMind current state — Phase 0 inspection

## Application architecture

Existing application; no rebuild or UI redesign is planned. React 19/TypeScript renders the interface in Tauri 2's system webview. Vite builds the frontend; Tailwind and shared CSS supply styling. Rust handles scanning, ingestion, quality metrics, grouping, suggestions and filesystem actions. SQLite/SQLx persists photos, derived metrics, recommendations, overrides, scan sessions/errors, RAW/JPEG logical pairs and recoverable Bin journals. macOS release packaging contains the native executable plus embedded frontend; Node/Rust/Python are development tools, not end-user requirements.

Scanning uses two asynchronous jobs per batch and blocking tasks for hashing/decoding. Common formats use `image`; RAW uses embedded-preview-first `rawler`; AVIF uses `avif-parse`/`rav1d` through a custom pixel-plane adapter. Thumbnails are content-addressed. Rust computes SHA-256, dHash, sharpness, brightness, contrast and clipping. A local decision engine handles core suggestions automatically. Ollama supplies only optional experimental photo descriptions/face estimates; no small CV/embedding model is bundled.

## Verified functionality before this audit

The complete baseline backend suite ran: **73 passed, 0 failed, 2 manual checks ignored, no tests filtered**. It includes real RAW and AVIF fixture sweeps, built-in format fixtures, EXIF rotation, invalid/unsupported files, global exact/visual grouping, cache reuse, persistence, migrations, locks and isolated Bin recovery. The prior packaged-app smoke check preserved 638 photos and recommendations without producing Ollama results. Native visual automation was unavailable.

## Partial functionality

Face/eye/expression analysis is experimental optional Ollama output, not a verified lightweight CV detector. There are no persisted image embeddings. dHash similarity does not prove frame equivalence. Technical sharpness is a global scene-dependent heuristic, rather than face/subject focus. Combined technical/decision stages persist, but not every separate future CV stage. Completed analysis is reused after file verification; startup still rehashes available folders rather than resuming a per-stage durable queue exactly at the last file. HEIC/HEIF pixels are unavailable. Windows/Linux release execution has not been validated locally. Export, calibrated confidence and personalization are not implemented.

## Crash investigation

Two historical native reports (`tauri-app`, 15:24/15:25) record SIGABRT/abort with stripped frames; the original matching executable/output is unavailable, so their exact cause cannot be established from those records alone. Two historical test-process reports (14:01/14:02) show SIGSEGV in `avif::copy_picture_planes` → `Vec::extend_from_slice` → `_platform_memmove`. The current adapter still casts negative row offsets to unsigned pointer addition, which violates pointer arithmetic requirements and needs correction; that is not proof of the precise historical bad pointer.

A current parser failure was reproduced with two minimal malformed containers in `avif::malformed_container_tests`:

```
boxes: slice index starts at 16 but ends at 8
find_primary_nclx: range start index 4 out of range for slice of length 1
rust_begin_unwind → slice_index_fail → avif::boxes / avif::find_primary_nclx
```

Reproduction: `RUST_BACKTRACE=1 cargo test --offline --manifest-path src-tauri/Cargo.toml malformed_container_tests -- --nocapture`.

Both regressions **failed before the fix**. A wide box claimed a length shorter than its 16-byte header; a truncated FullBox metadata payload was sliced without checking its minimum size. The outer AVIF ingest path catches ordinary unwind panics, but structural validation should prevent them. Signal crashes/allocator aborts cannot be made safe by `catch_unwind`.

## Size audit

Measured disk allocation at inspection (binary units from `du`):

| Item | Disk size | Required distribution? |
|---|---:|---|
| macOS PhotoMind.app | ~23 MiB | Yes |
| App database/previews | ~25 MiB in preceding check | Runtime/user-dependent |
| Debug target artifacts | ~19 GiB | No |
| Release target artifacts | ~3.8 GiB | No |
| Entire development directory | ~23 GiB | No |
| Dev-only camera fixtures | ~325 MiB | No |
| node_modules | ~148 MiB | No |
| Ollama models | ~8.8 GiB | No |
| Required model weights | **0 bytes** | None currently bundled |

No files/models will be blindly deleted. App size is already far below the requested 500 MB target. Build caches are the development-space issue. Required Ollama download is zero. This does not mean the planned face/embedding functionality is complete.

## Ollama dependency map

| Installed model | Manifest size (decimal) | Current use | Lightweight replacement |
|---|---:|---|---|
| llava:latest | 4.73 GB | Optional vision descriptions and experimental face/eye estimates; legacy optional vision APIs | Specialized face/landmark/eye models can replace CV estimates; semantic descriptions may remain optional |
| qwen2.5:7b | 4.68 GB | Text-only; not eligible for photo vision; no core requirement | Not needed for current core |

Sizes are model manifests, rather than exact shared disk allocation. No models will be added in Phase 0; model/license selection belongs to the later phase. RAW's LGPL dependency redistribution obligations still require a release license review; the project has no audited public-release license yet.

## Stability risks

- Built-in decode paths lack an explicit common pixel/file/allocation budget. Preview regeneration currently decodes synchronously inside an async command, and API callers can bypass the scanner's two-job limit.
- RAW full-decode fallback can allocate large sensor/development buffers. AVIF has a 64 MP header cap, but custom plane copying uses unsafe pointers and needs checked dimensions/strides.
- Per-photo errors persist, but multiple session/pairing/checkpoint database failures are discarded, risking false completion or lost resume accounting.
- Current foreign-key/WAL pragmas are set via pool queries rather than explicitly for every connection. The WAL size pragma is not a hard live-WAL memory/disk limit.
- Startup `.expect` turns setup errors into fatal panic; logger `.init` can panic if a subscriber exists.
- Frontend startup probes optional Ollama even when the user only wants core scanning. Request generations are not consistently checked for all refresh results.
- Keeping a single Library image passes the entire browsable page as its keeper group, which can unprotect unrelated images. Bulk suggested removals lack the requested review/confirmation step.
- Global visual matching is quadratic; views load some global metadata eagerly. Thumbnail cache is bounded (500 entries), large-preview cache is bounded (8), but displayed browser-decoded images still consume memory. No memory leak has been proven.

## Proposed Phase 0 changes

1. `avif.rs`: validate box/header lengths and optional metadata slices; validate plane layout/depth/stride and use signed checked row offsets; preserve picture unref on errors. Add malformed-box and negative-stride regressions.
2. `ingest.rs`: bounded header sniffing; explicit file/pixel/allocation limits; process-wide decode permits across scanner/previews/optional AI; check RAW fallback sensor dimensions. Add wrong-extension and oversized-header tests.
3. `scanner.rs`: consistent session failure reporting/checkpoints and worker logs with photo/session/stage identifiers; expose the existing pipeline to an isolated native QA run, not a new product feature.
4. `db.rs`: configure connection-level integrity/journal settings consistently, avoid redundant startup truncation, and test all pool connections.
5. `lib.rs`: offload thumbnail regeneration, avoid fatal setup/logging expectations, and add an explicit isolated QA launch path so actual Tauri scanning/restart can be tested without user-library mutations.
6. `similarity.rs` / `decisions.rs`: reject malformed hashes and send uncertain heuristic quality differences to REVIEW; retain exact-byte suggestions.
7. `App.tsx` / `PhotoViewer.tsx`: optional-AI probes only after user opens Advanced AI; preserve unrelated keeper protection; confirmation for bulk Bin moves; refresh-generation checks. Preserve the existing design.
8. `.gitignore` / README / Phase 0 report: distinguish distribution size from caches, prevent user databases/models/build artifacts being committed, and document measured tests plus untested areas.

Stop after Phase 0. Do not add models, embeddings, export, new profiles or roadmap features.
