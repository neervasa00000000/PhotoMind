# PhotoMind 0.1.1

PhotoMind organizes photos on your computer. No account, LLM, Ollama, or separate face model is required.

## What changed

- Face detection now decodes the bundled YuNet model correctly and refreshes older detection results.
- People shows conservative possible same-person groups from local face crops. Review the groups; they are suggestions, not verified identities.
- Disconnected drives no longer leave unavailable Trash entries visible in the Bin count.
- New scans start with a fresh library view instead of showing results from an earlier scan.
- Removed the unused Ollama integration and development-only binaries from release packages.

## Downloads

- **Windows:** Download and run `PhotoMind_0.1.1_x64_en-US.msi`. The installer includes WebView2 for offline setup. It is unsigned, so Windows may show a SmartScreen prompt.
- **macOS:** Download `PhotoMind_0.1.1_macOS.dmg` and follow the instructions inside. This build is for Apple Silicon and is not notarized.

The app processes photos and face descriptors on the device. PhotoMind does not upload your photos.

Eye-state analysis remains experimental and does not affect ranking. HEIC/HEIF pixel analysis is limited on Windows.
