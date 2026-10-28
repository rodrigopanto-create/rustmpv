# Workspace

```text
VisualRustMPV/
├── Cargo.toml
├── crates/
│   └── core_logic/
│       ├── Cargo.toml
│       └── src/lib.rs
├── apps/
│   ├── app_windows/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs
│   │       └── playlist.rs
│   └── app_android/
│       ├── Cargo.toml
│       ├── README.md
│       └── src/lib.rs
├── assets/android/arm64-v8a/
│   ├── libmpv.so
│   ├── libplayer.so
│   ├── libav*.so
│   ├── libsw*.so
│   └── libc++_shared.so
└── src/
    ├── app.rs
    ├── downloads.rs
    ├── mpv.rs
    └── android_bridge.rs
```

`src/` conserva la implementación modular anterior como fuente compartida temporal para `app_windows` y el puente Android. La lógica de dominio nueva vive en `crates/core_logic`; el siguiente paso natural es migrar gradualmente `app.rs`, `downloads.rs` y `mpv.rs` a sus respectivos paquetes sin cambiar contratos de usuario.

La reproduccion de `app_android` utiliza ahora su propio puente en
`apps/app_android/src/chaquopy.rs`; no reutiliza el puente Android antiguo de
`src/`. El build Gradle mantiene un unico Python administrado por Chaquopy.
