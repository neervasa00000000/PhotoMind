# CURRENT EYE DETECTION VALIDATION - PRELIMINARY ANALYSIS

## Status: VALIDATION TOOL READY - EMPIRICAL TESTING REQUIRED

This document analyzes the CURRENT eye detection implementation theoretically and provides the validation framework. **Real-world accuracy metrics require running the validation tool on manually-labeled photographs.**

---

## 1. Current Algorithm - Exact Implementation

### Inputs
- YuNet face detection provides 5 landmarks per face:
  - Right eye center: `(x, y)` coordinates
  - Left eye center: `(x, y)` coordinates  
  - Nose tip
  - Right mouth corner
  - Left mouth corner

### Eye Analysis Process

For each eye landmark:

1. **Calculate patch size**:
   ```rust
   radius = (face_scale * 0.08).max(4.0)  // face_scale = min(bbox.width, bbox.height)
   ```
   - 80px face → 6.4px radius → ~13×13px patch
   - 200px face → 16px radius → ~32×32px patch
   - Minimum 4px radius for very small faces

2. **Extract patch**: Square region centered on eye landmark

3. **Measure vertical edge energy**:
   ```rust
   for each pixel in patch:
       vertical_edges += |pixel_above - pixel_below|
   openness_score = (vertical_edges / num_samples / 64.0).clamp(0.0, 1.0)
   ```
   The constant `64.0` is an arbitrary normalization.

4. **Classify**:
   - `score >= 0.52` → **OPEN**
   - `score <= 0.38` → **CLOSED**
   - `0.38 < score < 0.52` → **UNCERTAIN**

5. **Confidence**:
   - For OPEN: `((score - 0.52) / 0.35 + 0.55) * visibility_conf`
   - For CLOSED: `((0.38 - score) / 0.35 + 0.55) * visibility_conf`
   - `visibility_conf = (face_scale / 80.0).clamp(0.0, 1.0)`
   - Faces < 24px → automatic UNCERTAIN with 0.2 confidence

---

## 2. Can This Algorithm Truly Detect Eye Openness?

### Answer: **PARTIALLY** - Weak Signal, Requires Validation

### Theoretical Basis

The algorithm assumes:
- **Open eyes** have more vertical edges (eyelid→sclera, iris→sclera transitions)
- **Closed eyes** have fewer vertical edges (smooth eyelid skin)

### Fundamental Problems

1. **Landmark Precision**
   - YuNet provides eye **center**, not eyelid boundaries
   - The patch captures the eye region but is not aligned to eyelid edges
   - Small patch size (8% of face scale) means limited context

2. **Ambiguous Signals**
   - **Eyebrows** create strong vertical edges (often in the patch)
   - **Eyelashes** create vertical edges regardless of eye state
   - **Shadows** from brow/nose create edges
   - **Glasses frames** create strong edges
   - **Glare/reflections** on glasses create edges

3. **Geometric Blindness**
   - No understanding of eyelid **shape** or **geometry**
   - No distinction between upper/lower eyelid position
   - No measurement of eye opening height
   - Cannot distinguish squinting from closing

4. **Squinting Problem**
   - People naturally narrow eyes when smiling
   - Narrowed eyes still have iris→sclera edges
   - May classify happy squints as "closed" (dangerous false-closed)

5. **Glasses Problem**
   - Clear glasses: reflections and frame edges
   - Dark/sunglasses: blocks view of eye entirely
   - No explicit detection or handling

6. **Size Limitations**
   - 13×13px patch for 80px face is extremely small
   - Single-pixel misalignment significantly affects measurement
   - Minimum useful face size unknown (needs empirical testing)

### Why It Might Work Anyway

1. **Statistical correlation**: Even weak signals can work if consistent
2. **Relative measurement**: Comparing photos of same person might work
3. **Conservative thresholds**: 0.38-0.52 dead zone allows UNCERTAIN
4. **Face size gating**: Very small faces → automatic UNCERTAIN

### Critical Unknown

**Does the vertical edge density difference between open/closed eyes exceed the noise from eyebrows, lashes, shadows, and glasses?**

**This requires empirical testing on real photographs.**

---

## 3. Validation Tool Created

