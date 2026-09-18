# Refreshing saved results

Updated to the user's fresh-launch preference: normal reopening clears the prior active scan library, scan decisions, groups, folder history and generated previews not needed by Bin. Original files and Bin recovery records survive. Previously scanned folders are not automatically reloaded. The prior persistent-launch behavior documented in the Phase 0 report is superseded.

- Startup reconciles system Trash and resets the active scan library before the UI loads. Only explicitly supplied isolated developer QA folders auto-scan.
- Returning to the app checks the active library and Bin. Checks also run every 30 seconds while visible, or every five seconds on Bin.
- When reconciliation removes records, all displayed results refresh, thumbnail/large-preview caches clear, and an open comparison closes so it cannot display removed photos.
- Completed scans remove generated previews that no remaining library/Bin record references.
- Refresh skips active scans/file operations and retries on the next check. Disconnected drives and permission errors do not prove deletion, so their records are preserved.

During the same session, scans rehash originals and reuse valid analysis; periodic checks identify confirmed deletions. Across launches, active scan results are reset, including active keeper choices. Bin recovery choices/previews remain. Select a folder again to generate fresh results.

Verification for this change: frontend production build and Rust check passed; `refresh_prunes_deleted_results_and_previews_but_retains_bin_and_keepers`, `gc_removes_only_orphaned_thumbnails_and_keeps_the_library`, and the cleanup-planner regression passed. Native visual checks remain unavailable without macOS Computer Use permissions.

Historical live-reconciliation test: a generated original was deleted externally and reopening removed its result/preview while retaining a keeper. That predates the fresh-launch preference.

Current packaged fresh-launch test (`.qa/native_refresh.py`) passed: two generated photos were scanned; one was moved to an isolated test Bin; reopening without a QA scan folder cleared every active photo, scan session, recommendation and moment, did not reload the previous folder, and removed unused previews. The original remaining photo, Bin file and Bin preview survived. Evidence: `/var/folders/yk/qmv6qyvj2kl8fcf7q77ttcxr0000gn/T/photomind-refresh-c5uk6apj/`.

An initial native run exposed an alias bug: `/var` and `/private/var` referred to the same directory but preview retention compared literal paths. Reset and garbage collection now compare canonical paths; both regressions and the native rerun passed. Release packaging succeeded after fetching the runtime required by concurrent face-analysis changes in the project. The latest bundle is approximately 41 MiB; the earlier Phase 0 bundle was 23 MiB.
