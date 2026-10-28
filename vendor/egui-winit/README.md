# egui-winit

VisualRustMPV vendors version 0.29.1 from crates.io (upstream tag `0.29.1`).
The local patch maps `NamedKey::BrowserBack` to `egui::Key::Escape` only on
Android, allowing the app to handle NativeActivity Back in fullscreen.
A regression test keeps other platforms unchanged. The root Cargo patch
selects this copy; do not edit the global Cargo cache. Upstream MIT and
Apache-2.0 licenses are included here and in the package manifest.

[![Latest version](https://img.shields.io/crates/v/egui-winit.svg)](https://crates.io/crates/egui-winit)
[![Documentation](https://docs.rs/egui-winit/badge.svg)](https://docs.rs/egui-winit)
![MIT](https://img.shields.io/badge/license-MIT-blue.svg)
![Apache](https://img.shields.io/badge/license-Apache-blue.svg)

This crates provides bindings between [`egui`](https://github.com/emilk/egui) and [`winit`](https://crates.io/crates/winit).

The library translates winit events to egui, handled copy/paste, updates the cursor, open links clicked in egui, etc.
