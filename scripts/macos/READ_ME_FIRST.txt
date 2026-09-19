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
