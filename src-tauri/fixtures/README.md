# Test fixtures (development only)

These files are downloaded at development time from
[filesamples.com](https://filesamples.com/samples/image/), which publishes
sample images for software testing. They are **never bundled with the shipped
app** — they only activate the fixture-gated `#[test]`s in
`../src/ingest.rs` that prove the RAW/HEIC decode paths against real files.

| File | Camera / container / source | Purpose |
|------|--------------------|---------|
| `sample.dng` | Canon EOS 350D DIGITAL (DNG) — filesamples.com | verifies the rawler RAW pipeline (full demosaic path) |
| `sample.nef` | Nikon D3 (NEF) — filesamples.com | verifies the rawler RAW pipeline (embedded-preview path) |
| `sample.heic` | HEIF / HEVC item — filesamples.com | verifies HEIC detection + honest "pixels not supported" rejection |
| `raw_manifest.tsv` + 9 `sample-*.{cr2,cr3,arw,raf,orf,rw2,pef,srw,nef}` | pixls rawdb corpus | per-camera RAW verification across 9 brands |
| 4 `sample-avif-*.avif` | `hato` photograph, link-u/avif-sample-images (CC-BY-SA 4.0) | AVIF decode: 8-bit 4:2:0, 10-bit 4:2:0, 8-bit 4:2:2, 8-bit monochrome |

The 9 per-camera RAWs (and their `file | make | model | sensor_w | sensor_h`
rows in `raw_manifest.tsv`) come from the **pixls.us raw zoo**, mirrored by
dnglab's rawdb service (`https://rawdb.dnglab.org/api/download/…`) — the same
corpus rawler's own `rawdb` test suite downloads. Camera owners contributed
these files specifically for decoder testing. Use the documented rawdb model
keys; see `docs/COMPATIBILITY.md` for the verified matrix.

The four AVIF files are the `hato` still (3082×2048) from
[link-u/avif-sample-images](https://github.com/link-u/avif-sample-images),
each in a different decode profile, encoded with the `reduced_still_picture`
header flag — exactly the "still image" AVIF family. **License
[CC-BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/), author
Kaede Fujisaki**; attribution is kept here and in `docs/COMPATIBILITY.md`.
No EXIF metadata is present in these files.

Licensing: filesamples.com states its sample files are free to use for
testing purposes (public-domain-style sample content). The rawdb corpus files
were contributed by camera owners for decoder testing. SHA-256 digests are
recorded in each fixture-gated test's `.digest` note if the file changes,
tests still pass as long as the decode paths hold.

If a fixture is absent, its `#[test]` logs a skip message and returns early —
the suite stays green either way. To re-fetch:

```
curl -o ../fixtures/sample.dng  https://filesamples.com/samples/image/dng/sample1.dng
curl -o ../fixtures/sample.nef  https://filesamples.com/samples/image/nef/sample1.nef
curl -o ../fixtures/sample.heic https://filesamples.com/samples/image/heic/sample1.heic
```

Per-camera RAWs re-fetch from the rawdb corpus, e.g.:

```
curl -L -o ../fixtures/sample-canon-5d3.cr2 \
  "https://rawdb.dnglab.org/api/download/Canon/EOS%205D%20Mark%20III/raw_modes/Canon%20EOS%205D%20Mark%20III_RAW_ISO_200.CR2"
```

(`make/model` keys are the directories under `cameras/` in the
`dnglab/rawler-testdata` repository; the service rate-limits, so space
requests out.)

AVIFs re-fetch from the sample-images repo, e.g.:

```
curl -o ../fixtures/sample-avif-8bpc-yuv420.avif \
  https://raw.githubusercontent.com/link-u/avif-sample-images/master/hato.profile0.8bpc.yuv420.avif
```