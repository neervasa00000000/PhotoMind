#!/bin/bash
# Package a self-contained unsigned macOS DMG.
# Install PhotoMind.app embeds PhotoMind.app + install script in Resources
# so macOS App Translocation cannot break the install.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
APP="${1:-$ROOT/src-tauri/target/release/bundle/macos/PhotoMind.app}"
VERSION="${2:-0.1.0}"
OUT_DIR="${3:-$ROOT/src-tauri/target/release/bundle/dmg}"
STAGE="$(mktemp -d)/PhotoMind"
DMG_NAME="PhotoMind_${VERSION}_macOS.dmg"

if [ ! -d "$APP" ]; then
  echo "Missing app bundle: $APP" >&2
  exit 1
fi

mkdir -p "$STAGE" "$OUT_DIR"

# Build installer shell app
osacompile -o "$STAGE/Install PhotoMind.app" "$ROOT/scripts/macos/Install PhotoMind.applescript"
RES="$STAGE/Install PhotoMind.app/Contents/Resources"
mkdir -p "$RES"

# Embed payload INSIDE the installer (critical for App Translocation)
cp -R "$APP" "$RES/PhotoMind.app"
codesign --force --deep --sign - "$RES/PhotoMind.app"
xattr -cr "$RES/PhotoMind.app" || true

cp "$ROOT/scripts/macos/install-photomind.sh" "$RES/install-photomind.sh"
chmod +x "$RES/install-photomind.sh"

codesign --force --deep --sign - "$STAGE/Install PhotoMind.app" || true
xattr -cr "$STAGE/Install PhotoMind.app" || true

ln -sf /Applications "$STAGE/Applications"

cp "$ROOT/scripts/macos/READ_ME_FIRST.txt" "$STAGE/READ ME FIRST.txt"

# Terminal fallback that uses the embedded payload
cat > "$STAGE/install-via-terminal.command" << 'EOF'
#!/bin/bash
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
INSTALL="$DIR/Install PhotoMind.app"
xattr -cr "$INSTALL" || true
/bin/bash "$INSTALL/Contents/Resources/install-photomind.sh"
EOF
chmod +x "$STAGE/install-via-terminal.command"

OUT="$OUT_DIR/$DMG_NAME"
rm -f "$OUT"
hdiutil create -volname "PhotoMind" -srcfolder "$STAGE" -ov -format UDZO "$OUT"
echo "Created: $OUT"
ls -lh "$OUT"
MNT="$(mktemp -d)"
hdiutil attach "$OUT" -nobrowse -mountpoint "$MNT" -quiet
echo "DMG contents:"
ls -la "$MNT"
echo "Installer Resources:"
ls -la "$MNT/Install PhotoMind.app/Contents/Resources" | head -20
test -d "$MNT/Install PhotoMind.app/Contents/Resources/PhotoMind.app"
test -f "$MNT/Install PhotoMind.app/Contents/Resources/install-photomind.sh"
hdiutil detach "$MNT" -quiet
rmdir "$MNT" 2>/dev/null || true
