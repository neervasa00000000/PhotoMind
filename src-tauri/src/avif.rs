//! Pure-Rust AVIF decoding path.
//!
//! - [`avif_parse`] (MPL-2.0) reads the ISO-BMFF/HEIF container and hands us
//!   the raw AV1 coded payload plus frame metadata (bit depth, chroma
//!   subsampling, monochrome).
//! - `rav1d` (BSD-2-Clause) is a pure-Rust port of dav1d. The crate currently
//!   ships dav1d's C API only (a friendlier Rust API is still in progress
//!   upstream), so this module binds the tiny subset we need: settings, open,
//!   send data, get picture, unref, close.
//! - This module converts the planar YUV output to an [`image::DynamicImage`].
//!
//! Both crates build with the Rust toolchain alone — no C compiler, no cmake,
//! no system libraries. HEVC-in-HEIF (plain HEIC) remains undecodable; only
//! AV1-in-HEIF (AVIF) is covered.
//!
//! Prototype scope: color comes from the primary item's `colr`/`nclx` box
//! when present, otherwise from the AV1 sequence header's color description
//! (the same code mappings), otherwise the AV1 defaults. Sub-sampled chroma is
//! upsampled with a separable bilinear filter phased by the bitstream's chroma
//! sample position. Alpha is dropped, and HDR signals use their standard
//! inverse EOTF with plain clamping to sRGB output — no perceptual tone
//! mapping. This is honest for index/thumbs/duplicates; archival color
//! management would need gamut mapping and HDR tone mapping.

use std::ffi::c_int;
use std::ptr::NonNull;

use image::{DynamicImage, RgbImage};
use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
use rav1d::include::dav1d::headers::{Dav1dSequenceHeader, Rav1dPixelLayout};
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::{
    dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_get_picture,
    dav1d_open, dav1d_picture_unref, dav1d_send_data,
};

/// rav1d returns 0 on success and negative errno values on failure; `EAGAIN`
/// means "no picture ready / buffer not consumed yet" and must be retried.
const EAGAIN: c_int = -(libc::EAGAIN as c_int);

/// One ISO-BMFF box: its 4CC type and its payload (after the 8-byte
/// size/type header, or 16 for 64-bit sizes).
struct Box<'a> {
    ty: [u8; 4],
    data: &'a [u8],
}

/// Iterate the top-level boxes in `blob`. Handles 32-bit sizes, the 64-bit
/// (`size == 1`) extension, and `size == 0` (box runs to EOF). Malformed
/// output is truncated silently — the box walker is advisory.
fn boxes(mut blob: &[u8]) -> impl Iterator<Item = Box<'_>> + '_ {
    std::iter::from_fn(move || {
        loop {
            if blob.len() < 8 {
                return None;
            }
            let size32 = u32::from_be_bytes(blob[0..4].try_into().unwrap());
            let (header, wide) = match size32 {
                1 => {
                    if blob.len() < 16 {
                        return None;
                    }
                    (
                        u64::from_be_bytes(blob[8..16].try_into().unwrap()) as usize,
                        true,
                    )
                }
                0 => (blob.len(), false), // box extends to EOF
                s => (s as usize, false),
            };
            let body_off = if wide { 16 } else { 8 };
            if header < body_off || header > blob.len() {
                return None;
            }
            let ty = [blob[4], blob[5], blob[6], blob[7]];
            let (data, rest) = (&blob[body_off..header], &blob[header..]);
            blob = rest;
            return Some(Box { ty, data });
        }
    })
}

/// The `nclx` color box from an AVIF file's `meta`/`iprp`/`ipco`, if the
/// primary item carries one. Returns the four IEC 61966-2-1 / ITU-R BT.2100
/// values rav1d's planar output should be interpreted with.
fn nclx_color_spec(data: &[u8]) -> Option<ColorSpec> {
    let (primaries, transfer, matrix, full_range) = find_primary_nclx(data)?;
    Some(ColorSpec {
        matrix: matrix_from_codes(matrix, primaries),
        transfer: transfer_from_code(transfer),
        full_range,
        primaries,
        // Siting is not carried by nclx; `decode_avif_bytes` fills it from the
        // AV1 sequence header. Colocated is the neutral placeholder.
        siting: ChromaSiting::Colocated,
    })
}

/// Walk the ISO-BMFF boxes to the primary item's `colr` property.
///
/// Layout (ISO/IEC 14496-12): top-level `meta` (a FullBox) contains `pitm`
/// (primary item id), `iprp` → `ipco` (property container, holds a `colr`
/// box at some 1-based index) and `ipma` (which properties each item
/// carries). We only look at the sub-set of the structure that AVIF
/// conformance requires, tolerating any interleaved unknown boxes.
fn find_primary_nclx(data: &[u8]) -> Option<(u16, u16, u16, bool)> {
    let meta = boxes(data).find(|b| b.ty == *b"meta")?;
    let mut meta_children = boxes(meta.data.get(4..)?); // skip the FullBox version/flags
    let primary = {
        let pitm = meta_children.find(|b| b.ty == *b"pitm")?;
        let v = pitm.data.first().copied()?; // FullBox version is a full byte
                                             // Item ID: 32-bit for version 0 per the spec, but real-world muxers
                                             // (ravif among them) also emit 16-bit ids; take whatever is present.
        let id = match (v, pitm.data.len()) {
            (0, 8..) => u32::from_be_bytes(pitm.data[4..8].try_into().ok()?) as u64,
            (0, 6..) => u16::from_be_bytes(pitm.data[4..6].try_into().ok()?) as u64,
            (0, 5..) => u64::from(pitm.data[4]),
            _ => u64::from_be_bytes(pitm.data.get(4..12)?.try_into().ok()?),
        };
        id
    };

    let iprp = meta_children.find(|b| b.ty == *b"iprp")?;
    // `iprp` and `ipco` are plain container boxes (children start right after
    // the 8-byte box header); only `meta`/`pitm`/`ipma` carry FullBox flags.
    let mut iprp_children = boxes(&iprp.data);
    let ipco = iprp_children.find(|b| b.ty == *b"ipco")?;

    // Property 1-based index of the `colr` box inside `ipco`.
    let colr_index = boxes(&ipco.data).position(|b| b.ty == *b"colr")? + 1;
    let ipma = iprp_children.find(|b| b.ty == *b"ipma")?;
    if !primary_has_property(&ipma.data, primary, colr_index) {
        return None;
    }

    // `colr` payload: colour_type ('nclx' | 'rICC' | 'prof') then per type.
    let colr_box = boxes(&ipco.data).nth(colr_index - 1)?;
    if colr_box.data.get(0..4) != Some(b"nclx") {
        return None; // ICC profiles are out of scope for the prototype.
    }
    let get = |range: std::ops::Range<usize>| -> Option<u16> {
        let bytes: [u8; 2] = colr_box.data.get(range)?.try_into().ok()?;
        Some(u16::from_be_bytes(bytes))
    };
    let full_range = colr_box.data.get(10).is_some_and(|b| b & 0x80 != 0);
    let (primaries, transfer, matrix) = (get(4..6)?, get(6..8)?, get(8..10)?);
    Some((primaries, transfer, matrix, full_range))
}

