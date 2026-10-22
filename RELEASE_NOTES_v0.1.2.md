# PhotoMind 0.1.2

This release improves photo and people grouping while keeping all processing on the device.

- Duplicates and edited copies appear in one group per photo, with a second visual hash to find more re-encoded versions.
- People uses a bundled 9.4 MB SFace recognition model instead of comparing tiny raw face crops. It reanalyzes older face results when opened.
- Face analysis uses a larger local preview so small faces contain more detail for matching.
- No Llama, Ollama, account, or model download is needed at runtime.

Download the Windows MSI or Apple Silicon macOS DMG from this release. The Windows installer includes WebView2 for offline setup. Neither installer is code signed.

People groups are suggestions. Very small, blurry, or profile faces may remain unmatched, and similar-looking people can still need review.
