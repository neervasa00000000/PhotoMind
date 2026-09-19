#!/bin/bash
# Package a Gatekeeper-friendlier unsigned macOS DMG:
# - ad-hoc codesign the .app
# - include one-click Install PhotoMind.command (clears quarantine)
# - include Applications shortcut
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
APP="${1:-$ROOT/src-tauri/target/release/bundle/macos/PhotoMind.app}"
VERSION="${2:-0.1.2}"
OUT_DIR="${3:-$ROOT/src-tauri/target/release/bundle/dmg}"
STAGE="$(mktemp -d)/PhotoMind"
DMG_NAME="PhotoMind_${VERSION}_macOS.dmg"

if [ ! -d "$APP" ]; then
  echo "Missing app bundle: $APP" >&2
  exit 1
fi

mkdir -p "$STAGE" "$OUT_DIR"
rm -rf "$STAGE/PhotoMind.app"
cp -R "$APP" "$STAGE/PhotoMind.app"

# Ad-hoc sign (no Apple Developer cert required). Helps Gatekeeper treat the
# bundle as a coherent signed object instead of "damaged".
codesign --force --deep --sign - "$STAGE/PhotoMind.app"
xattr -cr "$STAGE/PhotoMind.app" || true

cp "$ROOT/scripts/macos/Install PhotoMind.command" "$STAGE/"
chmod +x "$STAGE/Install PhotoMind.command"
ln -sf /Applications "$STAGE/Applications"

# README for users who open the DMG
cat > "$STAGE/HOW TO INSTALL.txt" << 'TXT'
PhotoMind — how to install (macOS)

1. Double-click "Install PhotoMind.command"
2. If macOS asks, click Open
3. PhotoMind installs and launches automatically

If that fails:
- Drag PhotoMind.app to Applications
- Open Terminal and run:
  xattr -cr /Applications/PhotoMind.app
  open /Applications/PhotoMind.app

Why? PhotoMind is open-source and not yet Apple-notarized.
macOS blocks unsigned downloads until quarantine is cleared.
TXT

OUT="$OUT_DIR/$DMG_NAME"
rm -f "$OUT"
hdiutil create -volname "PhotoMind" -srcfolder "$STAGE" -ov -format UDZO "$OUT"
echo "Created: $OUT"
ls -lh "$OUT"