/// Does the item with id `item` declare property index `property` (1-based
/// into `ipco`) in the `ipma` association table?
fn primary_has_property(ipma: &[u8], item: u64, property: usize) -> bool {
    let Some(v) = ipma.first() else { return false };
    let version = *v; // FullBox version is the first byte; flags are the next three
    let Some(mut it) = ipma.get(4..) else {
        return false;
    }; // skip version/flags
    let Some(count) = it.get(0..4) else {
        return false;
    };
    let count = u32::from_be_bytes(count.try_into().unwrap());
    it = &it[4..];
    for _ in 0..count {
        let Some(id) = it.get(0..4) else { return false };
        let id = u32::from_be_bytes(id.try_into().unwrap()) as u64;
        // Entry layout: item_ID[0..4], association_count[4] (u8/u16), then the
        // association list.
        let (assoc, per, base) = if version == 0 {
            match it.get(4..5).and_then(|s| s.first().copied()) {
                Some(n) => (n as usize, 1usize, 5usize),
                None => return false,
            }
        } else {
            match it.get(4..6).and_then(|s| s.try_into().ok()) {
                Some(n) => (usize::from(u16::from_be_bytes(n)), 2usize, 6usize),
                None => return false,
            }
        };
        if id == item {
            // Check this item's declared properties for our target index.
            let entries = it.get(base..base + assoc * per).unwrap_or(&[]);
            return entries.chunks(per).any(|e| match per {
                1 => (e[0] as usize & 0x7F) == property,
                _ => usize::from(u16::from_be_bytes([e[0], e[1]])) & 0x7FFF == property,
            });
        }
        let step = base + assoc * per;
        it = match it.get(step..) {
            Some(rest) => rest,
            None => return false,
        };
    }
    false
}

/// YUV→RGB matrix coefficients, chosen by the container's `nclx`
/// `matrix_coefficients` (and, as a fallback, colour primaries) value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Matrix {
    /// YCbCr marked as identity: the three planes carry R, G, B directly.
    Identity,
    Bt601,
    Bt709,
    Bt2020Ncl,
    Smpte240,
}

/// Transfer characteristics applied by the encoder (its OETF). We undo it to
/// recover linear light, then re-encode to sRGB for the 8-bit output.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Transfer {
    /// No declared OETF (or an unsupported one): pass matrix output through.
    None,
    Bt709,
    Srgb,
    Bt2020,
    Pq,
    Hlg,
}

/// Where a chroma sample sits relative to the luma grid (AV1 sequence-header
/// `chroma_sample_position`). This decides the phase used when upsampling
/// subsampled chroma back to full resolution.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum ChromaSiting {
    /// Horizontally co-sited with luma, vertically centered between two luma
    /// rows (AV1 `CSP_VERTICAL = 1`).
    Vertical,
    /// Co-sited with the top-left luma sample of the 2×2 block
    /// (AV1 `CSP_COLOCATED = 2`, and the treatment for `CSP_UNKNOWN = 0`).
    #[default]
    Colocated,
}

/// Color configuration read from the AV1 sequence header. The container's
/// `nclx` box is authoritative when present; this is the fallback and the
/// only source of chroma-siting information.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SeqColor {
    primaries: u16,
    transfer: u16,
    matrix: u16,
    full_range: bool,
    siting: ChromaSiting,
}

/// Map an ISO/IEC 23091-2 `matrix_coefficients` code (and colour-primaries code
/// used when the matrix is unspecified) to our matrix.
fn matrix_from_codes(matrix: u16, primaries: u16) -> Matrix {
    match matrix {
        0 => Matrix::Identity,
        1 => Matrix::Bt709,
        5 | 6 => Matrix::Bt601, // BT.470BG / SMPTE 170M, both the 601 luminance
        7 => Matrix::Smpte240,
        9 | 10 => Matrix::Bt2020Ncl,
        _ => match primaries {
            // Unspecified matrix: inherit from the declared primaries.
            5 | 6 => Matrix::Bt601,
            9 => Matrix::Bt2020Ncl,
            1 => Matrix::Bt709,
            _ => Matrix::Bt709,
        },
    }
}

/// Map an ISO/IEC 23091-2 `transfer_characteristics` code to our inverse EOTF.
fn transfer_from_code(code: u16) -> Transfer {
    match code {
        0 | 2 | 4 | 5 | 9 | 10 | 17 => Transfer::None,
        1 | 6 | 7 => Transfer::Bt709,
        8 => Transfer::None, // linear light already
        11 | 13 => Transfer::Srgb,
        14 | 15 => Transfer::Bt2020,
        16 => Transfer::Pq,
        18 => Transfer::Hlg,
        _ => Transfer::None,
    }
}

/// Color interpretation for one AVIF decode: where the `nclx` values point.
#[derive(Clone, Copy, Debug)]
struct ColorSpec {
    matrix: Matrix,
    transfer: Transfer,
    /// nclx `full_range_flag`: false = studio/limited range (16..235).
    full_range: bool,
    /// Raw colour primaries code (kept for diagnostics; drives the fallback).
    #[allow(dead_code)]
    primaries: u16,
    /// Chroma sample position, from the AV1 bitstream (never from nclx).
    siting: ChromaSiting,
}

