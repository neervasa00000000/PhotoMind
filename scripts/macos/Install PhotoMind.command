#!/bin/bash
# One-click install for unsigned PhotoMind builds.
# Copies the app and clears macOS quarantine so Gatekeeper will open it.

set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
SRC="$DIR/PhotoMind.app"

if [ ! -d "$SRC" ]; then
  osascript -e 'display alert "PhotoMind installer" message "PhotoMind.app was not found next to this installer. Keep both files in the same DMG/folder." as critical'
  exit 1
fi

# Prefer /Applications; fall back to ~/Applications
if [ -w /Applications ] || mkdir -p /Applications 2>/dev/null; then
  DEST="/Applications/PhotoMind.app"
else
  mkdir -p "$HOME/Applications"
  DEST="$HOME/Applications/PhotoMind.app"
fi

rm -rf "$DEST"
cp -R "$SRC" "$DEST"
# Critical: remove Brave/Safari quarantine so macOS stops saying "damaged"
xattr -cr "$DEST" || true

open "$DEST"
osascript -e 'display notification "PhotoMind is installed and opening." with title "PhotoMind"'
