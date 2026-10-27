# How Lavdeck is built

Lavdeck is a Rust app that talks to macOS through the [objc2](https://github.com/madsmtm/objc2) bindings. It began as a Swift app and was ported to Rust before its first release, keeping its behaviour, its files on disk and its settings. Photon and Kipimo, by the same author, are built the same way.

## Layout

| Crate | Was | Holds |
|---|---|---|
| `crates/kit` (`lavdeck-kit`) | `Sources/LavdeckKit`, `Sources/LavdeckPluginKit` | USB link (IOUSBHost), DUML framing, the status decoder, the four allowlisted commands, the mic controller, the button monitor (IOHID), the talking detector, the mic recorder, the track splitter, the take library, transcripts, the plugin protocol, plugin and capture-plugin runners, spawning |
| `crates/capture` (`lavdeck-capture`) | `Sources/LavdeckCapture` | Video settings, screen recording (ScreenCaptureKit), camera recording, the camera preview window, the finished picture-in-picture video |
| `crates/app` (`Lavdeck` binary) | `Sources/LavdeckApp` | The AppKit menu bar app: the deck, the menu bar icon and menu, Recordings, Settings, permissions, the hot key, the plugin host, demo and snapshot modes |
| `crates/cli` (`lavdeck` binary) | `Sources/lavdeck` | The command-line tool |

## Decisions

- **Same files as the Swift app.** Takes, `.take.json`, `plugin.json`, the plugin protocol, transcripts, `battery.log`, folder and file names are byte-compatible with what the Swift app wrote. Settings live in the `io.github.piedol.lavdeck` defaults domain under the same keys, so settings and mic names carried over.
- **AppKit views.** The interface is AppKit (objc2-app-kit), drawn to match `DESIGN.md`. The app reaches the kit and capture crates through one `Backend` trait (`crates/app/src/backend.rs`); `--demo` swaps in made-up mics.
- **The USB allowlist stays closed.** Only get version, get capabilities, set channel mode and set gain can be built. Nothing may reach the firmware-update channel (HID report 0x55) or send the V1 channel format (it resets the receiver).
- **Every user-visible string goes through `tr`/`trf`/`trn`** (`crates/app/src/i18n.rs`), with the English text as the key. The kit and capture crates return typed errors and data; the app words them.
- **Plugins in Swift keep an SDK.** `LavdeckPluginKit` and the take-reading part of `LavdeckKit` are a small Swift package in `sdk/swift` for plugin authors; the root `Package.swift` builds only that.
- Edition 2024, objc2 0.6 / framework crates 0.3. `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` stay clean.
