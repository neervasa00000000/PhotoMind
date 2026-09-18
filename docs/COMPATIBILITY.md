# Source-Format Compatibility

Status of every image family the app indexes, what happens to it at each stage,
and the licensing decisions behind each capability. Phase A is **backend-only**:
no UI changes ship with it; the format-handling engine and its verification do.

## Capability model

Each detected format carries a fixed capability set that the rest of the app
must respect:

| Capability | Meaning |
|---|---|
| `detect`       | bounded magic-byte inspection; compatible RAW/HEIF extension hints |
| `metadata`     | EXIF capture time / make / model / orientation / GPS readable |
| `preview`      | a JPEG preview can be produced for analysis and thumbnails |
| `full_decode`  | the photo's pixels can be decoded |
| `analyze`      | sharpness / exposure / contrast / clipping metrics are computable |

A photo whose `full_decode` is false is still **indexed** (existence, size,
hash, metadata) but produces no thumbnail and no per-photo metrics; the scan
records an explicit reason in `scan_errors`. Ordinary per-file decoder failures
are non-fatal; this does not guarantee protection from every native signal or allocation failure.

## Supported source formats

| Family | Extensions | detect | metadata | preview | full_decode | analyzed |
|---|---|---|---|---|---|---|
| Photos (builtin `image` crate) | `.jpg .jpeg`, `.png`, `.webp`, `.gif`, `.bmp`, `.ico`, `.tif .tiff`, `.qoi`, `.dds`, `.hdr`, `.exr` | yes | yes\* | yes | yes | yes |
| RAW via `rawler` (LGPL-2.1) | 23 camera formats | yes | yes | yes\* | yes | yes |
| AVIF (`avif`, `avis`) | `.avif` | yes | yes\* | yes | yes | yes |
| HEIF / HEIC | `.heif`, `.heic` | yes | yes\* | — | — | — |

\* `metadata`: EXIF is read only when the container carries an Exif item
(kamadak-exif parses JPEG/EXIF spans, TIFF-based raws, and ISO-BMFF
HEIC/HEIF/AVIF boxes). RAW `preview` prefers the camera's embedded JPEG
preview; a full demosaic is the fallback only when no preview exists.

### RAW formats (via `rawler` 0.8, embedded-preview-first)

`arw arif cr2 cr3 crw dcr dcs dng erf iiq kdc mef mos mrw nef nkd nrw orf pef
qtk raf rw2 srw tfr x3f` — 23 formats / 15 brands.

Verified against real camera files this session:

| File | Camera | Outcome |
|---|---|---|
| `fixtures/sample.dng` | Canon EOS 350D DIGITAL | full RAW decode → thumbnail + dims + metrics |
| `fixtures/sample.nef` | Nikon D3 | embedded-preview decode → thumbnail + dims + metrics |

### Phase B — per-camera fixture campaign

Nine more real cameras (one per brand/family) were pulled from the
pixls.us-derived rawdb corpus (`rawdb.dnglab.org`, the same samples rawler's
own test suite decodes) and every one is exercised end-to-end by the
manifest-driven test `raw_manifest_fixtures_decode_across_camera_brands`
(`fixtures/raw_manifest.tsv`):

| File | Make | Model | Family | Sensor width×height |
|---|---|---|---|---|
| `sample-canon-5d3.cr2` | Canon | EOS 5D Mark III | CR2 | 5920×3950 |
| `sample-canon-m200.cr3` | Canon | EOS M200 | CR3 | 6288×4056 |
| `sample-sony-7rm3a.arw` | Sony | ILCE-7RM3A | ARW | 8000×5320 |
| `sample-fuji-xt1.raf` | Fujifilm | X-T1 | RAF | 4992×3296 |
| `sample-olympus-em5.orf` | Olympus | E-M5 | ORF | 4640×3472 |
| `sample-panasonic-g9.rw2` | Panasonic | DC-G9 | RW2 | 5264×3904 |
| `sample-pentax-k3.pef` | Pentax | K-3 | PEF | 6080×4032 |
| `sample-samsung-nx100.srw` | Samsung | NX100 | SRW | 4704×3124 |
| `sample-nikon-d850.nef` | Nikon | D850 | NEF | 8288×5520 |