### Location
`src-tauri/src/bin/validate_eyes.rs`

### Build
```bash
cd src-tauri
cargo build --bin validate_eyes --release
```

### Usage

**Without ground truth** (exploration):
```bash
cargo run --bin validate_eyes --release -- /path/to/photos
```

**With ground truth** (accuracy measurement):
```bash
cargo run --bin validate_eyes --release -- /path/to/photos --ground-truth ground_truth.json
```

### Outputs

For each face:
- Filename and face index
- Detection confidence
- Face bounding box and sharpness
- Per eye:
  - Predicted state (OPEN/CLOSED/UNCERTAIN)
  - Openness score (raw measurement)
  - Confidence
  - Visibility confidence
  - Region sharpness
  - Ground truth comparison if provided

### Metrics Calculated

When ground truth is provided:
- **Confusion matrix**: true_open→pred_open, true_open→pred_closed, etc.
- **Confident accuracy**: % correct among non-UNCERTAIN predictions
- **Coverage**: % of eyes classified confidently
- **False-closed rate**: % of open eyes predicted closed (most dangerous)
- **False-open rate**: % of closed eyes predicted open
- **Uncertain rate**: % classified as UNCERTAIN (not counted as wrong)

---

## 4. Validation Dataset Requirements

### Minimum Sample Size
**50-100 manually labeled faces** across diverse conditions

### Required Test Cases

**Easy (baseline):**
- [ ] 10+ frontal faces, eyes clearly open
- [ ] 10+ frontal faces, eyes clearly closed  
- [ ] 5+ winks (one eye closed)

**Realistic (production conditions):**
- [ ] Small faces (< 80px)
- [ ] Large faces (> 200px)
- [ ] Multiple people per photo
- [ ] Clear glasses
- [ ] Side angles (15°, 30°, 45°)
- [ ] Looking up/down/sideways
- [ ] Various lighting (bright, dark, backlit)
- [ ] Slight blur
- [ ] Strong blur

**Failure modes (must test):**
- [ ] Smiling with squinted/narrowed eyes
- [ ] Sunglasses / dark glasses
- [ ] One eye occluded (hair, hand, etc.)
- [ ] Group photos with small distant faces
- [ ] Glasses with reflections
- [ ] Partial faces (edge-cropped)

### Ground Truth Format

See `docs/ground_truth_template.json`

```json
{
  "photo.jpg": [
    {
      "face": 0,
      "left_eye": "OPEN",
      "right_eye": "CLOSED"
    }
  ]
}
```

Values: `"OPEN"`, `"CLOSED"`, `"IGNORE"`

---

## 5. Expression (Smile) Heuristic

### Current Implementation

Uses two mouth corner landmarks:
```rust
mouth_width = distance(mouth_left, mouth_right)
width_norm = mouth_width / bbox_width
mouth_y_center = (mouth_left.y + mouth_right.y) / 2
corner_lift = (nose.y - mouth_y_center) / bbox_height
smile_score = width_norm * 0.65 + (corner_lift + 0.15) * 0.35
```

### Assessment: **LIMITED BUT USEFUL FOR RELATIVE COMPARISON**

**Theory**:
- Smiling widens mouth (increases width)
- Smiling lifts mouth corners (decreases y-coordinate)

**Limitations**:
- Only 2 mouth points (no lip shape)
- No teeth visibility
- Confuses neutral wide mouths with smiles
- Confuses talking/yawning with smiling
- Different people have different neutral mouth widths

**Expected Use**:
- Comparing multiple photos of **same person** (relative ranking)
- NOT absolute smile detection across different people
- NOT distinguishing genuine vs forced smiles

**Validation**: Should be tested alongside eye detection, but lower priority.

---

## 6. Current Safety in Recommendation Engine

### Question: Can eye detection currently cause REMOVE?

**Checking implementation...**

From `src-tauri/src/decisions.rs`:

```rust
let mut people_score = 0.0;
// Score calculation includes eye state
// But removal requires multiple negative factors
```

**CURRENT STATUS**: Eye detection influences `people_score` which contributes to total photo score. However:
- Removal decisions require confidence thresholds
- Multiple factors are combined
- Low-confidence eye state should not independently trigger REMOVE

