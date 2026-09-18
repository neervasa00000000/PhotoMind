# PhotoMind v0.1.0 Release Notes

**Release Date:** September 19, 2026

## Overview

This is the first public release of PhotoMind, a free, open-source, local-first photo culling and selection application for photographers.

## What's New

### Core Features

- ✅ **Folder Scanning** – Automatically scan photo folders and subfolders
- ✅ **Exact Duplicate Detection** – Find identical files using SHA-256 hashing
- ✅ **Visual Match Detection** – Group visually similar photos for review
- ✅ **Moment Grouping** – Automatically organize photos by capture time
- ✅ **Technical Quality Analysis** – Measure sharpness, exposure, clipping, saturation
- ✅ **Smart Recommendations** – Get Keep/Review suggestions based on local analysis
- ✅ **Safe Bin System** – Move photos to Bin with easy recovery to original locations
- ✅ **Face Detection** – Local face analysis using YuNet model

### Privacy & Local-First

- **No account required**
- **No cloud upload**
- **No API keys needed**
- **Works completely offline**
- **All processing on your device**

### Supported Formats

- **JPEG** (.jpg, .jpeg)
- **PNG** (.png)
- **RAW formats:**
  - Canon: .cr2, .cr3
  - Nikon: .nef
  - Sony: .arw
  - Fuji: .raf
  - Olympus: .orf
  - Panasonic: .rw2
  - Pentax: .pef
  - Samsung: .srw
  - Adobe: .dng
- **HEIC/HEIF** (.heic, .heif) – *pixel analysis incomplete*
- **AVIF** (.avif)

## Platform Support

### macOS ✅

- **Minimum:** macOS 11 (Big Sur) or later
- **Architecture:** Universal binary (Intel + Apple Silicon)
- **Installer:** DMG

### Windows ⏳

Coming in future release

### Linux ⏳

Coming in future release

## Known Limitations

### Experimental Features

**Eye-State Analysis**
- Eye detection is currently **experimental**
- Does **not affect ranking** while validation is ongoing
- Results are shown in UI but not used for recommendations
- Will be enabled after sufficient validation (`ENABLE_EYE_BASED_RANKING = false`)

### Format Limitations

**HEIC/HEIF**
- File detection and metadata work
- Pixel analysis incomplete (may not generate previews)
- RAW-embedded HEIC works through RAW decoder

**AVIF**
- Full support for 8-bit and 16-bit
- Limited color space support (sRGB, Rec.709, Rec.2020)
- Studio range and full range both supported

### macOS Specific

**Unsigned Application**
- This release is **not code-signed or notarized**
- macOS will show security warnings on first launch
- See installation instructions for workaround
- Right-click → Open to bypass Gatekeeper
- See [docs/MACOS_RELEASE.md](docs/MACOS_RELEASE.md) for details

## Installation

### macOS

1. Download `PhotoMind_0.1.0_macOS.dmg`
2. Open the DMG file
3. Drag PhotoMind to Applications folder
4. **Right-click** PhotoMind in Applications and choose **"Open"**
5. Click "Open" in the security dialog

**Important:** Do not double-click on first launch—use right-click → Open to bypass unsigned app warning.

## Requirements

- **macOS:** 11.0 or later (Big Sur+)
- **Disk Space:** ~100 MB for application
- **Memory:** 4 GB RAM recommended (handles large libraries better)
- **No additional software required:**
  - ❌ No Node.js
  - ❌ No Rust
  - ❌ No npm
  - ❌ No Ollama (unless using optional AI features)
  - ❌ No Python
  - ❌ No API keys

## Getting Started

1. **Launch PhotoMind** from Applications
2. **Choose a folder** using the "Choose Folder" button
3. **Wait for analysis** – PhotoMind will scan and analyze automatically
4. **Review results:**
   - **Exact Duplicates** – Identical files
   - **Visual Matches** – Similar photos needing review
   - **Moments** – Time-based groups
5. **Take action:**
   - Mark keepers with "Keep this photo"
   - Move unwanted photos to Bin
   - Recover from Bin if needed

## Privacy & Security

PhotoMind respects your privacy:

- **No telemetry** – We don't collect usage data
- **No analytics** – No tracking
- **No network requests** – Works offline (except optional Ollama)
- **No cloud processing** – All analysis local
- **Open source** – Code is public and auditable

## Optional Advanced Features

### Ollama Integration (Experimental)

PhotoMind can optionally use local Ollama for advanced AI analysis:

- **Requires:** Ollama installed separately
- **Requires:** Vision-capable model
- **Uses:** Local HTTP connection (stays private)
- **Not required** for core functionality

## Technical Details

### Architecture

- **Frontend:** React + TypeScript + Vite + Tailwind CSS
- **Backend:** Rust + Tauri 2.0
- **Database:** SQLite (local)
- **Face Detection:** YuNet (ONNX) – MIT licensed
- **Image Processing:** Rust libraries (rawler, image, rav1d)

### Performance

- **Scanning:** 2 concurrent image workers
- **Hashing:** SHA-256 for duplicate detection
- **Perceptual hashing:** For visual similarity
- **Preview caching:** Content-addressed to avoid duplicates

## License

- **PhotoMind source code:** MIT License
- **YuNet face detection model:** MIT License (Shiqi Yu)
- **Third-party components:** See [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md)

## Links

- **Repository:** https://github.com/neervasa00000000/PhotoMind
- **Issues:** https://github.com/neervasa00000000/PhotoMind/issues
- **Releases:** https://github.com/neervasa00000000/PhotoMind/releases

## What's Next

### Planned for Future Releases

- Windows installer
- Linux packages (deb, AppImage)
- Code signing & notarization (macOS)
- Eye-state validation completion
- HEIC/HEIF pixel analysis
- Additional face/photo analysis features
- Export recommendations
- Bulk operations improvements

## Feedback

This is an early release. Please report issues:

- **Bug reports:** https://github.com/neervasa00000000/PhotoMind/issues
- **Feature requests:** https://github.com/neervasa00000000/PhotoMind/issues
- **Questions:** https://github.com/neervasa00000000/PhotoMind/discussions

## Credits

PhotoMind is built with:
- YuNet face detection model (MIT) by Shiqi Yu
- ONNX Runtime
- Tauri framework
- React
- Rust and its ecosystem

Thank you to all open-source contributors!

---

**PhotoMind v0.1.0** – Local-first photo culling for photographers
