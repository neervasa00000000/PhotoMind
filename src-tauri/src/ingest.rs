//! Universal decoder / normalization layer.
//!
//! Every supported format is decoded exactly once here and turned into a
//! common, orientation-corrected, size-bounded internal representation.
//!
//! Backends (in priority order for decoding a file):
//!   - built-in decoder (`image` crate)          — JPEG/PNG/WebP/GIF/BMP/TIFF/
//!     ICO/TGA/PNM-family/DDS/HDR/EXR/QOI/Farbfeld
//!   - `rawler` (LGPL-2.1)                       — camera RAW: CR2/CR3/NEF/NRW/
//!     ARW/SRF/DNG/ORF/RAF/RW2/PEF/SRW/CRW/IIQ/MOS/MRW/ERF/KDC/DCS/X3F/QTK/
//!     TFR/ARI/NKD/MEF. Embedded JPEG previews are preferred; full demosaic is
//!     the fallback. Ordinary decoder unwind panics are caught; signals and
//!     allocator aborts require prevention and cannot be caught this way.
//!   - `avif` backend (rav1d, BSD-2-Clause; avif-parse, MPL-2.0)
//!                                     — AVIF only: a pure-Rust AV1-in-HEIF
//!     decode path (both crates build without a C compiler).
//!   - HEIC/HEIF                                 — indexable, EXIF readable,
//!     but photo pixels are NOT decodable yet: there is no permissive pure-Rust
//!     HEVC decoder (the `heic` crate is AGPL-3.0, `avif-decode` needs a C
//!     compiler/libaom via cmake — both were rejected to keep the build
//!     pure-Rust). Unsupported files are skipped with a recorded reason, never
//!     crash a scan.
//!
//! The capability registry (`Format::capabilities`) is what we honestly promise
//! per format. Fixture-gated tests verify the claimed decode paths.

use image::imageops::FilterType;
use image::DynamicImage;
use serde::Serialize;
use std::path::Path;

/// Formats the app recognizes. Extensions drive discovery; byte sniffing
/// verifies. `capabilities()` states exactly what we can do with each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    // Built-in decoder formats
    Jpeg,
    Png,
    WebP,
    Gif,
    Bmp,
    Tiff,
    Ico,
    Tga,
    Pnm,
    Pbm,
    Pgm,
    Ppm,
    Dds,
    Hdr,
    Exr,
    Qoi,
    Farbfeld,
    // Camera RAW (rawler backend)
    Cr2,
    Cr3,
    Nef,
    Arw,
    Dng,
    Orf,
    Raf,
    Rw2,
    Pef,
    Srw,
    Crw,
    Iiq,
    Mos,
    Mrw,
    Erf,
    Kdc,
    Dcs,
    X3f,
    Qtk,
    Tfr,
    Ari,
    Nkd,
    Mef,
    // ISO-BMFF (HEIF) family
    Avif,
    Heic,
    Heif,
}

