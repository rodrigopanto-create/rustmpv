# Android native assets

This directory is reserved for ABI-specific Android artifacts. Do not copy the Windows files from `assets/` here.

Required artifacts for a complete Android implementation:

- `libmpv.so` and its native dependencies from a compatible mpv-android/libmpv build.
- An Android/NDK FFmpeg integration, normally exposed through libmpv or JNI.
- A Python runtime plus the `yt-dlp` Python package, or a separately maintained Android-native downloader.

The current Rust app targets `aarch64-linux-android` and deliberately reports these dependencies as unavailable instead of trying to execute incompatible `.exe` files.
