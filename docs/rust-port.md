# Porting Lavdeck to Rust

Lavdeck is being rewritten from Swift (SwiftUI, AppKit, AVFoundation) to Rust with objc2, in the style of Photon and Kipimo (`../Photon Screen Capture`, `../Kipimo`). The Swift source in `Sources/` is the contract until the port is finished; it is removed at the end.

## Layout

| Crate | Replaces | Holds |
|---|---|---|
| `crates/kit` (`lavdeck-kit`) | `Sources/LavdeckKit`, `Sources/LavdeckPluginKit` | USB link (IOUSBHost), DUML framing, the status decoder, the four allowlisted commands, the mic controller, the button monitor (IOHID), the talking detector, the mic recorder, the track splitter, the take library, transcripts, the plugin protocol, plugin and capture-plugin runners, spawning |
| `crates/capture` (`lavdeck-capture`) | `Sources/LavdeckCapture` | Video settings, screen recording (ScreenCaptureKit), camera recording, the camera preview window, the finished picture-in-picture video |
| `crates/app` (`Lavdeck` binary) | `Sources/LavdeckApp` | The AppKit menu bar app: the deck, the menu bar icon and menu, Recordings, Settings, permissions, the hot key, the plugin host, demo and snapshot modes |
| `crates/cli` (`lavdeck` binary) | `Sources/lavdeck` | The command-line tool |

## Decisions

- **Same behaviour and the same files.** Takes, `.take.json`, `plugin.json`, the plugin protocol, transcripts, `battery.log`, folder and file names are byte-compatible with the Swift app. Settings stay in the `io.github.piedol.lavdeck` defaults domain under the same keys, so existing settings and mic names carry over.
- **No SwiftUI.** Views are AppKit (objc2-app-kit), drawn to match `DESIGN.md` and the Swift app's snapshots.
- **The USB allowlist stays closed.** Only get version, get capabilities, set channel mode and set gain can be built. Nothing may reach the firmware-update channel (HID report 0x55) or send the V1 channel format (it resets the receiver).
- **Every user-visible string goes through `tr`/`trf`/`trn`** (`crates/app/src/i18n.rs`, copied from Kipimo), with the English text as the key. The kit and capture crates return typed errors and data; the app words them.
- **Plugins in Swift keep an SDK.** `LavdeckPluginKit` and the take-reading part of `LavdeckKit` stay as a small Swift package for plugin authors; the root `Package.swift` builds only that.
- Edition 2024, objc2 0.6 / framework crates 0.3. `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` stay clean.