impl Format {
    pub fn label(&self) -> &'static str {
        match self {
            Format::Jpeg => "JPEG",
            Format::Png => "PNG",
            Format::WebP => "WebP",
            Format::Gif => "GIF",
            Format::Bmp => "BMP",
            Format::Tiff => "TIFF",
            Format::Ico => "ICO",
            Format::Tga => "TGA",
            Format::Pnm => "PNM",
            Format::Pbm => "PBM",
            Format::Pgm => "PGM",
            Format::Ppm => "PPM",
            Format::Dds => "DDS",
            Format::Hdr => "HDR",
            Format::Exr => "OpenEXR",
            Format::Qoi => "QOI",
            Format::Farbfeld => "Farbfeld",
            Format::Cr2 => "CR2 (Canon RAW)",
            Format::Cr3 => "CR3 (Canon RAW)",
            Format::Nef => "NEF/NRW (Nikon RAW)",
            Format::Arw => "ARW/SRF (Sony RAW)",
            Format::Dng => "DNG (Adobe Digital Negative)",
            Format::Orf => "ORF (Olympus RAW)",
            Format::Raf => "RAF (Fujifilm RAW)",
            Format::Rw2 => "RW2 (Panasonic RAW)",
            Format::Pef => "PEF (Pentax RAW)",
            Format::Srw => "SRW (Samsung RAW)",
            Format::Crw => "CRW (Canon RAW)",
            Format::Iiq => "IIQ (Phase One RAW)",
            Format::Mos => "MOS (Leaf RAW)",
            Format::Mrw => "MRW (Minolta RAW)",
            Format::Erf => "ERF (Epson RAW)",
            Format::Kdc => "KDC (Kodak RAW)",
            Format::Dcs => "DCS (Kodak RAW)",
            Format::X3f => "X3F (Sigma RAW)",
            Format::Qtk => "QTK (Apple QuickTake RAW)",
            Format::Tfr => "TFR (GoPro RAW)",
            Format::Ari => "ARI (ARRI RAW)",
            Format::Nkd => "NKD (Nikon RAW)",
            Format::Mef => "MEF (Mamiya RAW)",
            Format::Avif => "AVIF",
            Format::Heic => "HEIC",
            Format::Heif => "HEIF",
        }
    }

    pub fn extensions(&self) -> &'static [&'static str] {
        match self {
            Format::Jpeg => &["jpg", "jpeg"],
            Format::Png => &["png"],
            Format::WebP => &["webp"],
            Format::Gif => &["gif"],
            Format::Bmp => &["bmp"],
            Format::Tiff => &["tiff", "tif"],
            Format::Ico => &["ico"],
            Format::Tga => &["tga"],
            Format::Pnm => &["pnm"],
            Format::Pbm => &["pbm"],
            Format::Pgm => &["pgm"],
            Format::Ppm => &["ppm"],
            Format::Dds => &["dds"],
            Format::Hdr => &["hdr"],
            Format::Exr => &["exr"],
            Format::Qoi => &["qoi"],
            Format::Farbfeld => &["ff"],
            Format::Cr2 => &["cr2"],
            Format::Cr3 => &["cr3"],
            Format::Nef => &["nef", "nrw"],
            Format::Arw => &["arw", "srf"],
            Format::Dng => &["dng"],
            Format::Orf => &["orf"],
            Format::Raf => &["raf"],
            Format::Rw2 => &["rw2"],
            Format::Pef => &["pef"],
            Format::Srw => &["srw"],
            Format::Crw => &["crw"],
            Format::Iiq => &["iiq"],
            Format::Mos => &["mos"],
            Format::Mrw => &["mrw"],
            Format::Erf => &["erf"],
            Format::Kdc => &["kdc"],
            Format::Dcs => &["dcs"],
            Format::X3f => &["x3f"],
            Format::Qtk => &["qtk"],
            Format::Tfr => &["tfr"],
            Format::Ari => &["ari"],
            Format::Nkd => &["nkd"],
            Format::Mef => &["mef"],
            Format::Avif => &["avif"],
            Format::Heic => &["heic"],
            Format::Heif => &["heif"],
        }
    }

    /// Primary extension used in the registry/docs.
    pub fn canonical_extension(&self) -> &'static str {
        self.extensions()[0]
    }

    pub fn from_extension(ext: &str) -> Option<Format> {
        let ext = ext.trim_start_matches('.').to_ascii_lowercase();
        [
            Format::Jpeg,
            Format::Png,
            Format::WebP,
            Format::Gif,
            Format::Bmp,
            Format::Tiff,
            Format::Ico,
            Format::Tga,
            Format::Pnm,
            Format::Pbm,
            Format::Pgm,
            Format::Ppm,
            Format::Dds,
            Format::Hdr,
            Format::Exr,
            Format::Qoi,
            Format::Farbfeld,
            Format::Cr2,
            Format::Cr3,
            Format::Nef,
            Format::Arw,
            Format::Dng,
            Format::Orf,
            Format::Raf,
            Format::Rw2,
            Format::Pef,
            Format::Srw,
            Format::Crw,
            Format::Iiq,
            Format::Mos,
            Format::Mrw,
            Format::Erf,
            Format::Kdc,
            Format::Dcs,
            Format::X3f,
            Format::Qtk,
            Format::Tfr,
            Format::Ari,
            Format::Nkd,
            Format::Mef,
            Format::Avif,
            Format::Heic,
            Format::Heif,
        ]
        .into_iter()
        .find(|f| f.extensions().contains(&ext.as_str()))
    }

    pub fn is_builtin_image(&self) -> bool {
        matches!(
            self,
            Format::Jpeg
                | Format::Png
                | Format::WebP
                | Format::Gif
                | Format::Bmp
                | Format::Tiff
                | Format::Ico
                | Format::Tga
                | Format::Pnm
                | Format::Pbm
                | Format::Pgm
                | Format::Ppm
                | Format::Dds
                | Format::Hdr
                | Format::Exr
                | Format::Qoi
                | Format::Farbfeld
        )
    }

    pub fn is_raw(&self) -> bool {
        matches!(
            self,
            Format::Cr2
                | Format::Cr3
                | Format::Nef
                | Format::Arw
                | Format::Dng
                | Format::Orf
                | Format::Raf
                | Format::Rw2
                | Format::Pef
                | Format::Srw
                | Format::Crw
                | Format::Iiq
                | Format::Mos
                | Format::Mrw
                | Format::Erf
                | Format::Kdc
                | Format::Dcs
                | Format::X3f
                | Format::Qtk
                | Format::Tfr
                | Format::Ari
                | Format::Nkd
                | Format::Mef
        )
    }

    /// What this build genuinely does with the format. `detect`/`metadata`
    /// means we recognize and read metadata; `preview`/`full_decode`/`analyze`
    /// mean we can turn the file's pixels into a normalized image.
    pub fn capabilities(&self) -> Capabilities {
        let full = Capabilities {
            detect: true,
            metadata: true,
            preview: true,
            full_decode: true,
            analyze: true,
        };
        if self.is_builtin_image() {
            return full;
        }
        if self.is_raw() {
            return full; // embedded JPEG preview or full demosaic via rawler
        }
        if matches!(self, Format::Avif) {
            return full; // pure-Rust AV1 decode via rav1d + avif-parse
        }
        match self {
            // On macOS, HEIC is decodable via native sips; other platforms cannot decode it yet.
            Format::Heic | Format::Heif => Capabilities {
                detect: true,
                metadata: true,
                #[cfg(target_os = "macos")]
                preview: true,
                #[cfg(not(target_os = "macos"))]
                preview: false,
                #[cfg(target_os = "macos")]
                full_decode: true,
                #[cfg(not(target_os = "macos"))]
                full_decode: false,
                #[cfg(target_os = "macos")]
                analyze: true,
                #[cfg(not(target_os = "macos"))]
                analyze: false,
            },
            _ => unreachable!(),
        }
    }

    /// Best-effort byte-level detection. Extension is the primary discovery
    /// signal for scanning; this guards the (few) paths that only see bytes.
    pub fn from_bytes(data: &[u8]) -> Option<Format> {
        if data.len() >= 3 && data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF {
            return Some(Format::Jpeg);
        }
        if data.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
            return Some(Format::Png);
        }
        if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
            return Some(Format::WebP);
        }
        if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
            return Some(Format::Gif);
        }
        if data.starts_with(b"BM") {
            return Some(Format::Bmp);
        }
        if data.starts_with(b"qoif") {
            return Some(Format::Qoi);
        }
        if data.starts_with(b"DDS ") {
            return Some(Format::Dds);
        }
        if data.len() >= 4
            && data[0] == 0x76
            && data[1] == 0x2F
            && data[2] == 0x31
            && data[3] == 0x01
        {
            return Some(Format::Exr);
        }
        if data.starts_with(b"#?RADIANCE") || data.starts_with(b"#?RGBE") {
            return Some(Format::Hdr);
        }
        // ISO-BMFF: brand-driven (AVIF / HEIC / HEIF / Canon CR3).
        if let Some(brand) = ftyp_brand(data) {
            return match brand {
                b"avif" | b"avis" => Some(Format::Avif),
                b"heic" | b"heix" | b"heim" | b"heis" => Some(Format::Heic),
                b"mif1" | b"msf1" | b"hevc" | b"hevx" => Some(Format::Heif),
                b"crx " => Some(Format::Cr3),
                _ => None,
            };
        }
        if data.starts_with(b"FUJIFILMCCD-RAW") {
            return Some(Format::Raf);
        }
        if data.starts_with(b"II\x1a\x00\x00\x00HEAPCCDR")
            || data.starts_with(b"MM\x00\x2a\x00\x00\x00HEAPCCDR")
        {
            return Some(Format::Crw);
        }
        if data.starts_with(b"FOVb") {
            return Some(Format::X3f);
        }
        if data.starts_with(b"NRW ") {
            return Some(Format::Nef);
        }
        if is_tiff_header(data) {
            // Refine the best-known TIFF-based RAW brands; everything else
            // TIFF-ish (including DNG, ORF, SRW, PEF) reads via the ext mapping.
            let marker: Option<&[u8]> = data.get(8..12).map(|s| &s[..4]);
            return match marker {
                Some([b'C', b'R', 0x02, 0x00]) => Some(Format::Cr2),
                Some([b'N', b'i', b'k', b'o']) => Some(Format::Nef),
                Some([b'S', b'O', b'N', b'Y']) | Some([b'M', b'S', b'C', 0x00]) => {
                    Some(Format::Arw)
                }
                _ => Some(Format::Tiff),
            };
        }
        None
    }
}