/// Decompressed planar AV1 frame, owned locally so the FFI picture can be
/// released before we build the RGB image.
struct Av1Frame {
    width: usize,
    height: usize,
    layout: Rav1dPixelLayout,
    bit_depth: i32,
    /// [Y, Cb, Cr]; `None` components for I400 (no chroma).
    planes: [Option<Vec<u8>>; 3],
    /// Stride (bytes) per plane row.
    strides: [isize; 3],
    /// Color description from the AV1 sequence header (fallback when the
    /// container carries no `nclx`, and the only source of chroma siting).
    seq_color: Option<SeqColor>,
}

/// Decode an AVIF file's bytes to an RGB image.
pub fn decode_avif_bytes(data: &[u8]) -> Result<DynamicImage, String> {
    let file = avif_parse::read_avif(&mut std::io::Cursor::new(data))
        .map_err(|e| format!("Could not parse the AVIF container: {e}"))?;

    // Guard against absurd frame sizes before the decoder allocates 64-bit
    // arena buffers for e.g. a corrupted or bomb sequence header.
    let meta = file
        .primary_item_metadata()
        .map_err(|e| format!("Could not read the AV1 sequence header: {e}"))?;
    let (w, h) = (meta.max_frame_width.get(), meta.max_frame_height.get());
    if w.saturating_mul(h) > 64_000_000 {
        return Err(format!(
            "AVIF frame is {w}x{h}, above the 64-Megapixel decode safety cap"
        ));
    }

    let nclx = nclx_color_spec(data);
    let frame = decode_av1(&file.primary_item)?;
    let mut spec = nclx.unwrap_or_else(|| frame_color_spec(&frame));
    // nclx never carries sample siting; the bitstream does.
    spec.siting = frame
        .seq_color
        .map_or(ChromaSiting::Colocated, |c| c.siting);
    let rgb = frame_to_rgb(&frame, spec)?;
    Ok(DynamicImage::ImageRgb8(rgb))
}

/// Color spec when the container declares no `nclx`: prefer the AV1 sequence
/// header's color description, falling back to the AV1-specified defaults
/// (BT.601 matrix for sub-sampled layouts, BT.709 for 4:4:4, studio range,
/// no declared transfer).
fn frame_color_spec(frame: &Av1Frame) -> ColorSpec {
    if let Some(c) = frame.seq_color {
        return ColorSpec {
            matrix: matrix_from_codes(c.matrix, c.primaries),
            transfer: transfer_from_code(c.transfer),
            full_range: c.full_range,
            primaries: c.primaries,
            siting: c.siting,
        };
    }
    ColorSpec {
        matrix: if frame.layout == Rav1dPixelLayout::I444 {
            Matrix::Bt709
        } else {
            Matrix::Bt601
        },
        transfer: Transfer::None,
        full_range: false,
        primaries: 0,
        siting: ChromaSiting::Colocated,
    }
}

/// Decode a raw AV1 annex-B bitstream (an OBU stream) to planar YUV using the
/// rav1d (dav1d-ABI) FFI. All FFI calls are kept inside this one function.
fn decode_av1(obu: &[u8]) -> Result<Av1Frame, String> {
    if obu.is_empty() {
        return Err("AV1 payload is empty".to_string());
    }

    unsafe {
        // 1. Default settings, then a single decode thread (still image).
        let mut maybe_settings = std::mem::MaybeUninit::<Dav1dSettings>::uninit();
        dav1d_default_settings(NonNull::new_unchecked(maybe_settings.as_mut_ptr()));
        let mut settings = maybe_settings.assume_init();
        settings.n_threads = 1;
        settings.max_frame_delay = 0;

        // 2. Open a decoder instance.
        let mut ctx_slot: Option<Dav1dContext> = None;
        let open = dav1d_open(
            Some(NonNull::from(&mut ctx_slot)),
            Some(NonNull::from(&mut settings)),
        );
        if open.0 != 0 {
            return Err(format!("rav1d failed to open: {}", open.0));
        }

        // Ensure the context is closed on every path, including early errors.
        struct ContextGuard(NonNull<Option<Dav1dContext>>);
        impl Drop for ContextGuard {
            fn drop(&mut self) {
                // SAFETY: `slot` holds the context from `dav1d_open` and is
                // only closed once here; `dav1d_close` nulls the option out.
                unsafe { dav1d_close(Some(self.0)) };
            }
        }
        let _ctx_guard = ContextGuard(NonNull::new_unchecked(&mut ctx_slot));
        let ctx = *ctx_slot.as_ref().unwrap();

        // 3. Hand the compressed bitstream to the decoder.
        let mut maybe_data = std::mem::MaybeUninit::<Dav1dData>::uninit();
        let out = dav1d_data_create(
            Some(NonNull::new_unchecked(maybe_data.as_mut_ptr())),
            obu.len(),
        );
        if out.is_null() {
            return Err("rav1d could not allocate an input buffer".to_string());
        }
        let mut data = maybe_data.assume_init();
        // SAFETY: `out` is `obu.len()` >= 1 writable bytes from `data_create`.
        std::ptr::copy_nonoverlapping(obu.as_ptr(), out, obu.len());

        // Send until consumed (never for a still frame, but harmless).
        let mut attempts = 0;
        loop {
            let res = dav1d_send_data(Some(ctx), Some(NonNull::from(&mut data)));
            if res.0 == 0 {
                break; // The decoder took ownership of the buffer.
            }
            if res.0 != EAGAIN {
                // The buffer was not consumed; release it ourselves.
                dav1d_data_unref(Some(NonNull::from(&mut data)));
                return Err(format!("rav1d rejected the bitstream: {}", res.0));
            }
            attempts += 1;
            if attempts > 8 {
                dav1d_data_unref(Some(NonNull::from(&mut data)));
                return Err("rav1d never accepted the bitstream".to_string());
            }
            drain_one_picture(ctx)?.map(|frames| drop(frames));
        }

        // 4. Pull the decoded picture.
        let frame = drain_one_picture(ctx)?
            .ok_or_else(|| "rav1d produced no picture for a still frame".to_string())?;
        Ok(frame)
    }
}

