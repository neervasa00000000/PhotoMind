#!/bin/bash
# Package a user-friendly unsigned macOS DMG:
# - Hide PhotoMind.app in .payload (so users don't open the quarantined app)
# - Ship Install PhotoMind.app that copies + clears quarantine
# - Clear first-open instructions in READ ME FIRST.txt
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
APP="${1:-$ROOT/src-tauri/target/release/bundle/macos/PhotoMind.app}"
VERSION="${2:-0.1.3}"
OUT_DIR="${3:-$ROOT/src-tauri/target/release/bundle/dmg}"
STAGE="$(mktemp -d)/PhotoMind"
DMG_NAME="PhotoMind_${VERSION}_macOS.dmg"

if [ ! -d "$APP" ]; then
  echo "Missing app bundle: $APP" >&2
  exit 1
fi

mkdir -p "$STAGE/.payload" "$OUT_DIR"
rm -rf "$STAGE/.payload/PhotoMind.app"
cp -R "$APP" "$STAGE/.payload/PhotoMind.app"

codesign --force --deep --sign - "$STAGE/.payload/PhotoMind.app"
xattr -cr "$STAGE/.payload/PhotoMind.app" || true

# Helper script used by the Install app
cp "$ROOT/scripts/macos/install-photomind.sh" "$STAGE/install-photomind.sh"
chmod +x "$STAGE/install-photomind.sh"

# Build a real .app installer (clearer than a .command file)
osacompile -o "$STAGE/Install PhotoMind.app" "$ROOT/scripts/macos/Install PhotoMind.applescript"
# Give the installer a generic app icon feel; keep it ad-hoc signed
codesign --force --deep --sign - "$STAGE/Install PhotoMind.app" || true
xattr -cr "$STAGE/Install PhotoMind.app" || true

ln -sf /Applications "$STAGE/Applications"

cat > "$STAGE/READ ME FIRST.txt" << 'TXT'
========================================
  PhotoMind — how to install on Mac
========================================

Apple blocks unsigned downloads. Double-click and
even Right-click → Open may only show "Done".
That is normal. The app is NOT malware.

----------------------------------------
EASIEST METHOD (macOS Sequoia / recent Macs)
----------------------------------------

1. Double-click "Install PhotoMind.app" once
   (you will see the warning — click Done)

2. Open System Settings → Privacy & Security

3. Scroll down to the Security section

4. Click "Open Anyway" next to Install PhotoMind

5. Confirm Open

PhotoMind installs to Applications and launches.

----------------------------------------
ALTERNATIVE (Terminal — always works)
----------------------------------------

1. Keep this DMG window open
2. Open Terminal
3. Paste this and press Return:

xattr -cr /Volumes/PhotoMind && /Volumes/PhotoMind/install-photomind.sh

----------------------------------------

Do NOT try to open a hidden PhotoMind.app yourself.
Need help? https://github.com/neervasa00000000/PhotoMind
TXT

OUT="$OUT_DIR/$DMG_NAME"
rm -f "$OUT"
hdiutil create -volname "PhotoMind" -srcfolder "$STAGE" -ov -format UDZO "$OUT"
echo "Created: $OUT"
ls -lh "$OUT"
# List DMG contents for CI logs
MOUNT=$(hdiutil attach "$OUT" -nobrowse | awk 'END{print $NF}')
echo "DMG contents:"
ls -la "$MOUNT"
hdiutil detach "$MOUNT" >/dev/null