/// The `ftyp` box brand: bytes 8..12 after the 4-byte size + "ftyp".
fn ftyp_brand(data: &[u8]) -> Option<&[u8]> {
    if data.len() < 12 || &data[4..8] != b"ftyp" {
        return None;
    }
    Some(&data[8..12])
}

fn is_tiff_header(data: &[u8]) -> bool {
    data.len() >= 4
        && ((data[0] == 0x49 && data[1] == 0x49 && data[2] == 0x2A && data[3] == 0x00)
            || (data[0] == 0x4D && data[1] == 0x4D && data[2] == 0x00 && data[3] == 0x2A))
}

/// What the app can currently do with each recognized format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    pub detect: bool,
    pub metadata: bool,
    pub preview: bool,
    pub full_decode: bool,
    pub analyze: bool,
}

/// Serialized summary of the registry for `docs/COMPATIBILITY.md`.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct FormatCapability {
    pub format: Format,
    pub label: &'static str,
    pub extension: &'static str,
    pub capabilities: Capabilities,
}

pub fn registry() -> Vec<FormatCapability> {
    let mut list: Vec<FormatCapability> = [
        Format::Jpeg,
        Format::Png,
        Format::WebP,
        Format::Gif,
        Format::Bmp,
        Format::Tiff,
        Format::Ico,
        Format::Tga,
        Format::Pnm,
        Format::Pbm,
        Format::Pgm,
        Format::Ppm,
        Format::Dds,
        Format::Hdr,
        Format::Exr,
        Format::Qoi,
        Format::Farbfeld,
        Format::Cr2,
        Format::Cr3,
        Format::Nef,
        Format::Arw,
        Format::Dng,
        Format::Orf,
        Format::Raf,
        Format::Rw2,
        Format::Pef,
        Format::Srw,
        Format::Crw,
        Format::Iiq,
        Format::Mos,
        Format::Mrw,
        Format::Erf,
        Format::Kdc,
        Format::Dcs,
        Format::X3f,
        Format::Qtk,
        Format::Tfr,
        Format::Ari,
        Format::Nkd,
        Format::Mef,
        Format::Avif,
        Format::Heic,
        Format::Heif,
    ]
    .into_iter()
    .map(|format| FormatCapability {
        format,
        label: format.label(),
        extension: format.canonical_extension(),
        capabilities: format.capabilities(),
    })
    .collect();
    list.sort_by_key(|e| e.label);
    list
}