All nine decode through the production path (`detect_format` →
`load_normalized_info`) to a correctly-sized photo and re-encode to JPEG.
Result: **10 cameras across 9 families** (CR2, CR3, ARW, RAF, ORF, RW2, PEF,
SRW, NEF, plus Canon-350D DNG) are verified, not just advertised.

Adding a camera: drop the RAW into `src-tauri/fixtures/`, add one line to
`raw_manifest.tsv` with its sensor size, and the manifest test verifies (and
locks in) its decode.

Any other RAW family is claimed as *indexable* only — per-camera proof
requires its own fixture. RAW decode runs inside `std::panic::catch_unwind`
and release builds unwind ordinary panics. Signal crashes and allocator aborts
cannot be caught this way. Phase 0 adds source/dimension limits and two global
decode permits; untested camera branches remain a risk.

### AVIF — pure-Rust AV1 decode (Phase B)

AVIF still images are decoded end-to-end with **no C compiler anywhere in the
build**: `avif-parse` (MPL-2.0) parses the HEIF container, `rav1d` 1.1.0
(BSD-2-Clause — a pure-Rust port of dav1d, used via its dav1d-compatible C
ABI) decodes the AV1 bitstream, and the app converts planar YUV → RGB.

Verified against real AVIF files (the `hato` photo from
`link-u/avif-sample-images`, CC-BY-SA 4.0 — see `fixtures/README.md`), one
file per claimed decode profile, all 3082×2048:

| File | Profile exercised |
|---|---|
| `sample-avif-8bpc-yuv420.avif` | 8-bit, 4:2:0 (the standard AVIF layout) |
| `sample-avif-10bpc-yuv420.avif` | 10-bit, 4:2:0 (HDR-capable depths) |
| `sample-avif-8bpc-yuv422.avif` | 8-bit, 4:2:2 |
| `sample-avif-8bpc-mono.avif` | 8-bit monochrome, 4:0:0 |

Every file is exercised by `ingest::tests::avif_fixtures_decode_through_the_av1_backend`
through the production path: detect → decode → full-size photo → JPEG export.

