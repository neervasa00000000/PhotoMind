# PhotoMind

**Free, open-source, local-first photo culling and selection for photographers.**

PhotoMind helps you organize your photo library, find duplicates, and identify your best shots. All processing happens on your computer—your photos never leave your device.

## Download

### macOS

**[Download latest release →](https://github.com/neervasa00000000/PhotoMind/releases/latest)**

#### Installation (macOS):

1. Download the latest **`.dmg`** from Releases and open it  
2. Double-click **Install PhotoMind.app** once → click **Done** on the warning  
3. **System Settings → Privacy & Security → Open Anyway**  

Or double-click **`install-via-terminal.command`** in the DMG (most reliable).

⚠️ Not Apple-notarized. Fully silent install needs a paid Apple Developer certificate.

📖 **Details:** [macOS Release Notes](docs/MACOS_RELEASE.md)

### Windows

**[Download latest release →](https://github.com/neervasa00000000/PhotoMind/releases/latest)**

#### Installation Steps:

1. Download the latest `PhotoMind_*_x64_en-US.msi` from Releases
2. Double-click the MSI file
3. Follow the installation wizard
4. PhotoMind will be installed to `C:\Program Files\PhotoMind`
5. Launch from Start Menu or Desktop shortcut

**Note:** Windows may show SmartScreen warnings for unsigned installers. Click "More info" → "Run anyway" if prompted.
The Windows installer includes the WebView2 offline installer, so setup does not need to download a runtime. Face detection and People grouping need no separate AI software or model download.

### Linux

Coming soon (Ubuntu .deb and AppImage planned)

## What PhotoMind Does

PhotoMind scans your photo folders and helps you:

- **Find exact duplicates** – Identifies identical files using SHA-256 hashing
- **Detect visual matches** – Groups visually similar photos that need review
- **Organize by moments** – Automatically groups photos taken close together in time
- **Measure technical quality** – Analyzes sharpness, exposure, clipping, and saturation
- **Identify stronger shots** – Provides Keep/Review recommendations based on local analysis
- **Review possible same-person groups** – Compare detected face crops across photos in the People tab
- **Clean up safely** – Move unwanted photos to Bin with easy recovery

### Local & Private

- ✅ No account required
- ✅ No cloud upload
- ✅ No API keys
- ✅ Works completely offline
- ✅ Your photos stay on your computer

### Face Analysis

PhotoMind includes local face detection to help identify photos with people. The People tab compares detected face crops locally and shows only strong possible same-person matches. These are suggestions, not verified identities; similar-looking people may appear together and changes in lighting or pose may prevent a match. Face descriptors remain in the local SQLite database. Eye-state analysis is currently **experimental** and does not affect ranking while validation is ongoing. Face detection runs locally using the bundled YuNet model.

## How To Use

1. **Download** and install PhotoMind
2. **Open** the application
3. **Choose a folder** to scan your photos
4. **Wait** while PhotoMind analyzes your library (processing happens automatically)
5. **Review** exact duplicates, visual matches, moment groups, and possible same-person groups
6. **Keep** your best shots and move others to Bin
7. **Recover** or permanently delete from the Bin tab

PhotoMind works with JPEG, PNG, RAW formats (CR2, CR3, NEF, ARW, RAF, etc.), HEIC/HEIF, and AVIF. See [format compatibility](docs/COMPATIBILITY.md) for details.

## Screenshots

*Coming soon*

---

## For Developers

### Building from Source

**Requirements:**
- Node.js 22+
- A current stable Rust toolchain
- npm or pnpm
- Windows builds: Windows 10/11 with the MSVC C++ build tools

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
On Windows, `npm run tauri build -- --bundles msi` creates an installer under `src-tauri/target/release/bundle/msi/`. Release tags build both platforms in GitHub Actions.

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

Hashing, visual matching, technical quality analysis, face detection, and possible same-person grouping run on the device. No LLM or remote service is used.

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

**Privacy:** PhotoMind processes your photos locally and does not send them to external servers.