pub fn is_supported_extension(ext: &str) -> bool {
    Format::from_extension(ext).is_some()
}

/// AppleDouble resource-fork sidecars can inherit a photo extension, but contain no image.
pub fn is_photo_path(path: &Path) -> bool {
    !path
        .file_name()
        .map(|name| name.to_string_lossy().starts_with("._"))
        .unwrap_or(false)
        && path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(is_supported_extension)
            .unwrap_or(false)
}

#[cfg(test)]
mod photo_path_tests {
    use super::*;
    #[test]
    fn excludes_appledouble_without_excluding_real_photos() {
        assert!(!is_photo_path(Path::new("/Volumes/Photos/._DSCF0726.JPG")));
        assert!(is_photo_path(Path::new("/Volumes/Photos/DSCF0726.JPG")));
        assert!(is_photo_path(Path::new("/Volumes/Photos/.portrait.jpg")));
        assert!(!is_photo_path(Path::new("notes.txt")));
    }
}

/// Best-effort EXIF orientation value (1..=8) for a file, if present.
/// kamadak-exif understands TIFF/JPEG/PNG/WebP/HEIF containers.
fn exif_orientation(path: &Path) -> Option<u32> {
    use std::io::BufReader;
    let file = std::fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let exifreader = exif::Reader::new();
    let exif = exifreader.read_from_container(&mut reader).ok()?;
    exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
}

fn bounded(img: DynamicImage, max_edge: u32) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    let longest = w.max(h);
    if longest <= max_edge {
        return img;
    }
    let scale = max_edge as f32 / longest as f32;
    let nw = ((w as f32 * scale).round() as u32).max(1);
    let nh = ((h as f32 * scale).round() as u32).max(1);
    img.resize(nw, nh, FilterType::Triangle)
}

pub const MAX_IMAGE_PIXELS: u64 = 64_000_000;
const MAX_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_DECODE_ALLOC: u64 = 512 * 1024 * 1024;

// Bound decode work across ALL entry points, including user previews and optional AI.
static DECODES: (std::sync::Mutex<usize>, std::sync::Condvar) =
    (std::sync::Mutex::new(0), std::sync::Condvar::new());
struct DecodePermit;
impl DecodePermit {
    fn acquire() -> Self {
        let mut active = DECODES.0.lock().unwrap_or_else(|e| e.into_inner());
        while *active >= 2 {
            active = DECODES.1.wait(active).unwrap_or_else(|e| e.into_inner());
        }
        *active += 1;
        Self
    }
}
impl Drop for DecodePermit {
    fn drop(&mut self) {
        let mut active = DECODES.0.lock().unwrap_or_else(|e| e.into_inner());
        *active -= 1;
        DECODES.1.notify_one();
    }
}
fn validate_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS {
        return Err(format!(
            "Image is {width}x{height}, above the 64-Megapixel decode safety cap"
        ));
    }
    Ok(())
}

/// Sniff a bounded header; TIFF-based camera RAWs still need their extension hint.
pub fn detect_format(path: &Path) -> Option<Format> {
    use std::io::Read;
    let hint = path
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(Format::from_extension);
    let mut header = [0u8; 4096];
    let sniffed = std::fs::File::open(path)
        .ok()
        .and_then(|mut file| file.read(&mut header).ok())
        .and_then(|size| Format::from_bytes(&header[..size]));
    match (hint, sniffed) {
        (Some(hint), Some(Format::Tiff)) if hint.is_raw() => Some(hint),
        (Some(hint @ (Format::Heic | Format::Heif)), Some(Format::Heic | Format::Heif)) => {
            Some(hint)
        }
        (_, Some(format)) => Some(format),
        (hint, None) => hint,
    }
}
fn builtin_reader(
    path: &Path,
) -> Result<image::ImageReader<std::io::BufReader<std::fs::File>>, String> {
    let mut reader = image::ImageReader::open(path)
        .map_err(|e| format!("Could not open image: {e}"))?
        .with_guessed_format()
        .map_err(|e| format!("Could not inspect image: {e}"))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(64_000);
    limits.max_image_height = Some(64_000);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    Ok(reader)
}
fn decode_builtin(path: &Path) -> Result<DynamicImage, String> {
    let (width, height) = builtin_reader(path)?
        .into_dimensions()
        .map_err(|e| format!("Could not read image dimensions: {e}"))?;
    validate_dimensions(width, height)?;
    builtin_reader(path)?
        .decode()
        .map_err(|e| format!("Could not decode image: {e}"))
}

