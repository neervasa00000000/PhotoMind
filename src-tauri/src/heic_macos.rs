//! macOS-native HEIC decoding through sips command-line tool.
//! Only compiled on macOS; other platforms will continue to skip HEIC thumbnails.

use image::DynamicImage;
use std::path::Path;

/// Decode HEIC on macOS using the native `sips` command-line tool.
/// This is simpler and more reliable than using FFI to Image I/O.
#[cfg(target_os = "macos")]
pub fn decode_heic_macos(path: &Path) -> Result<DynamicImage, String> {
    use std::process::Command;
    
    // Create a temporary PNG file
    let temp_dir = std::env::temp_dir();
    let temp_png = temp_dir.join(format!("photomind-heic-{}.png", uuid::Uuid::new_v4()));
    
    // Use sips to convert HEIC to PNG
    let output = Command::new("sips")
        .arg("-s")
        .arg("format")
        .arg("png")
        .arg(path)
        .arg("--out")
        .arg(&temp_png)
        .output()
        .map_err(|e| format!("Failed to run sips: {e}"))?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("sips conversion failed: {stderr}"));
    }
    
    // Load the PNG with the image crate
    let result = image::open(&temp_png)
        .map_err(|e| format!("Failed to load converted PNG: {e}"));
    
    // Clean up temporary file
    let _ = std::fs::remove_file(&temp_png);
    
    result
}

#[cfg(not(target_os = "macos"))]
pub fn decode_heic_macos(_path: &Path) -> Result<DynamicImage, String> {
    Err("HEIC decoding is only available on macOS".to_string())
}
