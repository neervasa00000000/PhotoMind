========================================
  PhotoMind — how to install on Mac
========================================

Apple blocks unsigned downloads. That is normal.
PhotoMind is NOT malware.

----------------------------------------
METHOD A — System Settings (GUI)
----------------------------------------

1. Double-click "Install PhotoMind.app" once
   (warning appears — click Done)

2. Open System Settings → Privacy & Security

3. Scroll to Security

4. Click "Open Anyway" next to Install PhotoMind

5. Confirm Open

----------------------------------------
METHOD B — Terminal (most reliable)
----------------------------------------

1. Keep this DMG window open
2. Double-click "install-via-terminal.command"
   OR paste in Terminal:

xattr -cr "/Volumes/PhotoMind/Install PhotoMind.app" && "/Volumes/PhotoMind/Install PhotoMind.app/Contents/Resources/install-photomind.sh"

----------------------------------------

Need help? https://github.com/neervasa00000000/PhotoMind