/// Pull exactly one decoded picture. `None` only if the decoder has nothing
/// ready yet after EAGAIN (drained during the send loop).
///
/// # Safety
///
/// `ctx` must be a live `dav1d_open` context for the duration of the call.
unsafe fn drain_one_picture(ctx: Dav1dContext) -> Result<Option<Av1Frame>, String> {
    let mut maybe_pic = std::mem::MaybeUninit::<Dav1dPicture>::zeroed();
    loop {
        let res = dav1d_get_picture(
            Some(ctx),
            Some(NonNull::new_unchecked(maybe_pic.as_mut_ptr())),
        );
        if res.0 == 0 {
            let mut pic = maybe_pic.assume_init();
            let frame = copy_picture_planes(&pic);
            dav1d_picture_unref(Some(NonNull::from(&mut pic)));
            return frame.map(Some);
        }
        if res.0 == EAGAIN {
            return Ok(None);
        }
        return Err(format!("rav1d picture error: {}", res.0));
    }
}

/// Copy the plane data out of a rav1d picture into owned `Vec`s. Callers must
/// `dav1d_picture_unref` the picture afterwards; this only borrows it.
fn copy_picture_planes(pic: &Dav1dPicture) -> Result<Av1Frame, String> {
    let params = &pic.p;
    let (w, h) = (params.w.max(0) as usize, params.h.max(0) as usize);
    if w == 0 || h == 0 || w.checked_mul(h).is_none_or(|pixels| pixels > 64_000_000) {
        return Err("AVIF picture dimensions exceed the decode safety budget".into());
    }
    let layout =
        Rav1dPixelLayout::try_from(params.layout).map_err(|_| "Invalid AVIF pixel layout")?;
    let bpc = params.bpc;
    if ![8, 10, 12].contains(&bpc) {
        return Err("Invalid AVIF bit depth".into());
    }

    // The sequence header is retained by the picture; read its color
    // description while the picture is still alive.
    let seq_color = pic.seq_hdr.map(|nn| {
        // SAFETY: `seq_hdr` points at the sequence header owned by this
        // picture's ref-counted header set, which outlives this borrow.
        let seq: &Dav1dSequenceHeader = unsafe { nn.as_ref() };
        SeqColor {
            primaries: seq.pri as u16,
            transfer: seq.trc as u16,
            matrix: seq.mtrx as u16,
            full_range: seq.color_range != 0,
            siting: if seq.chr == 1 {
                ChromaSiting::Vertical
            } else {
                ChromaSiting::Colocated
            },
        }
    });

    let (cw, ch) = match layout {
        Rav1dPixelLayout::I444 => (w, h),
        Rav1dPixelLayout::I422 => (w.div_ceil(2), h),
        Rav1dPixelLayout::I420 => (w.div_ceil(2), h.div_ceil(2)),
        Rav1dPixelLayout::I400 => (0, 0),
    };
    let element_len: usize = if bpc > 8 { 2 } else { 1 };

    let mut planes: [Option<Vec<u8>>; 3] = [None, None, None];
    let mut strides: [isize; 3] = [0, 0, 0];
    let dims = [(w, h), (cw, ch), (cw, ch)];

    for i in 0..3 {
        if dims[i].0 == 0 || dims[i].1 == 0 {
            continue;
        }
        let nn = pic.data[i].ok_or("Missing AVIF pixel plane")?;
        let stride = pic.stride[i.min(1)]; // stride[0]=luma, stride[1]=shared chroma
        let plane_w = dims[i].0;
        let plane_h = dims[i].1;
        let row_bytes = plane_w * element_len;
        if stride.unsigned_abs() < row_bytes {
            return Err("Invalid AVIF plane stride".into());
        }
        let last_offset = (plane_h - 1) as isize;
        if last_offset.checked_mul(stride).is_none() {
            return Err("AVIF plane stride overflow".into());
        }
        // `data[i]` always points at the first render row; a negative stride
        // means rows run upward in memory, so `row * stride` covers both.
        let base = nn.as_ptr().cast::<u8>();
        let mut buf = Vec::new();
        buf.try_reserve_exact(plane_h * row_bytes)
            .map_err(|_| "AVIF plane allocation exceeds available memory")?;
        for row in 0..plane_h {
            let row_ptr = unsafe { base.byte_offset(row as isize * stride) };
            // SAFETY: `stride` abs covers `row_bytes` per row (checked above)
            // and the plane is `plane_h` rows tall from `data[i]`.
            unsafe {
                buf.extend_from_slice(std::slice::from_raw_parts(row_ptr, row_bytes));
            }
        }
        planes[i] = Some(buf);
        strides[i] = row_bytes as isize;
    }
    Ok(Av1Frame {
        width: w,
        height: h,
        layout,
        bit_depth: bpc,
        planes,
        strides,
        seq_color,
    })
}