**RECOMMENDATION**: 
- Eye state should influence **BEST-OF-GROUP** ranking (relative)
- Eye state can suggest **REVIEW** flag
- Eye state should **NOT** independently cause **REMOVE**
- Especially: `UNCERTAIN` or low-confidence `CLOSED` → never cause removal

---

## 7. Left/Right Orientation

### YuNet Landmark Mapping

From `yunet.rs` line 180-181:
```rust
right_eye: (data[base + 4] * scale_x, data[base + 5] * scale_y),
left_eye: (data[base + 6] * scale_x, data[base + 7] * scale_y),
```

### Database Storage

Both eyes stored separately as `left_eye` and `right_eye` in `photo_faces` table.

### UI Display

Frontend receives left/right separately.

### Verification Need

**Wink tests required** to confirm:
- YuNet's "left" = subject's left (not viewer's left)
- Database storage matches YuNet
- UI display matches database
- No swaps at any layer

Test with photos where you KNOW which eye is closed.

---

## 8. Multi-Face Mapping

From code inspection:
- Each face gets separate `DetectedFace` struct
- Eye analysis performed independently per face
- No landmark sharing between faces

**Should be correct**, but verify with group photos during validation.

---

## 9. Rotation Handling

Photos are loaded via `ingest::load_normalized()` which applies EXIF orientation before analysis.

**Should be correct**, but test with:
- Portrait photos (EXIF orientation 6)
- Physically rotated images
- Different EXIF orientations

---

## 10. VALIDATION REQUIRED BEFORE CONCLUSIONS

### I CANNOT report:
- Actual confident accuracy: **UNKNOWN - needs empirical testing**
- False-closed rate: **UNKNOWN - needs empirical testing**
- Coverage: **UNKNOWN - needs empirical testing**
- Performance on glasses: **UNKNOWN - needs empirical testing**
- Performance on squints: **UNKNOWN - needs empirical testing**
- Performance on small faces: **UNKNOWN - needs empirical testing**

### I CAN report:

**Algorithm**: Measures vertical edge density in small patch around eye center

**Theoretical assessment**: 
- **Weak signal** susceptible to noise (eyebrows, lashes, shadows, glasses)
- **May work** if signal-to-noise is adequate in practice
- **Uncertain** without empirical validation

**Validation tool**: Ready to use

**Dataset needed**: 50-100 labeled faces

**Estimated time to validate**: 2-4 hours (collect photos, label, run tool, analyze)

---

## 11. RECOMMENDATION PENDING VALIDATION

### Cannot recommend until empirical results available.

### Possible outcomes:

**A. If validation shows ≥85% confident accuracy, <5% false-closed**:
→ Keep current ~0 MB heuristic
→ Tune thresholds if needed
→ Document limitations

**B. If works well only on large frontal faces**:
→ Restrict to high-quality scenarios
→ Stricter face size minimum
→ More aggressive UNCERTAIN for angles/glasses
→ Keep 0 MB footprint

**C. If <75% confident accuracy or high false-closed**:
→ Disable eye weighting in recommendations
→ Propose specific small landmark/eye model
→ Report model name, size, license
→ STOP and await approval

---

## 12. NEXT STEPS

1. ✅ Validation tool created
2. ✅ Documentation written
3. ⏳ **Collect 50-100 diverse test photos** (user task - DO NOT commit)
4. ⏳ **Create manual ground truth labels** (user task)
5. ⏳ **Run validation tool**
6. ⏳ **Analyze results**
7. ⏳ **Make evidence-based recommendation**
8. ⏳ **If needed: propose specific model with details**
9. ⏳ **Await approval before installing any new model**

---

## 13. STOP - DO NOT ADD MODELS YET

I have:
- ✅ Analyzed the current implementation honestly
- ✅ Identified fundamental theoretical limitations
- ✅ Created validation framework
- ✅ Documented requirements

I have NOT:
- ❌ Added any new models
- ❌ Changed the architecture
- ❌ Made unvalidated claims about accuracy
- ❌ Confused "code works" with "algorithm works"

**The validation tool is ready. Real-world testing is required before any architectural decisions.**