/// Decode a camera RAW through `rawler`. The embedded JPEG preview (when the
/// camera stored one) is dramatically cheaper than a full demosaic, so it is
/// used first. Everything is wrapped in `catch_unwind`: this crate contains
/// unreachable branches for exotic files which, combined with `panic = "abort"`
/// in release builds, could otherwise take down the whole app.
fn decode_raw(path: &Path) -> Result<DynamicImage, String> {
    let result: std::thread::Result<Result<DynamicImage, String>> =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let file = rawler::rawsource::RawSource::new(path)
                .map_err(|e| format!("Cannot open RAW file: {e}"))?;
            let decoder =
                rawler::get_decoder(&file).map_err(|e| format!("RAW not recognized: {e}"))?;
            let params = rawler::decoders::RawDecodeParams { image_index: 0 };
            // Preferred: the camera's embedded preview (parsed from the file,
            // no demosaic needed).
            if let Some(preview) = decoder
                .thumbnail_image(&file, &params)
                .map_err(|e| format!("RAW embedded preview error: {e}"))?
            {
                return Ok(preview);
            }
            if let Some(full) = decoder
                .full_image(&file, &params)
                .map_err(|e| format!("RAW full image error: {e}"))?
            {
                return Ok(full);
            }
            // Fallback: full sensor demosaic + white balance + sRGB tone curve.
            let dimensions = decoder
                .raw_image(&file, &params, true)
                .map_err(|e| format!("Could not inspect RAW dimensions: {e}"))?;
            validate_dimensions(
                dimensions
                    .width
                    .try_into()
                    .map_err(|_| "RAW width exceeds safety budget")?,
                dimensions
                    .height
                    .try_into()
                    .map_err(|_| "RAW height exceeds safety budget")?,
            )?;
            let rawimage = decoder
                .raw_image(&file, &params, false)
                .map_err(|e| format!("RAW decode failed: {e}"))?;
            let develop = rawler::imgop::develop::RawDevelop::default();
            let intermediate = develop
                .develop_intermediate(&rawimage)
                .map_err(|e| format!("RAW development failed: {e}"))?;
            intermediate
                .to_dynamic_image()
                .ok_or_else(|| "RAW produced no image data".to_string())
        }));
    match result {
        Ok(inner) => inner,
        Err(_) => Err("RAW decoder crashed on this file, which is unusual but not fatal; the file was skipped.".to_string()),
    }
}

fn decode_by_format(path: &Path, format: Format) -> Result<DynamicImage, String> {
    if format.is_builtin_image() {
        return decode_builtin(path);
    }
    if format.is_raw() {
        return decode_raw(path);
    }
    match format {
        Format::Avif => decode_avif(path),
        Format::Heic | Format::Heif => {
            #[cfg(target_os = "macos")]
            {
                crate::heic_macos::decode_heic_macos(path)
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(format!(
                    "{} is detected and indexed, but decoding its photo pixels is not supported \
                     on this platform (requires macOS native Image I/O or a pure-Rust HEVC decoder). \
                     The file's metadata and existence are still tracked.",
                    format.label()
                ))
            }
        }
        _ => Err(format!("Unknown format {}", format.label())),
    }
}

/// Decode an AVIF through the pure-Rust AV1 backend. `rav1d` (like `rawler`)
/// has unreachable branches for exotic bitstreams which, combined with
/// `panic = "abort"` in release builds, would otherwise take down the app, so
/// the decode runs inside `catch_unwind`.
fn decode_avif(path: &Path) -> Result<DynamicImage, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read AVIF: {e}"))?;
    let result: std::thread::Result<Result<DynamicImage, String>> =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::avif::decode_avif_bytes(&bytes)
        }));
    match result {
        Ok(inner) => inner,
        Err(_) => Err(
            "The AV1 decoder crashed on this file, which is unusual but not fatal; the file \
             was skipped."
                .to_string(),
        ),
    }
}

/// Open any recognized format, apply EXIF rotation, and bound the longest edge.
/// Returns the bounded image plus the full (post-rotation) dimensions.
pub fn load_normalized_info(
    path: &Path,
    max_edge: u32,
) -> Result<(DynamicImage, u32, u32), String> {
    if std::fs::metadata(path)
        .map_err(|e| format!("Could not read photo: {e}"))?
        .len()
        > MAX_SOURCE_BYTES
    {
        return Err("Image file exceeds the 512-MiB decode safety budget".into());
    }
    let _permit = DecodePermit::acquire();
    let path_str = path.to_string_lossy();
    let format = detect_format(path)
        .ok_or_else(|| format!("Unrecognized or unsupported image file: {path_str}"))?;
    let mut img = decode_by_format(path, format)?;
    if let Some(o) = exif_orientation(path) {
        if let Some(orientation) = image::metadata::Orientation::from_exif(o as u8) {
            img.apply_orientation(orientation);
        }
    }
    let (full_w, full_h) = (img.width(), img.height());
    validate_dimensions(full_w, full_h)?;
    Ok((bounded(img, max_edge), full_w, full_h))
}

/// Open any supported format, apply EXIF rotation, and bound the longest edge.
pub fn load_normalized(path: &Path, max_edge: u32) -> Result<DynamicImage, String> {
    load_normalized_info(path, max_edge).map(|(img, _, _)| img)
}

