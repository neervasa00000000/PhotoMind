# Eye Detection Safety Flag

## Summary

Eye detection has been implemented and is collecting data, but is **disabled for ranking decisions** until empirical validation confirms accuracy.

## Changes Made

### 1. Feature Flag Added
**Location:** `src-tauri/src/decisions.rs`

```rust
/// SAFETY: Eye detection feature flag.
/// Set to `false` until empirical validation confirms accuracy ≥85% and false-closed rate <5%.
/// When false: eye detection runs and stores results, but does NOT affect ranking/recommendations.
/// When true: eye state influences BEST-OF-GROUP ranking (after validation).
const ENABLE_EYE_BASED_RANKING: bool = false;
```

### 2. Neutral Scoring When Disabled
**Location:** `src-tauri/src/decisions.rs` in `people_score()` function

When `ENABLE_EYE_BASED_RANKING = false`:
- Eye detection still runs on all photos
- Results are stored in database
- Results are displayed in UI
- Eye score is set to **neutral 0.5** (middle value)
- This means eyes don't affect relative ranking between photos

When `ENABLE_EYE_BASED_RANKING = true` (after validation):
- Eye state affects ranking normally
- OPEN eyes score 1.0 (positive)
- CLOSED eyes score 0.0 (negative)
- UNCERTAIN eyes score 0.45 (slight negative, but less than closed)

### 3. Documentation Updated
**Location:** `src-tauri/src/decision_weights.rs`

Added comments explaining:
- Eye ranking is gated
- Weight contributes constant term when disabled
- Doesn't affect relative ranking until validated

## Current Behavior

### What Runs
✅ Face detection (YuNet)
✅ Eye landmark extraction
✅ Eye state classification (OPEN/CLOSED/UNCERTAIN)
✅ Confidence scoring
✅ Database storage
✅ UI display
✅ Data collection for validation

### What's Disabled
❌ Eye state affecting BEST-OF-GROUP ranking
❌ Eye state affecting KEEP/REVIEW/REMOVE decisions
❌ CLOSED eyes penalizing photo scores
❌ UNCERTAIN eyes penalizing photo scores

### Safety Guarantees
1. UNCERTAIN never receives negative penalty (neutral 0.5)
2. Low-confidence CLOSED never receives negative penalty (neutral 0.5)
3. Eye state cannot cause REMOVE
4. Eye state does not materially contribute to BEST-OF-GROUP
5. All eye analysis values preserved for validation
6. Eye thresholds unchanged (collecting baseline)

## How to Enable After Validation

### Prerequisites
1. Complete empirical validation on 50-100 labeled faces
2. Confirm confident accuracy ≥ 85%
3. Confirm false-closed rate < 5%
4. Review validation report
5. Make informed decision about enabling

### If Validation Shows Good Results

**File:** `src-tauri/src/decisions.rs`

Change:
```rust
const ENABLE_EYE_BASED_RANKING: bool = false;
```

To:
```rust
const ENABLE_EYE_BASED_RANKING: bool = true;
```

Then rebuild:
```bash
cd src-tauri
cargo test  # Verify tests still pass
npm run tauri build  # Build production app
```

### If Validation Shows Poor Results

**Do NOT enable the flag.**

Instead:
1. Keep flag as `false`
2. Decide on one of:
   - **Restrict**: Tighten conditions for confident classification (more UNCERTAIN)
   - **Replace**: Propose specific eye/landmark model with details
   - **Disable**: Remove eye-based scoring entirely

## Validation Command

```bash
# Label privately owned photos (local UI, gitignored output)
./scripts/label-eyes.sh

# Then compare against ground truth — do not tune thresholds on Baseline V1
cd src-tauri
cargo run --bin validate_eyes --release -- \
  ../eye-validation/photos \
  --ground-truth ../eye-validation/ground_truth.json
```

See `docs/EYE_VALIDATION.md` for the full validation guide.
The labeler is development-only and is not packaged into PhotoMind.


## Test Results

All existing tests pass: ✅ 81 passed; 0 failed

The safety flag does not break any existing functionality.
