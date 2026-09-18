# PhotoMind

**Free, open-source, local-first photo culling and selection for photographers.**

PhotoMind helps you organize your photo library, find duplicates, and identify your best shots. All processing happens on your computer—your photos never leave your device.

## Download

### macOS

**[Download latest release →](https://github.com/neervasa00000000/PhotoMind/releases/latest)**

1. Download `PhotoMind_0.1.0_macOS.dmg`
2. Open the DMG file
3. Drag PhotoMind to your Applications folder
4. Open PhotoMind from Applications

**Note:** PhotoMind is currently unsigned. macOS may show a security warning. Right-click the app and choose "Open" to bypass this on first launch. See [macOS Release Notes](docs/MACOS_RELEASE.md) for details.

### Windows

Coming soon

### Linux

Coming soon

## What PhotoMind Does

PhotoMind scans your photo folders and helps you:

- **Find exact duplicates** – Identifies identical files using SHA-256 hashing
- **Detect visual matches** – Groups visually similar photos that need review
- **Organize by moments** – Automatically groups photos taken close together in time
- **Measure technical quality** – Analyzes sharpness, exposure, clipping, and saturation
- **Identify stronger shots** – Provides Keep/Review recommendations based on local analysis
- **Clean up safely** – Move unwanted photos to Bin with easy recovery

### Local & Private

- ✅ No account required
- ✅ No cloud upload
- ✅ No API keys
- ✅ Works completely offline
- ✅ Your photos stay on your computer

### Face Analysis

PhotoMind includes local face detection to help identify photos with people. Eye-state analysis is currently **experimental** and does not affect ranking while validation is ongoing. Face detection runs locally using the YuNet model.

## How To Use

1. **Download** and install PhotoMind
2. **Open** the application
3. **Choose a folder** to scan your photos
4. **Wait** while PhotoMind analyzes your library (processing happens automatically)
5. **Review** exact duplicates, visual matches, and moment groups
6. **Keep** your best shots and move others to Bin
7. **Recover** or permanently delete from the Bin tab

PhotoMind works with JPEG, PNG, RAW formats (CR2, CR3, NEF, ARW, RAF, etc.), HEIC/HEIF, and AVIF. See [format compatibility](docs/COMPATIBILITY.md) for details.

## Screenshots

*Coming soon*

---

## For Developers

### Building from Source

**Requirements:**
- Node.js 18+
- Rust 1.70+
- npm or pnpm

**Build instructions:**

```sh
# Install frontend dependencies
npm install

# Build frontend
npm run build

# Build and run in development mode
npm run tauri dev

# Build optimized release
npm run tauri build
```

The built macOS app will be at `src-tauri/target/release/bundle/macos/PhotoMind.app`.

### Testing

```sh
# Frontend build
npm run build

# Rust test suite
cargo test --offline --manifest-path src-tauri/Cargo.toml

# Cleanup planning regression
node --experimental-strip-types .qa/cleanup.test.ts
```

### Development Documentation

- [Phase 0 Stability Report](docs/PHASE0_CURRENT_STATE.md)
- [Format Compatibility](docs/COMPATIBILITY.md)
- [Eye Detection Safety](docs/EYE_DETECTION_SAFETY_FLAG.md)
- [Cache Refresh Behavior](docs/CACHE_REFRESH.md)
- [Eye Validation](docs/EYE_VALIDATION.md)

### Architecture

PhotoMind is built with:
- **Frontend:** React + TypeScript + Vite + Tailwind CSS
- **Backend:** Rust + Tauri
- **Database:** SQLite (local)
- **Image Processing:** Rust image libraries
- **Face Detection:** YuNet (ONNX model, MIT licensed)

Core analysis (hashing, perceptual hashing, technical quality metrics, face detection) runs entirely locally. Optional Ollama integration for advanced AI analysis is experimental and not required.

### CI/CD

Cross-platform CI runs on macOS, Ubuntu, and Windows via GitHub Actions (`.github/workflows/ci.yml`). Tests cover:
- Frontend build
- Rust test suite
- Pagination beyond 1,000 photos
- Duplicate matching across folders/dates
- Bin recovery and cleanup
- Concurrent indexing
- Scanner regression tests

## License

PhotoMind source code is released under the [MIT License](LICENSE).

PhotoMind includes third-party components that retain their respective licenses. See [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md).

---

**Privacy:** PhotoMind processes your photos locally. No data is sent to external servers unless you explicitly enable optional Ollama integration (which requires a separate local Ollama installation).
