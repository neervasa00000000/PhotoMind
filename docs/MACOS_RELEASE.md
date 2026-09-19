# macOS Release Notes

## Current Signing Status

**PhotoMind v0.1.0 is UNSIGNED.**

The current release is built without Apple Developer code signing or notarization. This is a development release suitable for testing and personal use.

## What Users Will See

When opening PhotoMind for the first time, macOS may display:

> **"PhotoMind" cannot be opened because it is from an unidentified developer.**

Or:

> **"PhotoMind" is damaged and can't be opened. You should move it to the Trash.**

**These warnings are expected for unsigned apps.** PhotoMind is not damaged—macOS simply cannot verify the developer signature.

## How to Open PhotoMind (Unsigned)

### Method 1: One-click installer (Recommended)

From v0.1.2 onward, the DMG includes **Install PhotoMind.command**:

1. Open the DMG
2. Double-click **Install PhotoMind.command** (click Open if macOS asks)
3. PhotoMind is copied to Applications, quarantine is cleared, and the app launches

### Method 2: Right-Click Override

1. Locate PhotoMind in Applications
2. **Right-click** (or Control-click) on PhotoMind.app
3. Select **"Open"** from the context menu
4. Click **"Open"** in the security dialog
5. PhotoMind will launch and remember your choice

### Method 3: System Settings

1. Try to open PhotoMind normally (it will be blocked)
2. Open **System Settings → Privacy & Security**
3. Scroll to the Security section
4. Click **"Open Anyway"** next to the PhotoMind message
5. Confirm by clicking **"Open"**

### Method 4: Remove Quarantine Flag (Terminal)

```bash
xattr -cr /Applications/PhotoMind.app
open /Applications/PhotoMind.app
```

This removes macOS's quarantine attribute. Use only if you trust the source.

## Future Signing & Notarization

To release a properly signed and notarized version of PhotoMind, the following is required:

### Required Apple Developer Account

- **Apple Developer Program membership** ($99/year)
- **Developer ID Application Certificate**
- **App-specific password** for notarization

### Required GitHub Secrets

The automated release workflow will need these secrets configured in GitHub:

| Secret Name | Purpose |
|-------------|---------|
| `APPLE_CERTIFICATE` | Base64-encoded Developer ID certificate (.p12) |
| `APPLE_CERTIFICATE_PASSWORD` | Password for the certificate |
| `APPLE_ID` | Apple ID email for notarization |
| `APPLE_TEAM_ID` | 10-character Team ID from Apple Developer |
| `APPLE_APP_PASSWORD` | App-specific password for notarization |

### Signing Process

Once credentials are available:

1. Export Developer ID certificate from Keychain as `.p12`
2. Base64 encode: `base64 -i certificate.p12 | pbcopy`
3. Add to GitHub repository secrets
4. Update `.github/workflows/release.yml` to enable signing
5. Tauri will automatically sign and notarize during build

### Verification

Signed builds can be verified:

```bash
codesign --verify --verbose /Applications/PhotoMind.app
spctl --assess --verbose /Applications/PhotoMind.app
```

## DMG Creation

PhotoMind uses Tauri's built-in DMG bundler. The DMG includes:

- PhotoMind.app (complete application bundle)
- Applications folder shortcut for easy installation
- Custom volume name and icon (configured in tauri.conf.json)

## Bundle Contents

The PhotoMind.app bundle includes:

- **Executable:** Rust-compiled Tauri application
- **Frontend:** React UI (embedded in app bundle)
- **Models:** YuNet face detection model (bundled as resource)
- **ONNX Runtime:** Downloaded automatically by `ort` during build
- **Icons:** macOS app icons in all required sizes

The bundled app is completely self-contained and does not require:
- Node.js
- Rust
- npm
- Separate model downloads
- Ollama (unless user wants optional AI features)

## Security Considerations

### Unsigned Releases

The current unsigned release:
- ✅ Safe to distribute for testing/personal use
- ✅ Includes all required functionality
- ⚠️ Triggers macOS security warnings
- ⚠️ Requires manual security bypass on first launch
- ❌ Cannot be distributed through Mac App Store
- ❌ May be flagged by some enterprise security policies

### Signed Releases

Once signing is implemented:
- ✅ No security warnings for users
- ✅ Notarized by Apple
- ✅ Can be distributed widely
- ✅ Enterprise-friendly
- ✅ Eligible for Mac App Store (with additional sandboxing)

## Building Locally

To build the DMG locally:

```bash
npm install
npm run build
npm run tauri build -- --bundles dmg
```

Output: `src-tauri/target/release/bundle/dmg/PhotoMind_0.1.0_*.dmg`

## Release Checklist

- [ ] Version numbers updated (package.json, Cargo.toml, tauri.conf.json)
- [ ] README.md updated with release notes
- [ ] CHANGELOG.md created/updated
- [ ] All tests passing
- [ ] DMG builds successfully
- [ ] Manual testing on clean macOS system
- [ ] YuNet model included in bundle
- [ ] GitHub release created with DMG attached
- [ ] Release notes published

## Support

For issues with unsigned builds or signing questions:
- Open an issue: https://github.com/neervasa00000000/PhotoMind/issues
- macOS installation guide: See main README.md

---

**Current Status:** v0.1.0 is an unsigned development release. Signing and notarization will be added in a future release.
