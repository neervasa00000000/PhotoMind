#!/bin/bash
# Installs PhotoMind: copy embedded app + clear Gatekeeper quarantine.
# Lives inside Install PhotoMind.app/Contents/Resources/ (App Translocation safe).
set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"

if [ -d "$DIR/PhotoMind.app" ]; then
  SRC="$DIR/PhotoMind.app"
elif [ -d "$DIR/.payload/PhotoMind.app" ]; then
  SRC="$DIR/.payload/PhotoMind.app"
elif [ -d "$DIR/../PhotoMind.app" ]; then
  SRC="$DIR/../PhotoMind.app"
else
  osascript <<'EOF'
display alert "PhotoMind installer" message "Could not find the embedded PhotoMind.app. Re-download the DMG from GitHub Releases." as critical
EOF
  exit 1
fi

if [ -w /Applications ] 2>/dev/null; then
  DEST="/Applications/PhotoMind.app"
else
  mkdir -p "$HOME/Applications"
  DEST="$HOME/Applications/PhotoMind.app"
fi

rm -rf "$DEST"
cp -R "$SRC" "$DEST"
xattr -cr "$DEST" || true

open "$DEST"

osascript <<'EOF'
display dialog "PhotoMind is installed and should be opening now.

Next time launch it from Applications or Spotlight.

If macOS blocks it: System Settings → Privacy & Security → Open Anyway." buttons {"OK"} default button 1 with title "PhotoMind"
EOF
