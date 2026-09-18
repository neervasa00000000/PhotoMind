# Eye Detection Validation Guide

## Purpose

This document explains how to validate PhotoMind's eye detection accuracy using real photographs.

## Current Algorithm

PhotoMind uses YuNet for face detection, which provides 5 facial landmarks:
- Right eye center
- Left eye center
- Nose tip
- Right mouth corner
- Left mouth corner

For eye state detection, the current algorithm:

1. **Extracts** a small square patch around each eye center
   - Patch radius = `face_scale * 0.08` pixels (minimum 4px)
   - For an 80px face: ~6.4px radius → 13x13px patch
   
2. **Measures** vertical edge energy
   - Sums `|pixel_above - pixel_below|` across all pixels
   - Normalizes by dividing by samples and then by 64.0
   
3. **Classifies** based on thresholds:
   - **OPEN**: score >= 0.52
   - **CLOSED**: score <= 0.38
   - **UNCERTAIN**: between 0.38 and 0.52

## Validation Tool

### Build

```bash
cd src-tauri
cargo build --bin validate_eyes --release
```

### Run Without Ground Truth

Analyze photos and see predictions:

```bash
cargo run --bin validate_eyes --release -- /path/to/test/photos
```

### Run With Ground Truth

Compare predictions against manual labels:

```bash
cargo run --bin validate_eyes --release -- /path/to/test/photos --ground-truth ground_truth.json
```

## Creating Ground Truth

Use the **development-only local labeler**. Do not edit JSON by hand unless you have to. PhotoMind's OPEN/CLOSED prediction is hidden while you label.

### 1. Put legally usable photographs in

```
eye-validation/photos/
```

That folder is gitignored. Never commit photos, crops, `ground_truth.json`, or failure reviews.

### 2. Start the labeler (one command)

```bash
./scripts/label-eyes.sh
```

or:

```bash
cd src-tauri
cargo run --bin label_eyes --release
```

The UI shows the photograph, face box, Face N of M, a zoomed face crop, and **SUBJECT'S LEFT EYE** / **SUBJECT'S RIGHT EYE** crops. Click OPEN / CLOSED / IGNORE for each eye, or use:

- Subject's left: `1` OPEN · `2` CLOSED · `3` IGNORE
- Subject's right: `7` OPEN · `8` CLOSED · `9` IGNORE
- `N` / `→` next · `B` / `←` back · `S` save · `X` skip photo

Labels auto-save to `eye-validation/ground_truth.json`. Reopening resumes at the first unlabeled face.

Target: **at least 100 usable labelled eyes** (OPEN+CLOSED, not IGNORE); 200+ preferred. Quality and diversity matter more than the count.

Left/right are the **subject's anatomical eyes** (YuNet left/right landmarks). For a person facing the camera, the subject's right eye is on the left side of the photo.

### 3. Composition checklist

Cover these if you can. The labeler shows the list. Do **not** auto-fabricate category tags.

- NORMAL: frontal, open eyes, closed eyes
- WINK: one open / one closed
- EXPRESSION: smiling, strong smiling/squinting
- EYEWEAR: glasses, reflections, sunglasses
- POSE: slight angle, side angle, looking down, looking sideways
- QUALITY: sharp, slightly blurry, dark, bright
- SIZE: large / medium / small face
- GROUPS: 2 people, several people, mixed open/closed eyes

### 4. Run validation

```bash
cd src-tauri
cargo run --bin validate_eyes --release -- \
  ../eye-validation/photos \
  --ground-truth ../eye-validation/ground_truth.json
```

This writes `eye-validation/results/baseline_v1.txt` and a local `eye-validation/failure-review.html` for OPEN→CLOSED and CLOSED→OPEN mistakes. Original photos are not modified.

