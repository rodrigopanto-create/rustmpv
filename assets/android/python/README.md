# Python Android runtime

Place the arm64-v8a Python runtime and yt-dlp package here before packaging the `native-python` feature.

Required staging content:

- `libpython3.13.so` goes in `assets/android/arm64-v8a/`.
- `python313.zip` and the `yt_dlp/` package are extracted at runtime to the app-private `files/python/` directory.

These binaries must be built for Android arm64-v8a with the NDK. Windows or desktop Linux Python libraries are incompatible.