/// Convert planar YUV/RGB (8/10/12-bit, 4:0:0 / 4:2:0 / 4:2:2 / 4:4:4) to an
/// 8-bit sRGB image, honoring the decode's `ColorSpec`.
///
/// `spec.matrix == Identity` means the three planes already carry R, G, B and
/// only the range/transfer apply. Sub-sampled chroma is upsampled with a
/// separable bilinear filter phased by the AV1 chroma sample position;
/// 4:4:4 and 4:2:2 reads are exact. Fine for previews and duplicate analysis,
/// not archival.
fn frame_to_rgb(frame: &Av1Frame, spec: ColorSpec) -> Result<RgbImage, String> {
    let (w, h) = (frame.width, frame.height);
    let max_sample = (1u32 << frame.bit_depth.max(1)) - 1;
    let max_f = max_sample as f64;

    if frame.bit_depth > 16 {
        return Err(format!("Unsupported AV1 bit depth {}", frame.bit_depth));
    }

    let plane_at = |i: usize| -> Option<&[u8]> { frame.planes[i].as_deref() };
    let plane_stride = |i: usize| -> usize { frame.strides[i].max(0) as usize };
    // The sample clamp must use the REAL chroma plane size, or I444/full-size
    // chroma would pin reads to the top-left 2:1 tile.
    let (chroma_w, chroma_h) = match frame.layout {
        Rav1dPixelLayout::I444 => (w, h),
        Rav1dPixelLayout::I422 => (w.div_ceil(2), h),
        _ => (w.div_ceil(2), h.div_ceil(2)),
    };
    // Chroma sampling step per chroma sample.
    let (step_x, step_y) = match frame.layout {
        Rav1dPixelLayout::I444 => (1usize, 1usize),
        Rav1dPixelLayout::I422 => (2, 1),
        _ => (2, 2),
    };

    let elem = if frame.bit_depth > 8 { 2usize } else { 1 };
    // Sample a plane value as a float in 0.0..=1.0. Coordinates are clamped to
    // the plane, so out-of-range (bilinear tap) reads replicate the edge.
    let sample = |plane: Option<&[u8]>, stride: usize, x: isize, y: isize, pw: usize, ph: usize| {
        let Some(p) = plane else { return 127.5 / max_f };
        if pw == 0 || ph == 0 {
            return 127.5 / max_f;
        }
        let px = x.clamp(0, pw as isize - 1) as usize;
        let py = y.clamp(0, ph as isize - 1) as usize;
        let idx = py * stride + px * elem;
        let v: u32 = if frame.bit_depth > 8 {
            u16::from_le_bytes([p[idx], p[idx + 1]]) as u32
        } else {
            p[idx] as u32
        };
        (v as f64 / max_f).clamp(0.0, 1.0)
    };

    // Upsample one chroma component to luma resolution with a separable
    // bilinear (tent) filter, phased by the chroma sample position. For
    // 4:4:4 (and 4:2:2 vertically) this collapses to a direct read.
    let chroma_at = |plane: Option<&[u8]>, stride: usize, x: usize, y: usize| -> f64 {
        if step_x == 1 && step_y == 1 {
            return sample(plane, stride, x as isize, y as isize, chroma_w, chroma_h);
        }
        let hpos = x as f64 / step_x as f64;
        let vpos = match (step_y, spec.siting) {
            // Vertically centered between two luma rows.
            (2, ChromaSiting::Vertical) => y as f64 / 2.0 - 0.25,
            (sy, _) => y as f64 / sy as f64,
        };
        let (h0, tx) = (hpos.floor(), hpos.fract());
        let (v0, ty) = (vpos.floor(), vpos.fract());
        let (h0, v0) = (h0 as isize, v0 as isize);
        let s = |xi: isize, yi: isize| sample(plane, stride, xi, yi, chroma_w, chroma_h);
        let top = s(h0, v0) + (s(h0 + 1, v0) - s(h0, v0)) * tx;
        let bot = s(h0, v0 + 1) + (s(h0 + 1, v0 + 1) - s(h0, v0 + 1)) * tx;
        top + (bot - top) * ty
    };

    // Studio range: luma 16..235, chroma 16..240 centered on 128. Full range:
    // luma 0..255, chroma centered on 128 (0.5 when normalized).
    let luma = |v: f64| {
        if spec.full_range {
            v
        } else {
            ((v * 255.0) - 16.0) / 219.0
        }
    };
    let chroma = |v: f64| {
        if spec.full_range {
            v - 0.5
        } else {
            ((v * 255.0) - 128.0) / 224.0
        }
    };

    // Inverse EOTF (signal -> linear light) for the declared transfer.
    let inverse_eotf = |x: f64| -> f64 {
        match spec.transfer {
            Transfer::None => x,
            Transfer::Bt709 => bt709_to_linear(x),
            Transfer::Srgb => srgb_to_linear(x),
            Transfer::Bt2020 => bt2020_to_linear(x),
            Transfer::Pq => pq_to_linear(x),
            Transfer::Hlg => hlg_to_linear(x),
        }
    };
    // Linear light -> sRGB signal for the 8-bit output.
    let to_srgb = |x: f64| {
        if x <= 0.003_130_8 {
            12.92 * x
        } else {
            1.055 * x.powf(1.0 / 2.4) - 0.055
        }
    };

    let mut out = RgbImage::new(w as u32, h as u32);
    let y_plane = plane_at(0);
    let y_stride = plane_stride(0);
    let cb_plane = plane_at(1);
    let cr_plane = plane_at(2);
    let cb_stride = plane_stride(1);
    let cr_stride = plane_stride(2);

    for y in 0..h {
        for x in 0..w {
            let yv = sample(y_plane, y_stride, x as isize, y as isize, w, h);
            let cb = chroma_at(cb_plane, cb_stride, x, y);
            let cr = chroma_at(cr_plane, cr_stride, x, y);

            // Three primaries in the source signal space (0.0..1.0-ish).
            let (rp, gp, bp): (f64, f64, f64) = match spec.matrix {
                Matrix::Identity => (luma(yv), luma(cb), luma(cr)),
                _ => {
                    let yv = luma(yv);
                    let (cbr, crr) = (chroma(cb), chroma(cr));
                    let (kr, kb) = match spec.matrix {
                        Matrix::Bt601 => (0.2990, 0.1140),
                        Matrix::Bt709 => (0.2126, 0.0722),
                        Matrix::Bt2020Ncl => (0.2627, 0.0593),
                        Matrix::Smpte240 => (0.2120, 0.0870),
                        Matrix::Identity => unreachable!(),
                    };
                    let (cr_f, cb_f) = (2.0 * (1.0 - kr), 2.0 * (1.0 - kb));
                    let kg = 1.0 - kr - kb;
                    (
                        yv + cr_f * crr,
                        yv - (kr / kg) * cr_f * crr - (kb / kg) * cb_f * cbr,
                        yv + cb_f * cbr,
                    )
                }
            };

            // Undo the encoder's OETF, then encode to sRGB for display.
            let px = |v: f64| {
                let lin = inverse_eotf(v.clamp(0.0, 1.0));
                let disp = if spec.transfer == Transfer::None {
                    lin
                } else {
                    to_srgb(lin.clamp(0.0, 1.0))
                };
                (disp.clamp(0.0, 1.0) * 255.0).round() as u8
            };
            out.put_pixel(x as u32, y as u32, image::Rgb([px(rp), px(gp), px(bp)]));
        }
    }
    Ok(out)
}

/// BT.709 OETF inverse (rec. ITU-R BT.709 / SMPTE 170M / 240M curves).
fn bt709_to_linear(c: f64) -> f64 {
    if c <= 0.081 {
        c / 4.5
    } else {
        ((c + 0.099) / 1.099).powf(1.0 / 0.45)
    }
}

/// sRGB OETF inverse (IEC 61966-2-1).
fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// BT.2020 OETF inverse — the BT.709 shape with 2020's (α, β) = (1.0993, 0.081).
fn bt2020_to_linear(c: f64) -> f64 {
    if c <= 0.081 {
        c / 4.5
    } else {
        ((c + 0.0993) / 1.0993).powf(1.0 / 0.45)
    }
}

