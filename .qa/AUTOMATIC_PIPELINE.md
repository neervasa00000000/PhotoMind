# Automatic local culling: stabilization report

## Root cause of the reported comparison error

The screenshot reports `AI returned unknown or repeated photo IDs`. The error is emitted by `ollama::validate_comparison` when a generative model returns an ID outside the supplied snapshot, or assigns the same ID to multiple categories. The screenshot is an application error, rather than evidence of a native process crash. The original raw model response was not available to replay.

Previously core group comparison sent up to six full photo previews to Ollama, allowed a 300-second request, and asked the model to reproduce UUIDs. Automatic per-photo analysis also sent every photo to a large model. That created long waits and unreliable identifiers.

## Implemented behavior

- Folder selection automatically starts bounded background decoding, EXIF extraction, preview generation, SHA-256/dHash, sharpness/exposure/clipping analysis, grouping and local recommendations.
- The scanner publishes previews progressively. It periodically saves group decisions while the remaining files continue processing; final groups are rebuilt at completion.
- Core group comparison uses database IDs directly and never invokes a generative model. The existing `analyze_moment` command also uses the local engine for compatibility.
- Library cards show KEEP / REVIEW / REJECT. Detailed reasons appear in Moments. Compare large opens the existing side-by-side viewer with keeper selection and recoverable Bin actions.
- Decisions are suggestions. No scan/recommendation path deletes or moves a file.
- Unique frames are never automatically rejected for low sharpness. Unreadable frames are REVIEW. User keep choices/overrides take priority.
- Exact byte copies in a moment have a stable keeper. A rejection for poor focus requires a close dHash alternative with substantially stronger measured sharpness. Other visual alternatives remain REVIEW. Separate global duplicate detection still covers all indexed folders and filename numbers.
- AppleDouble `._` sidecars are excluded. Decode failures retain an error state, are counted separately, and are recorded per scan with a structured native error log.
- Results persist across launches. Startup reconciles missing originals and system Trash, then verifies previously scanned available folders in the background. Unmounted macOS volumes are not treated as deletion. Cancelled scans stay cancelled.
- Every visited original is rehashed to validate content. Unchanged images reuse their derived analysis and previews; missing previews are regenerated. Changed images invalidate old recommendations/AI results.
- The current combined decode/metadata/technical stage persists `technical_ready`, followed by `decision_ready`; unavailable previews remain `error`. There are not seven separately checkpointed CV stages yet.
- Moments now load with three batched queries instead of two extra queries per group. Progressive refresh avoids recomputing global visual duplicate clusters after every batch.

## Verification

- `cargo check` passed.
- Backend suite: 67 passed, 0 failed, 2 manual tests ignored, 2 existing long decoder fixture sweeps filtered (RAW manifest and AVIF fixture sweep). Individual RAW/AVIF and scan tests still ran.
- Frontend TypeScript/Vite build passed.
- `.qa/cleanup.test.ts` passed.
- New mixed-library regression: five valid files, two invalid files, one excluded AppleDouble sidecar. Seven persisted recommendations; zero Ollama result rows. Reopen preserved the override and did not rewrite an unchanged thumbnail. External deletion reconciled correctly.
- The tiny mixed-library decision pass measured approximately 0.75 milliseconds locally. This measures recommendation computation only; it is not a large-library scan benchmark.

## Remaining limitations

There is currently no bundled lightweight face/landmark/eye/expression model or image embedding model. The fast engine does not claim to detect closed eyes, smile quality or face sharpness. Existing experimental face/expression estimates remain available through optional per-photo Ollama functionality in Advanced AI. They do not gate scanning or core decisions.

HEIC is currently metadata-only when its pixel decoder is unavailable. RAW/AVIF decoding can still dominate scan time. dHash is a coarse visual heuristic, so recommendations require human review; its distance does not prove two frames are equivalent. Technical scores are scene-dependent and confidence values are rule strengths, not calibrated probabilities.

This work builds the local macOS app package. It does not publish GitHub releases, validate Windows/Linux installers, implement export, or claim native visual end-to-end QA when computer-use permissions are unavailable.

## Packaged macOS smoke check

The optimized macOS package built successfully and was launched after gracefully quitting the prior app. Read-only inspection of the real app database showed 638 retained photos, all at `decision_ready`, with 441 KEEP and 197 REVIEW recommendations and zero Ollama result rows. The saved external-drive scan folder was unavailable, so startup correctly skipped automatic verification of that folder. The mixed-file processing regression was run through backend pipeline stages, rather than through native UI folder selection.

The initial Launch Services restart did not leave a process running. A subsequent direct launch of the packaged executable remained running under observation with no startup error output; the persisted library stayed intact. Native visual inspection was not available.