Until that report exists, the JSON format (0-based face index, subject's left/right) is:

```json
{
  "photo001.jpg": [
    {
      "face": 0,
      "left_eye": "OPEN",
      "right_eye": "OPEN"
    }
  ]
}
```

**Values:**
- `"OPEN"` — Eye is clearly open
- `"CLOSED"` — Eye is clearly closed
- `"IGNORE"` — Cannot determine (sunglasses, occluded, etc.)

**Important:**
- Face indices start at 0 and follow detection order (same as `validate_eyes`)
- Left/right are from the **subject's** perspective
- Use `"IGNORE"` when YOU cannot determine the state
- DO NOT label based on algorithm output — use YOUR judgment
- The labeler never preselects PhotoMind's prediction

## Interpreting Results

### Key Metrics

**Confident Accuracy**: Percentage correct among confident (non-UNCERTAIN) predictions
- Should be > 85% for "reliable"
- Below 75% indicates poor discrimination

**Coverage**: Percentage of eyes classified confidently (not UNCERTAIN)
- High coverage (>80%) = algorithm is confident
- Low coverage (<50%) = algorithm knows its limits (good!)

**False-Closed Rate**: % of truly OPEN eyes predicted CLOSED
- **Most dangerous** for PhotoMind
- Should be < 5%
- False-closed can cause good photos to be unfairly penalized

**False-Open Rate**: % of truly CLOSED eyes predicted OPEN
- Less dangerous (might keep a blink photo, but not remove a good one)
- Should still be < 10%

**Uncertain Rate**: % classified as UNCERTAIN
- Not counted as "wrong"
- UNCERTAIN is intentionally allowed for difficult cases

### Example Good Result

```
Total labelled eyes: 120
Confident predictions: 85 (70.8%)
Uncertain predictions: 35 (29.2%)

Confident accuracy: 89.4% (76/85)
False-closed rate: 3.3%
False-open rate: 8.3%
```

This shows:
- Algorithm is selective (29% uncertain)
- High accuracy when confident
- Low false-closed rate (safe)

### Example Poor Result

```
Total labelled eyes: 120
Confident predictions: 110 (91.7%)
Uncertain predictions: 10 (8.3%)

Confident accuracy: 62.7% (69/110)
False-closed rate: 18.2%
False-open rate: 19.1%
```

This shows:
- Algorithm is overconfident
- Poor discrimination
- High false-closed rate (dangerous)

## Validation Outcomes

After validation, choose one:

### A. CURRENT HEURISTIC IS GOOD ENOUGH
- Confident accuracy > 85%
- False-closed rate < 5%
- Coverage reasonable for face sizes > 60px

**Action**: Keep current ~0 MB eye detection, tune thresholds if needed

### B. USEFUL BUT LIMITED
- Works well on large frontal faces
- Fails on small faces, angles, glasses, squints

**Action**: Restrict to high-quality scenarios only:
- Minimum face size threshold
- Maximum pose angle
- Minimum patch quality
- Everything else → UNCERTAIN

### C. NOT RELIABLE
- Confident accuracy < 75%
- High false-closed rate
- Poor discrimination even on easy cases

**Action**: 
1. Disable eye-based recommendation weighting
2. Recommend a small dedicated eye/landmark model
3. Report model details and await approval before installing

## Left/Right Verification

The tool should help verify:
- YuNet's left/right landmark mapping
- Our storage representation
- UI display consistency

Test with wink photos specifically to ensure left/right isn't swapped.

## Recommendations Safety

Until validation is complete:
- Eye state should be **advisory only**
- Can influence BEST-OF-GROUP ranking
- Can suggest REVIEW
- **Must NOT** independently cause REMOVE
- **Never** remove based on UNCERTAIN
- **Never** remove based on low-confidence CLOSED

## Next Steps

1. Build validation tool
2. Collect diverse test photos (50-100 faces)
3. Create manual ground truth
4. Run validation
5. Report results
6. Decide on action (A, B, or C)
7. If C: propose specific model with size/license details
8. DO NOT add model without approval