/// SMPTE ST 2084 (PQ) EOTF inverse, normalized so the 10,000-nit peak maps to
/// linear 1.0. HDR ranges clip at the sRGB output ceiling (no tone mapping).
fn pq_to_linear(c: f64) -> f64 {
    const M1: f64 = 2610.0 / 16384.0;
    const M2: f64 = 2523.0 / 4096.0 * 128.0;
    const C1: f64 = 3424.0 / 4096.0;
    const C2: f64 = 2413.0 / 4096.0 * 32.0;
    const C3: f64 = 2392.0 / 4096.0 * 32.0;
    let cp = c.powf(1.0 / M2);
    let l = ((cp - C1).max(0.0) / (C2 - C3 * cp)).powf(1.0 / M1);
    (l / 10_000.0).clamp(0.0, 1.0)
}

/// ARIB STD-B67 (HLG) EOTF inverse, mapped to display light per the BT.2390
/// reference (scene light × 12). Non-HDR output clips as above.
fn hlg_to_linear(c: f64) -> f64 {
    const A: f64 = 0.178_832_77;
    const B: f64 = 0.284_668_92; // 1 - 4A
    const C: f64 = 0.559_910_73; // 0.5 - A·ln(4A)
    const M: f64 = 12.0;
    let l = if c <= 0.5 {
        (c * c) / 3.0
    } else {
        (((c - C) / A).exp() + B) / M
    };
    (l * M).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Box-walker tests over hand-built - but structurally real - boxes ----

    /// Concatenate `payload` into a 32-bit-size ISO-BMFF box.
    fn b4(ty: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + payload.len());
        out.extend(u32::to_be_bytes(8 + payload.len() as u32));
        out.extend_from_slice(ty);
        out.extend_from_slice(payload);
        out
    }

    /// `meta` with one primary item id and one associated `colr`/`nclx` box.
    /// `ipma_v1` selects the 16-bit association variant of `ipma`.
    fn container_with_nclx(primary_id: u64, ipma_v1: bool, associate_primary: bool) -> Vec<u8> {
        let colr = b4(b"colr", &{
            let mut p = Vec::new();
            p.extend_from_slice(b"nclx");
            p.extend(u16::to_be_bytes(1)); // colour primaries: BT.709
            p.extend(u16::to_be_bytes(13)); // transfer: sRGB
            p.extend(u16::to_be_bytes(6)); // matrix: BT.601
            p.push(0x80); // full_range_flag
            p
        });
        let ipco = b4(b"ipco", &colr);

        // ipma FullBox (version, flags) then one item association. The item
        // points at property (index 1 = the colr box) only when the caller
        // wants it adopted; otherwise it references an unused property.
        let prop_index: u16 = if associate_primary { 1 } else { 2 };
        let mut ipma = if ipma_v1 {
            vec![1u8, 0, 0, 0]
        } else {
            vec![0u8; 4]
        };
        ipma.extend(u32::to_be_bytes(1)); // entry_count
        ipma.extend(u64::to_be_bytes(primary_id)[4..8].to_vec()); // item_ID (32-bit)
        if ipma_v1 {
            ipma.extend(u16::to_be_bytes(1)); // association_count (u16)
            ipma.extend(u16::to_be_bytes(prop_index)); // essential=0, index
        } else {
            ipma.push(1); // association_count (u8)
            ipma.push(prop_index as u8); // essential=0, property index
        }
        let ipma = b4(b"ipma", &ipma);
        let iprp = b4(b"iprp", &[ipco, ipma].concat());

        let mut meta = vec![0u8; 4]; // FullBox version 0, flags 0
        let mut pitm = vec![0u8; 4];
        pitm.extend(u64::to_be_bytes(primary_id)[4..8].to_vec()); // (1) primary id
        meta.extend(b4(b"pitm", &pitm));
        meta.extend(iprp);
        let meta = b4(b"meta", &meta);

        let ftyp = b4(
            b"ftyp",
            &[b'a', b'v', b'i', b'f', 0, 0, 0, 0, b'm', b'i', b'f', b'1'],
        );
        [ftyp, meta].concat()
    }

    #[test]
    fn nclx_walker_finds_primary_item_colr() {
        let data = container_with_nclx(7, false, true);
        let spec = nclx_color_spec(&data).expect("primary colr should be found");
        assert_eq!(spec.matrix, Matrix::Bt601);
        assert_eq!(spec.transfer, Transfer::Srgb);
        assert!(spec.full_range);
        assert_eq!(spec.primaries, 1);
    }

    #[test]
    fn nclx_walker_handles_ipma_version1_associations() {
        let data = container_with_nclx(1, true, true);
        assert_eq!(
            nclx_color_spec(&data).map(|s| s.matrix),
            Some(Matrix::Bt601)
        );
    }

    #[test]
    fn nclx_walker_ignores_unattached_colr() {
        // A colr box that is not associated with the primary item (ipma does
        // not list property 1 for it) must not be adopted.
        let data = container_with_nclx(1, false, false);
        assert!(nclx_color_spec(&data).is_none());
    }

    #[test]
    fn nclx_omits_container_defaults() {
        // No colr in ipco, no ipma entries: nothing to adopt, so the caller
        // falls back to the AV1-specified defaults.
        let ipco = b4(b"ipco", &[]);
        let mut ipma = vec![0u8; 4];
        ipma.extend(u32::to_be_bytes(0)); // no item entries
        let iprp = b4(b"iprp", &ipco);
        let mut meta = vec![0u8; 4];
        let mut pitm = vec![0u8; 4];
        pitm.extend(u32::to_be_bytes(1));
        meta.extend(b4(b"pitm", &pitm));
        meta.extend(iprp);
        let meta = b4(b"meta", &meta);
        let data = [b4(b"ftyp", &[0; 8]), meta].concat();
        assert!(nclx_color_spec(&data).is_none());
    }

    // ---- Color-math checks over a synthetic 1×1 frame ----

    fn one_px_frame(rgb: [u8; 3]) -> Av1Frame {
        Av1Frame {
            width: 1,
            height: 1,
            layout: Rav1dPixelLayout::I444,
            bit_depth: 8,
            planes: [Some(vec![rgb[0]]), Some(vec![rgb[1]]), Some(vec![rgb[2]])],
            strides: [1, 1, 1],
            seq_color: None,
        }
    }

    fn convert_px(frame: &Av1Frame, spec: ColorSpec) -> [u8; 3] {
        let img = frame_to_rgb(frame, spec).unwrap();
        let p = img.get_pixel(0, 0);
        [p[0], p[1], p[2]]
    }

    #[test]
    fn identity_full_range_passthrough_keeps_mid_gray() {
        let spec = ColorSpec {
            matrix: Matrix::Identity,
            transfer: Transfer::None,
            full_range: true,
            primaries: 0,
            siting: ChromaSiting::Colocated,
        };
        assert_eq!(
            convert_px(&one_px_frame([128, 128, 128]), spec),
            [128, 128, 128]
        );
    }

    #[test]
    fn studio_range_709_black_luma_is_black() {
        // Y=16, Cb=Cr=128 (studio range zero chroma) must decode to pure black.
        let spec = ColorSpec {
            matrix: Matrix::Bt709,
            transfer: Transfer::None,
            full_range: false,
            primaries: 1,
            siting: ChromaSiting::Colocated,
        };
        let px = convert_px(&one_px_frame([16, 128, 128]), spec);
        assert!(
            px[0] <= 1 && px[1] <= 1 && px[2] <= 1,
            "expected black, got {px:?}"
        );
    }

    #[test]
    fn full_range_601_encodes_pure_red_recoverably() {
        // BT.601 full-range YUV for pure red (255,0,0): Y≈0.299, Cb≈0.331,
        // Cr=1.0. Quantize to 8-bit and confirm the inverse recovers red.
        let spec = ColorSpec {
            matrix: Matrix::Bt601,
            transfer: Transfer::None,
            full_range: true,
            primaries: 0,
            siting: ChromaSiting::Colocated,
        };
        let y = (0.2990 * 255.0) as u8;
        let cb = (0.331_264 * 255.0) as u8;
        let px = convert_px(&one_px_frame([y, cb, 255]), spec);
        assert!(
            px[0] >= 252 && px[1] <= 3 && px[2] <= 3,
            "expected red, got {px:?}"
        );
    }

    #[test]
    fn srgb_transfer_roundtrips_mid_gray() {
        // sRGB transfer must be self-consistent for a neutral gray even with
        // BT.601 coefficients (full range).
        let spec = ColorSpec {
            matrix: Matrix::Bt601,
            transfer: Transfer::Srgb,
            full_range: true,
            primaries: 0,
            siting: ChromaSiting::Colocated,
        };
        let px = convert_px(&one_px_frame([128, 128, 128]), spec);
        assert!(
            px[0].abs_diff(128) <= 1 && px[1].abs_diff(128) <= 1 && px[2].abs_diff(128) <= 1,
            "got {px:?}"
        );
    }

    #[test]
    fn pq_and_hlg_paths_do_not_panic_and_stay_in_range() {
        for transfer in [Transfer::Pq, Transfer::Hlg] {
            let spec = ColorSpec {
                matrix: Matrix::Bt2020Ncl,
                transfer,
                full_range: true,
                primaries: 9,
                siting: ChromaSiting::Colocated,
            };
            // Exercising the inverse-EOTF path is the point; u8 output is
            // inherently in range, so reaching this line is the assertion.
            let _ = convert_px(&one_px_frame([128, 128, 128]), spec);
        }
    }

    // ---- Chroma siting + interpolation ----

    fn yuv420_frame(w: usize, h: usize, cb: [u8; 4], cr: u8) -> Av1Frame {
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        Av1Frame {
            width: w,
            height: h,
            layout: Rav1dPixelLayout::I420,
            bit_depth: 8,
            planes: [
                Some(vec![0u8; w * h]),
                Some(cb.to_vec()),
                Some(vec![cr; cw * ch]),
            ],
            strides: [w as isize, cw as isize, cw as isize],
            seq_color: None,
        }
    }

    // Identity matrix + full range makes the output Green channel the raw
    // interpolated Cb, so siting/interpolation is directly observable.
    fn identity_spec(siting: ChromaSiting) -> ColorSpec {
        ColorSpec {
            matrix: Matrix::Identity,
            transfer: Transfer::None,
            full_range: true,
            primaries: 0,
            siting,
        }
    }

    #[test]
    fn colocated_chroma_interpolates_halfway_between_samples() {
        let f = yuv420_frame(4, 4, [10, 20, 30, 40], 0);
        let img = frame_to_rgb(&f, identity_spec(ChromaSiting::Colocated)).unwrap();
        let g = |x: u32, y: u32| img.get_pixel(x, y)[1];
        assert_eq!(g(0, 0), 10, "even/even luma is the co-sited sample");
        assert_eq!(g(1, 0), 15, "odd column averages horizontally");
        assert_eq!(g(0, 1), 20, "odd row averages vertically");
        assert_eq!(g(1, 1), 25, "odd/odd averages all four taps");
    }

    #[test]
    fn vertical_siting_shifts_the_interpolation_phase() {
        let f = yuv420_frame(4, 4, [10, 20, 30, 40], 0);
        let img = frame_to_rgb(&f, identity_spec(ChromaSiting::Vertical)).unwrap();
        let g = |x: u32, y: u32| img.get_pixel(x, y)[1];
        assert_eq!(g(0, 0), 10, "row 0 clamps to the first chroma row");
        assert_eq!(g(0, 1), 15, "vertical phase is offset by a quarter");
        assert_eq!(g(0, 2), 25, "quarter-phase lands between rows 0 and 1");
        assert_eq!(g(0, 3), 30, "last row clamps to the final chroma row");
        assert_eq!(g(1, 0), 15, "horizontal interpolation is unchanged");
    }

    #[test]
    fn av1_and_nclx_share_one_code_mapping() {
        assert_eq!(matrix_from_codes(0, 0), Matrix::Identity);
        assert_eq!(matrix_from_codes(6, 0), Matrix::Bt601);
        assert_eq!(matrix_from_codes(9, 0), Matrix::Bt2020Ncl);
        // Unspecified matrix inherits from the primaries.
        assert_eq!(matrix_from_codes(2, 5), Matrix::Bt601);
        assert_eq!(matrix_from_codes(2, 9), Matrix::Bt2020Ncl);
        assert_eq!(transfer_from_code(16), Transfer::Pq);
        assert_eq!(transfer_from_code(13), Transfer::Srgb);
        assert_eq!(transfer_from_code(8), Transfer::None);
    }

    #[test]
    fn frame_color_spec_prefers_the_sequence_header() {
        let mut f = yuv420_frame(4, 4, [10, 20, 30, 40], 0);
        f.seq_color = Some(SeqColor {
            primaries: 9,
            transfer: 16,
            matrix: 9,
            full_range: true,
            siting: ChromaSiting::Vertical,
        });
        let s = frame_color_spec(&f);
        assert_eq!(s.matrix, Matrix::Bt2020Ncl);
        assert_eq!(s.transfer, Transfer::Pq);
        assert!(s.full_range);
        assert_eq!(s.siting, ChromaSiting::Vertical);

        f.seq_color = None;
        let s2 = frame_color_spec(&f);
        assert_eq!(s2.matrix, Matrix::Bt601, "sub-sampled layout default");
        assert!(!s2.full_range);
    }

    // ---- Real round trip: ravif encodes, we decode ----

    #[test]
    fn encoded_avif_roundtrips_through_known_encoding() {
        let img = {
            let mut img = RgbImage::new(512, 512);
            for (_, y, p) in img.enumerate_pixels_mut() {
                let band = if y < 170 {
                    0
                } else if y < 340 {
                    1
                } else {
                    2
                };
                *p = match band {
                    0 => image::Rgb([255, 0, 0]),
                    1 => image::Rgb([128, 128, 128]),
                    _ => image::Rgb([0, 128, 128]),
                };
            }
            img
        };
        let path = std::env::temp_dir().join(format!("roundtrip-{}.avif", std::process::id()));
        img.save_with_format(&path, image::ImageFormat::Avif)
            .expect("ravif encode must work");
        let bytes = std::fs::read(&path).expect("encode output readable");
        let _ = std::fs::remove_file(&path);

        // Documented ravif behavior: it encodes full-range BT.601 YCbCr but
        // writes no `colr` box at all. Check both halves of that contract.
        assert!(nclx_color_spec(&bytes).is_none(), "ravif emits no nclx box");

        // Our public path works anyway (falls back to the AV1 defaults).
        let decoded = decode_avif_bytes(&bytes).expect("public decode must accept ravif output");
        let public_rgb = decoded.to_rgb8();
        assert_eq!((public_rgb.width(), public_rgb.height()), (512, 512));

        // ravif's AV1 sequence header declares full-range BT.601 + sRGB, so
        // the public fallback path (no nclx box) recovers the bands too.
        let file = avif_parse::read_avif(&mut std::io::Cursor::new(bytes)).unwrap();
        let frame = decode_av1(&file.primary_item).expect("rav1d decodes ravif output");
        let seq = frame.seq_color.expect("AV1 sequence header carries color");
        assert_eq!(
            (seq.matrix, seq.transfer, seq.full_range),
            (6, 13, true),
            "documented ravif sequence-header color"
        );

        // Also decode with the encoding spelled out explicitly, proving the
        // conversion against a real bitstream rather than our own assumptions.
        let spec = ColorSpec {
            matrix: Matrix::Bt601,
            transfer: Transfer::None,
            full_range: true,
            primaries: 0,
            siting: ChromaSiting::Colocated,
        };
        let rgb = frame_to_rgb(&frame, spec).unwrap();

        let band_means = |img: &RgbImage, start: u32, end: u32| {
            let mut sum = [0u64; 3];
            for y in start..end {
                for x in 0..512 {
                    let p = *img.get_pixel(x, y);
                    for c in 0..3 {
                        sum[c] += p[c] as u64;
                    }
                }
            }
            let n = (end - start) as f64 * 512.0;
            [sum[0] as f64 / n, sum[1] as f64 / n, sum[2] as f64 / n]
        };
        let targets: [[f64; 3]; 3] = [
            [255.0, 0.0, 0.0],
            [128.0, 128.0, 128.0],
            [0.0, 128.0, 128.0],
        ];
        for (label, img) in [("explicit", &rgb), ("public fallback", &public_rgb)] {
            for (band, target) in targets.iter().enumerate() {
                let (start, end) = match band {
                    0 => (0, 170),
                    1 => (170, 340),
                    _ => (340, 512),
                };
                let r = band_means(img, start, end);
                for c in 0..3 {
                    assert!(
                        (r[c] - target[c]).abs() < 8.0,
                        "{label} band {band} channel {c} mean {:.1} vs target {}",
                        r[c],
                        target[c],
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod malformed_container_tests {
    use super::*;
    #[test]
    fn negative_stride_copies_rows_in_render_order() {
        let mut bytes = [1u8, 2, 99, 99, 3, 4, 99, 99, 5, 6];
        let mut pic = Dav1dPicture::default();
        pic.p.w = 2;
        pic.p.h = 3;
        pic.p.bpc = 8;
        pic.p.layout = Rav1dPixelLayout::I400.into();
        pic.data[0] = NonNull::new(unsafe { bytes.as_mut_ptr().add(8) }.cast());
        pic.stride[0] = -4;
        let frame = copy_picture_planes(&pic).unwrap();
        assert_eq!(frame.planes[0].as_deref(), Some(&[5, 6, 3, 4, 1, 2][..]));
        pic.stride[0] = 1;
        assert!(copy_picture_planes(&pic).is_err());
    }
    #[test]
    fn truncated_metadata_boxes_are_nonfatal() {
        // A valid box header with an incomplete FullBox body must not index [4..].
        let bytes = [0, 0, 0, 9, b'm', b'e', b't', b'a', 0];
        assert!(find_primary_nclx(&bytes).is_none());
        for len in 0..4 {
            assert!(!primary_has_property(&[0; 4][..len], 1, 1));
        }
    }
    #[test]
    fn extended_boxes_smaller_than_their_header_are_rejected() {
        let bytes = [0, 0, 0, 1, b'm', b'e', b't', b'a', 0, 0, 0, 0, 0, 0, 0, 8];
        assert_eq!(boxes(&bytes).count(), 0);
    }
}
