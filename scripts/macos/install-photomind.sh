#!/bin/bash
# Installs PhotoMind for end users: copy + clear Gatekeeper quarantine.
# Called by "Install PhotoMind.app". Do not open PhotoMind.app from the DMG.
set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"

# App is hidden in .payload so users don't double-click the quarantined .app
if [ -d "$DIR/.payload/PhotoMind.app" ]; then
  SRC="$DIR/.payload/PhotoMind.app"
elif [ -d "$DIR/PhotoMind.app" ]; then
  SRC="$DIR/PhotoMind.app"
else
  osascript <<'EOF'
display alert "PhotoMind installer" message "Could not find PhotoMind.app next to the installer. Re-download the DMG from GitHub Releases." as critical
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
# This is what makes the app openable without the malware warning
xattr -cr "$DEST" || true

open "$DEST"

osascript <<EOF
display dialog "PhotoMind is installed and should be opening now.

Next time you can launch it from Applications or Spotlight.

If it did not open: System Settings → Privacy & Security → Open Anyway." buttons {"OK"} default button 1 with title "PhotoMind"
EOF