Color is driven by the container's `colr`/`nclx` box: the module walks
`meta → pitm/iprp → ipco/ipma` to find the primary item's colour primaries,
transfer characteristics (`nclx` full-range flag included), and matrix
coefficients, then applies that matrix and the matching inverse EOTF
(BT.601/709/2020, sRGB, PQ, HLG) before encoding 8-bit sRGB. When no `colr` is
present (e.g. ravif's own output) the same code mappings are applied to the
**AV1 sequence header's** colour description, read straight off the decoded
picture; the layout defaults remain the last resort. Sub-sampled chroma is
upsampled with a separable bilinear filter whose phase follows the bitstream's
`chroma_sample_position` (co-sited vs vertically centered). Verified by
`avif.rs` unit tests over hand-built box trees, direct pixel-level color-math
checks for each matrix/transfer, a siting/interpolation test per chroma phase,
and a live encode→decode round trip against ravif/rav1e output
(`encoded_avif_roundtrips_through_known_encoding`) that now asserts both the
explicit and the sequence-header fallback paths recover the source bands.

Prototype scope (documented, honest): the alpha item is dropped (opaque RGB
out); HDR (PQ/HLG) input is inverse-EOTF'd then clamped to sRGB with no tone
mapping, so highlights can clip. Good enough for previews, duplicates, and
thumbnails, not archival color.

### Not decodable on this build (honest, documented)

HEIC / HEIF (HEVC-in-HEIF) pixels are **not** decoded: no permissive,
pure-Rust HEVC decoder exists that could be vendored. The only general
library previously considered (`heic`, imazen) is **AGPL-3.0**. Its suitability
was not audited for redistribution. Offline operation does not determine
license compatibility; a proper dependency/license review is still required.

These files are still discovered, detected, hashed, and metadata-tracked, and
every failure is recorded with a reason. Identical copies of such files are
**not** falsely reported as duplicates of each other's bytes.

## RAW + JPEG logical pairing

Cameras commonly write an identical-shot JPEG next to each RAW. Both files
become **one logical photo** (RAW primary) when — conservatively — all of:

- one is RAW and the other is `.jpg`/`.jpeg`;
- same folder and identical filename stem (case-insensitive);
- different file sizes (identical copies are duplicates, never a "pair");
- capture times, when both are known, are within 120 seconds.

Pairs are rebuilt from the whole library after every scan (and refreshed on
duplicate queries), pruned automatically when a side disappears, declared with
`ON DELETE CASCADE`, and exposed through the `get_logical_pairs` command.
Paired sides are excluded from duplicate-candidate grouping, so the twin is
never offered up as "delete one of these two".

## Database changes (Phase A)

- `scan_sessions`: new `phase`, `scan_path` columns (checkpoint/resume state).
- `scan_errors`: per-file failure log (`session_id, absolute_path, stage,
  reason, error_at`).
- `logical_photos`: `primary_id` (RAW), `paired_id` (JPEG), `match_reason`.
- All migrations are idempotent `CREATE TABLE IF NOT EXISTS` /
  conditional `ADD COLUMN`; existing libraries upgrade in place.

## Licensing decisions (all new dependencies)

| Crate | Version | License | Decision |
|---|---|---|---|
| `rawler` | 0.8.0 | **LGPL-2.1** | used in development; static-link redistribution obligations still require audit |
| `rav1d` | 1.1.0 | **BSD-2-Clause** | accepted (pure-Rust AV1 decoder; used `no-default-features` to skip asm/nasm) |
| `avif-parse` | 2.1.0 | **MPL-2.0** | accepted (pure-Rust HEIF/AVIF container parser; file-level copyleft only) |
| `libc` | 0.2 | MIT OR Apache-2.0 | accepted (alias for rav1d's negative-errno results) |
| `avif-decode` | 1.0.2 | BSD-3-Clause | **rejected** — needs cmake + C libaom, breaks pure-Rust build |
| `heic` (imazen) | – | **AGPL-3.0** | not added; redistribution compatibility not audited |
| `kamadak-exif` | 0.6.1 | MPL-2.0 | pre-existing; now also proves HEIC/HEIF/AVIF EXIF path |
| `image` | 0.25.10 | MIT OR Apache-2.0 | pre-existing (AVIF *encoder* feature remains inert) |

## Build, runtime, and platform notes

- Tools: `rustc 1.95.0`, `npm` — full **pure-Rust** dependency tree, no `cmake`
  or Python anywhere in the build.
- Fully offline at runtime; the single dependency is the local library and
  (optionally) a local Ollama endpoint. No telemetry, no network calls.
- Tested: **macOS (Apple Silicon)** — 63 unit/integration tests, 0 failures,
  2 manual-perf `#[ignore]`d. Both ignored checks were also run in `--release`
  this session (Trash roundtrip; 1.50 ms SHA-256 + 111 ms image processing),
  and the release bundle builds with `npm run tauri build -- --bundles app`.
- `.github/workflows/ci.yml` runs the frontend build, the Rust suite, and the
  cleanup-planner regression on macOS, Ubuntu 22.04, and Windows; it is the
  cross-platform check. It has **not** run yet (no remote), and the fixture
  tests skip cleanly without the dev-only samples.
- `src-tauri/fixtures/` holds dev-only sample files (CC-style sample content)
  used to verify decoders; they are excluded from any shipped build and their
  absence degrades nothing.

## Known limitations (candid)

1. HEIC/HEIF (HEVC) pixels undecodable on this build — see the section above.
   AVIF pixels decode via rav1d, with the prototype color/alpha scope noted
   in the AVIF section.
2. RAW per-camera decode is unproven until a fixture for that camera exists;
   the RAW capture path is shared across all 23 formats, so CR3/RW2/etc. rely
   on rawler's coverage.
3. RAW dimensions are the processed image's (embedded preview when used), not
   always the sensor's native resolution.
4. Logical pairing is deliberately conservative: renamed stems, RAW+TIFF
   twins, or shots more than 120s apart in metadata are not paired.