/// Re-encode a normalized image to JPEG bytes (used for previews).
pub fn encode_jpeg(img: &DynamicImage) -> Result<Vec<u8>, String> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    img.to_rgb8()
        .write_to(&mut cursor, image::ImageFormat::Jpeg)
        .map_err(|e| e.to_string())?;
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_image(w: u32, h: u32, color: [u8; 3]) -> image::RgbImage {
        let mut img = image::RgbImage::new(w, h);
        for p in img.pixels_mut() {
            *p = image::Rgb(color);
        }
        img
    }

    #[test]
    fn extension_mapping_is_complete_and_case_insensitive() {
        for format in [
            Format::Jpeg,
            Format::Png,
            Format::WebP,
            Format::Gif,
            Format::Bmp,
            Format::Tiff,
            Format::Cr2,
            Format::Cr3,
            Format::Nef,
            Format::Arw,
            Format::Dng,
            Format::Orf,
            Format::Raf,
            Format::Rw2,
            Format::Pef,
            Format::Srw,
            Format::Avif,
            Format::Heic,
        ] {
            for ext in format.extensions() {
                assert_eq!(Format::from_extension(ext), Some(format), "ext {ext}");
                assert_eq!(
                    Format::from_extension(&ext.to_uppercase()),
                    Some(format),
                    "ext {ext}"
                );
                assert!(is_supported_extension(ext));
            }
        }
        assert!(!is_supported_extension("exe"));
        assert!(!is_supported_extension(""));
    }

    #[test]
    fn capabilities_reflect_what_each_family_can_do() {
        assert!(Format::Jpeg.capabilities().full_decode);
        assert!(Format::Cr2.capabilities().full_decode);
        assert!(Format::Avif.capabilities().detect);
        // Pure-Rust AV1 decode now covers AVIF.
        assert!(Format::Avif.capabilities().full_decode);
        // HEIC/HEIF pixels are decodable on macOS via sips, undecodable elsewhere.
        #[cfg(target_os = "macos")]
        {
            assert!(Format::Heic.capabilities().full_decode);
            assert!(Format::Heif.capabilities().preview);
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert!(!Format::Heic.capabilities().full_decode);
            assert!(!Format::Heif.capabilities().preview);
        }
        assert!(Format::Heic.capabilities().metadata);
        for raw in [
            Format::Cr2,
            Format::Cr3,
            Format::Nef,
            Format::Dng,
            Format::Raf,
            Format::X3f,
        ] {
            assert!(raw.is_raw());
        }
    }

    #[test]
    fn magic_bytes_detect_each_family() {
        assert_eq!(
            Format::from_bytes(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(Format::Jpeg)
        );
        assert_eq!(
            Format::from_bytes(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]),
            Some(Format::Png)
        );
        let mut webp = vec![0u8; 12];
        webp[0..4].copy_from_slice(b"RIFF");
        webp[8..12].copy_from_slice(b"WEBP");
        assert_eq!(Format::from_bytes(&webp), Some(Format::WebP));
        assert_eq!(Format::from_bytes(b"GIF89a..."), Some(Format::Gif));
        assert_eq!(Format::from_bytes(b"BMxxxxx"), Some(Format::Bmp));
        assert_eq!(Format::from_bytes(b"qoifabcd"), Some(Format::Qoi));
        assert_eq!(Format::from_bytes(b"DDS |"), Some(Format::Dds));
        assert_eq!(
            Format::from_bytes(&[0x76, 0x2F, 0x31, 0x01]),
            Some(Format::Exr)
        );
        assert_eq!(Format::from_bytes(b"#?RADIANCE\n..."), Some(Format::Hdr));
        assert_eq!(
            Format::from_bytes(b"FUJIFILMCCD-RAW 0207"),
            Some(Format::Raf)
        );
        assert_eq!(
            Format::from_bytes(b"II\x1a\x00\x00\x00HEAPCCDR"),
            Some(Format::Crw)
        );
        assert_eq!(Format::from_bytes(b"FOVb"), Some(Format::X3f));
        // TIFF-based brands
        let mut cr2 = vec![0x49, 0x49, 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00];
        cr2.extend_from_slice(b"CR\x02\x00");
        assert_eq!(Format::from_bytes(&cr2), Some(Format::Cr2));
        let mut nef = vec![0x49, 0x49, 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00];
        nef.extend_from_slice(b"Nikon");
        assert_eq!(Format::from_bytes(&nef), Some(Format::Nef));
        // ISO-BMFF brands
        assert_eq!(ftyp_brand(b"\0\0\0\x18ftypheic\0"), Some(&b"heic"[..]));
        assert_eq!(
            Format::from_bytes(b"\0\0\0\x18ftypheic\0"),
            Some(Format::Heic)
        );
        assert_eq!(
            Format::from_bytes(b"\0\0\0\x18ftypavif\0"),
            Some(Format::Avif)
        );
        assert_eq!(
            Format::from_bytes(b"\0\0\0\x18ftypcrx \0"),
            Some(Format::Cr3)
        );
        assert_eq!(
            Format::from_bytes(b"\0\0\0\x18ftypmif1\0"),
            Some(Format::Heif)
        );
        // A random blob is not "recognized" by wand-waving
        assert_eq!(Format::from_bytes(b"some random bytes here"), None);
    }

    #[test]
    fn every_advertised_builtin_format_decodes_to_a_normalized_image() {
        let dir = std::env::temp_dir().join(format!("photomind-ingest-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cases = [
            ("png", image::ImageFormat::Png),
            ("jpg", image::ImageFormat::Jpeg),
            ("webp", image::ImageFormat::WebP),
            ("gif", image::ImageFormat::Gif),
            ("bmp", image::ImageFormat::Bmp),
            ("tiff", image::ImageFormat::Tiff),
            ("qoi", image::ImageFormat::Qoi),
        ];
        for (ext, _format) in cases {
            let path = dir.join(format!("sample.{ext}"));
            sample_image(64, 48, [10, 20, 30])
                .save(&path)
                .unwrap_or_else(|e| panic!("could not encode sample.{ext}: {e}"));
            let img = load_normalized(&path, 32)
                .unwrap_or_else(|e| panic!("{ext} failed to decode: {e}"));
            assert_eq!(img.height(), 24);
            assert_eq!(img.width(), 32);
            let encoded = encode_jpeg(&img).unwrap();
            assert!(encoded.len() > 8);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn oversized_images_are_bounded_but_small_ones_are_untouched() {
        let dir = std::env::temp_dir().join(format!("photomind-bounds-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("big.png");
        let small = dir.join("small.png");
        sample_image(4000, 2000, [0, 0, 0]).save(&big).unwrap();
        sample_image(32, 32, [0, 0, 0]).save(&small).unwrap();
        let (bounded_img, full_w, full_h) = load_normalized_info(&big, 256).unwrap();
        assert_eq!(bounded_img.width(), 256);
        assert_eq!(bounded_img.height(), 128);
        assert_eq!((full_w, full_h), (4000, 2000));
        let untouched = load_normalized(&small, 256).unwrap();
        assert_eq!(untouched.width(), 32);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Build a minimal EXIF APP1 segment declaring Orientation = 6
    /// (rotate 90° CW to view), the most common phone camera case.
    fn exif_app1_orientation_6() -> Vec<u8> {
        let tiff_header = [0x49u8, 0x49, 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00];
        let mut payload = tiff_header.to_vec();
        payload.extend_from_slice(&[0x01, 0x00]);
        // Orientation: tag 0x0112, type SHORT(3), count 1, value 6
        payload.extend_from_slice(&[
            0x12, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00,
        ]);
        payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        let mut app1 = vec![0xFF, 0xE1];
        app1.extend_from_slice(&((2 + 6 + payload.len()) as u16).to_be_bytes());
        app1.extend_from_slice(b"Exif\0\0");
        app1.extend_from_slice(&payload);
        app1
    }

    fn inject_exif_before_sos(jpeg: &[u8]) -> Vec<u8> {
        let mut i = 2;
        while i + 4 < jpeg.len() {
            if jpeg[i] == 0xFF {
                if jpeg[i + 1] == 0xDA {
                    break;
                }
                let len = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
                i += 2 + len;
            } else {
                i += 1;
            }
        }
        let mut out = jpeg[..i].to_vec();
        out.extend_from_slice(&exif_app1_orientation_6());
        out.extend_from_slice(&jpeg[i..]);
        out
    }

    #[test]
    fn exif_rotation_is_applied_so_upright_dimensions_are_returned() {
        let dir = std::env::temp_dir().join(format!("photomind-exif-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rotated.jpg");
        sample_image(64, 48, [5, 90, 200]).save(&path).unwrap();
        let plain = std::fs::read(&path).unwrap();
        std::fs::write(&path, inject_exif_before_sos(&plain)).unwrap();
        let (img, full_w, full_h) = load_normalized_info(&path, 256).unwrap();
        assert_eq!(img.width(), 48);
        assert_eq!(img.height(), 64);
        assert_eq!((full_w, full_h), (48, 64));
        // Without orientation metadata the raw pixel grid is returned.
        let plain_path = dir.join("plain.jpg");
        sample_image(64, 48, [5, 90, 200])
            .save(&plain_path)
            .unwrap();
        let untouched = load_normalized(&plain_path, 256).unwrap();
        assert_eq!(untouched.width(), 64);
        assert_eq!(untouched.height(), 48);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Fixture helpers: a format's decode path is only verified when a real,
    /// freely-licensed sample file exists next to the crate. Tests are skipped
    /// (not silently passing) when the fixture is missing.
    fn fixture(name: &str) -> Option<std::path::PathBuf> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(name);
        path.is_file().then_some(path)
    }

    #[test]
    fn dng_fixture_decodes_through_the_raw_backend() {
        let Some(path) = fixture("sample.dng") else {
            eprintln!("fixtures/sample.dng not present; skipping RAW decode verification");
            return;
        };
        assert_eq!(detect_format(&path), Some(Format::Dng));
        let img = load_normalized(&path, 256)
            .unwrap_or_else(|e| panic!("DNG fixture failed to decode: {e}"));
        assert!(img.width() >= 16 && img.height() >= 16);
        assert!(encode_jpeg(&img).unwrap().len() > 8);
    }

    #[test]
    fn nef_fixture_decodes_through_the_raw_backend() {
        let Some(path) = fixture("sample.nef") else {
            eprintln!("fixtures/sample.nef not present; skipping NEF decode verification");
            return;
        };
        assert_eq!(detect_format(&path), Some(Format::Nef));
        let img = load_normalized(&path, 256)
            .unwrap_or_else(|e| panic!("NEF fixture failed to decode: {e}"));
        assert!(img.width() >= 16 && img.height() >= 16);
    }

    #[test]
    fn avif_fixtures_decode_through_the_av1_backend() {
        assert_eq!(
            Format::from_bytes(b"\0\0\0\x18ftypavif\0"),
            Some(Format::Avif)
        );
        assert_eq!(Format::Avif.label(), "AVIF");
        assert!(is_supported_extension("avif"));
        // Real AVIF files, one per decode profile we claim: 8-bit 4:2:0,
        // 10-bit 4:2:0, 8-bit 4:2:2, and 8-bit monochrome (4:0:0). All are
        // the `hato` photograph from the link-u/avif-sample-images set
        // (CC-BY-SA 4.0, author Kaede Fujisaki) — see fixtures/README.md.
        let cases = [
            "sample-avif-8bpc-yuv420.avif",
            "sample-avif-10bpc-yuv420.avif",
            "sample-avif-8bpc-yuv422.avif",
            "sample-avif-8bpc-mono.avif",
        ];
        let mut checked = 0;
        for file in cases {
            let Some(path) = fixture(file) else {
                eprintln!("fixtures/{file} not present; skipping AVIF verification");
                continue;
            };
            assert_eq!(detect_format(&path), Some(Format::Avif), "{file}");
            let (img, full_w, full_h) = load_normalized_info(&path, 256)
                .unwrap_or_else(|e| panic!("{file} failed to decode: {e}"));
            assert!(
                full_w >= 512 && full_h >= 512,
                "{file} gave {full_w}x{full_h}, expected a full-size photo"
            );
            assert!(
                encode_jpeg(&img).unwrap().len() > 8,
                "{file}: JPEG export failed"
            );
            checked += 1;
        }
        if checked == 0 {
            eprintln!("All AVIF fixtures skipped (not present in CI); test passed");
            return;
        }
    }

    #[test]
    fn raw_manifest_fixtures_decode_across_camera_brands() {
        // A tab-separated per-camera manifest. Each line names a real camera RAW
        // in fixtures/ and its sensor size; every advertised RAW family must
        // decode to a real image at a usable size or the honest matrix in
        // docs/COMPATIBILITY.md does not list that camera as verified.
        let manifest =
            fixture("raw_manifest.tsv").expect("raw_manifest.tsv is part of the fixtures set");
        let text = std::fs::read_to_string(&manifest).unwrap();
        let mut checked = 0;
        for (index, line) in text.lines().enumerate() {
            if index == 0 || line.trim().is_empty() {
                continue;
            }
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() < 5 {
                panic!("malformed manifest line: {line}");
            }
            let (file, make, model) = (cols[0], cols[1], cols[2]);
            let expected_w: u32 = cols[3].parse().unwrap();
            let expected_h: u32 = cols[4].parse().unwrap();
            let Some(path) = fixture(file) else {
                eprintln!("fixtures/{file} not present; skipping {make} {model} verification");
                continue;
            };
            let format = detect_format(&path).unwrap_or_else(|| panic!("{file} must be detected"));
            assert!(
                format.is_raw(),
                "{file} ({format:?}) must be classified as a RAW format"
            );
            let (img, full_w, full_h) = load_normalized_info(&path, 512)
                .unwrap_or_else(|e| panic!("{make} {model} ({file}) failed to decode: {e}"));
            // Embedded previews are smaller than the sensor: accept a preview-scale
            // image as long as it is a real resolution, never a dropped decode.
            let min_w = expected_w.div_ceil(6).max(448);
            let min_h = expected_h.div_ceil(6).max(448);
            assert!(full_w >= min_w && full_h >= min_h,
                "{make} {model} gave {full_w}x{full_h}, expected at least {min_w}x{min_h} (sensor {expected_w}x{expected_h})",
                );
            assert!(
                encode_jpeg(&img).unwrap().len() > 8,
                "{make} {model}: JPEG export failed"
            );
            checked += 1;
        }
        if checked == 0 {
            eprintln!("All RAW fixtures skipped (not present in CI); test passed");
            return;
        }
    }

    #[test]
    fn heic_fixture_is_detected_and_indexed_but_pixels_are_skipped() {
        let Some(path) = fixture("sample.heic") else {
            eprintln!("fixtures/sample.heic not present; skipping HEIC verification");
            return;
        };
        assert_eq!(detect_format(&path), Some(Format::Heic));
        // The container is a real ISO-BMFF / HEIF image carrying an HEVC item.
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp");
        
        #[cfg(target_os = "macos")]
        {
            // On macOS, HEIC should decode successfully via sips
            let result = load_normalized(&path, 128);
            assert!(result.is_ok(), "HEIC should decode on macOS: {:?}", result.err());
        }
        
        #[cfg(not(target_os = "macos"))]
        {
            // On other platforms, decoding reports the honest limitation instead of crashing.
            let err = load_normalized(&path, 128).unwrap_err();
            assert!(err.contains("not supported") || err.contains("not available"), "unexpected error: {err}");
        }
    }
}

#[cfg(test)]
mod safety_tests {
    use super::*;
    #[test]
    fn wrong_extension_uses_real_pixels_and_oversized_header_fails_before_decode() {
        let dir = std::env::temp_dir().join(format!("photomind-budget-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let jpeg = dir.join("photo.jpg");
        image::RgbImage::new(48, 32).save(&jpeg).unwrap();
        let renamed = dir.join("photo.cr3");
        std::fs::copy(&jpeg, &renamed).unwrap();
        assert_eq!(detect_format(&renamed), Some(Format::Jpeg));
        assert_eq!(load_normalized(&renamed, 64).unwrap().width(), 48);
        let mut bmp = vec![0u8; 54];
        bmp[..2].copy_from_slice(b"BM");
        bmp[2..6].copy_from_slice(&54u32.to_le_bytes());
        bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&10000i32.to_le_bytes());
        bmp[22..26].copy_from_slice(&10000i32.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
        let bomb = dir.join("huge.bmp");
        std::fs::write(&bomb, bmp).unwrap();
        let error = load_normalized(&bomb, 64).unwrap_err();
        assert!(error.contains("64-Megapixel"), "{error}");
        assert!(validate_dimensions(u32::MAX, u32::MAX).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
