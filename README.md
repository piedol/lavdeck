<p align="center">
  <img src="assets/logo.png" width="128" alt="Lavdeck icon">
</p>

<h1 align="center">Lavdeck</h1>

<p align="center">
  A Mac menu bar app that shows the status of DJI Mic Series Mobile RX mics and controls their mode, gain and recording, written in Rust.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/macOS-15%2B%20·%20Apple%20silicon-1c1c1e?logo=apple" alt="macOS 15 or later on Apple silicon">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0--or--later-1c1c1e" alt="License: GPL-3.0-or-later"></a>
</p>

<p align="center">
  <img src="docs/images/showcase.png" width="880" alt="Lavdeck's deck recording two mics in stereo, next to the Recordings window with a two-speaker take and its transcript">
</p>

Lavdeck controls a DJI Mic Series Mobile RX from your Mac's menu bar. Plug the receiver into the Mac and the deck shows both transmitters, with each mic's link state, live level, charging state and firmware. From the same window you switch the receiver between mono and stereo, set its gain from −12 to +12 dB, and record. In stereo, Lavdeck saves a 24-bit WAV file for each mic next to the stereo take.

Lavdeck can record your screen and camera with the mics, keeps every take in a searchable Recordings window, and runs plugins for jobs such as transcription. It talks to the receiver directly over USB and sends it only four commands, each checked on the hardware: get version, get capabilities, set channel mode and set gain.

The program file is 2.6 MB, and Lavdeck uses about 12 MB of memory with the receiver connected.

Lavdeck is in English, Simplified Chinese, Japanese, German, French, Spanish, Brazilian Portuguese and Korean, and follows your system language. You can set Lavdeck's language in System Settings › General › Language & Region › Applications.

Lavdeck is an independent project. It isn't made by DJI or affiliated with DJI.

<p align="center"><sub>The name joins <em>lav</em>, short for lavalier mic, and <em>deck</em>, as in a mixing desk.</sub></p>

## Contents

- [The deck](#the-deck) · [Which mic is which?](#which-mic-is-which) · [Recording](#recording) · [Screen and camera](#screen-and-camera)
- [Recordings](#recordings) · [The menu bar icon](#the-menu-bar-icon) · [Settings](#settings)
- [Requirements](#requirements)
- [Install](#install) · [Updates](#updates) · [Uninstall](#uninstall)
- [Privacy and permissions](#privacy-and-permissions)
- [Plugins](#plugins)
- [Troubleshooting](#troubleshooting)
- [Development](#development) · [Contributing](#contributing)
- [Credits](#credits) · [License](#license)

## The deck

<img src="docs/images/deck.png" width="372" align="right" alt="The deck: strips named Host and Guest, both armed and linked, with level meters, Host's battery at High and Guest charging, firmware 02.03.17, the gain fader at 0 dB, stereo selected, SCREEN on and the Record button">

Click the menu bar icon to open the deck. Each transmitter has a channel strip, and the receiver's controls are on the right.

On each strip, from the top:

- The mic's name. Click it to rename the mic, up to 14 characters. The name is stored with the mic's Bluetooth ID, so it follows that physical mic whichever slot it links to.
- **ARMED** or **OFF**. Only armed mics are recorded.
- The link light: green while the mic is linked, an amber ring if it drops.
- A 12-step level meter. In stereo it shows that mic's level. In mono the receiver mixes both mics before the Mac sees them, so both meters show the mix and say **mono mix**.
- **Charging**, or the battery level the receiver reports. The Mobile RX reports a coarse code, not a percentage, and so far only its "High" code has been mapped.
- The firmware version, marked when a mic's firmware is older than the other's.

On the right are the gain fader (−12, −6, 0, +6 and +12 dB), **MONO** and **STEREO** (stereo puts TX1 on the left channel and TX2 on the right), **SCREEN** and **CAMERA**, and **Record**. The footer shows the receiver's noise cancelling, which you set with the O, L and H switch on the receiver, and the recording format. Click the format to open Recordings, or the gear for Settings.

Right-click the menu bar icon for a menu with recording, mono or stereo, Recordings, Settings, **Check for Updates…** and **Quit Lavdeck**.

<br clear="right">

## Which mic is which?

<img src="docs/images/identify.png" width="372" align="right" alt="Identify mode: Host's strip is outlined in green and the banner says That's Host (TX1)">

Click **Which mic is which?** and talk into a mic. Its strip lights up and the banner names it. Click a strip's name to label the mic while you're there.

The receiver reports separate levels only in stereo, so Lavdeck switches the receiver to stereo while the banner is open and switches it back when you click **Done**, or after 45 seconds.

<br clear="right">

## Recording

<img src="docs/images/recording.png" width="372" align="right" alt="The deck while recording: red name plates for Host on the left and Guest on the right, each naming its file, the elapsed time 00:12:47 and SCREEN lit">

Start and stop a take with **Record**, with **⌥⌘R** from any app, or by tapping the link button on either transmitter. While a take runs, each strip names the file it's writing, its meter shows the level being recorded with a dBFS readout, and the elapsed time is at the top.

Takes go into a folder for each day, named by the time they started:

| Mode | Files |
|---|---|
| Stereo, both mics armed | `11.05.09 Stereo.wav`, plus `11.05.09 TX1.wav` and `11.05.09 TX2.wav` (WAV only, and the separate files can be turned off) |
| Mono | `11.05.09 Mix.wav` |
| TX1 or TX2 armed alone | `11.05.09 TX1.wav` (Lavdeck switches to stereo and keeps only that mic's channel) |

Audio is 48 kHz WAV at 24 bits, or M4A (AAC at 256 kbps). Next to the audio, `11.05.09.take.json` notes when the take started, its length, its mode and the mics' names. The folder is `~/Recordings/Lavdeck` unless you choose another in Settings.

If you quit Lavdeck during a take, including when macOS offers to quit and reopen it after a permission change, it stops the take and saves its files first.

Recording with the transmitters' buttons is on by default, in Settings › General. The receiver can't tell the Mac which button was pressed, so either button starts and stops recording. While the setting is on, the buttons don't change your Mac's volume.

<br clear="right">

## Screen and camera

Switch on **SCREEN**, **CAMERA** or both before you record, and the take includes video:

- The screen recording is the main display at 30 or 60 fps, in HEVC or H.264, with your apps' sound as a second audio track if you want it. Lavdeck's windows and the camera preview are left out.
- Your default camera records at its largest landscape size, up to 3840 pixels wide.
- Video starts before the audio. Lavdeck waits up to 5 seconds for each source's first frame, so the picture covers the take from its first second, and it notes where each file starts relative to the audio.
- While you record, a camera preview floats above your windows unless you turn it off. Drag it anywhere, and double-click it to switch between a bubble, a large view and full screen.
- After the take, Lavdeck makes `11.05.09 PiP.mp4` unless you turn it off: your screen with the camera in a corner (or the camera alone) and the mic audio. The bubble's corner, size and shape are in **Settings › Video**, and **Remake Video** in Recordings applies new settings to an earlier take.

## Recordings

<p align="center">
  <img src="docs/images/recordings.png" width="880" alt="The Recordings window: 18 takes with their date, length, video badge, mics and the start of their transcript; the selected take shows its audio player, files and a Host and Guest transcript with timestamps">
</p>

The Recordings window lists every take with its date, length, a badge when it has video, its mics and the first words of its transcript. You can sort by any column, filter to takes with video, with or without a transcript, or with two speakers, and search transcripts, mic names and dates.

- Space plays and pauses. For a take with video, the video plays in the window with the mic audio. Otherwise you hear the stereo or mono take.
- Click a line of the transcript to play from there. The transcript follows playback until you scroll it.
- Click an audio or video file to play it, drag a file into another app, or right-click it to open it or show it in Finder. **Share** and **More** hold the other actions: Show in Finder, Copy Transcript, Remake Video, plugin actions and Move to Trash.
- Select several takes with ⌘-click, ⇧-click or ⌘A to transcribe the ones without a transcript, remake their videos, copy their transcripts as text, or move them to the Trash.

Moving a take to the Trash moves all its files there, so Finder's **Put Back** can restore it.

Lavdeck doesn't ship a transcription plugin. Transcripts come from a plugin you install that writes Lavdeck's transcript format, as described under [Plugins](#plugins).

## The menu bar icon

<p align="center">
  <img src="docs/images/menu-bar-states.png" width="648" alt="The menu bar icon on a light and a dark menu bar, in six states">
</p>

From left to right: both mics linked; recording; a mic has dropped; low battery; something needs attention; no receiver. The low-battery state is for receivers that report a battery level. The Mobile RX doesn't, so the state never appears with it.

## Settings

| Tab | Holds |
|---|---|
| General | The recordings folder, WAV or M4A, separate TX1 and TX2 files in stereo, the transmitter buttons, **Open at login** and **Check for updates automatically** |
| Video | Screen and camera recording, the camera preview, system audio, frame rate, video format, and the finished video's camera bubble |
| Plugins | Installed plugins, switching each on or off, their settings, and the permissions they use |

## Requirements

- A DJI Mic Series Mobile RX, the receiver that plugs into a phone or computer, connected to the Mac directly or through a hub that carries data.
- A Mac with Apple silicon and macOS 15 or later. Lavdeck has been tested on macOS 26.

| Hardware | Status |
|---|---|
| Mobile RX with DJI Mic Mini transmitters | Tested |
| Mobile RX with DJI Mic Mini 2, Mic 2 or Mic 3 transmitters | Likely to work; untested |
| Other DJI Mic receivers, such as the clip-on Mic Mini and Mic 2 receivers | Unknown; [reports welcome](#contributing) |

Lavdeck can't update the mics' firmware. The deck marks a mic whose firmware is older than the other's, and you update it with DJI's app.

## Install

### Download

Download `Lavdeck-macOS-arm64.zip` from the [Releases page](https://github.com/piedol/lavdeck/releases). It's signed and notarized by Apple.

1. Open the zip and drag **Lavdeck** into your Applications folder.
2. Open Lavdeck. Its icon appears in the menu bar. Lavdeck has no Dock icon.
3. Plug in the receiver and click the icon.

### Build from source

You need Rust 1.98 or later ([rustup](https://rustup.rs)) and Xcode's Command Line Tools (`xcode-select --install`).

```bash
git clone https://github.com/piedol/lavdeck.git
cd lavdeck && scripts/bundle.sh
ditto build/Lavdeck.app /Applications/Lavdeck.app && open /Applications/Lavdeck.app
```

`scripts/bundle.sh` builds `build/Lavdeck.app` and signs it with the hardened runtime. It uses the code-signing identity in `LAVDECK_SIGN_IDENTITY`, or else the first line of a `.signing-identity` file in the checkout (a certificate name or SHA-1, ignored by git), or else the first Developer ID Application certificate in your keychain. Without any of these it signs ad hoc, which works on your Mac, but an ad-hoc signature changes with every build and macOS then asks for the microphone, camera and screen recording permissions again. Any identity in your keychain avoids that, including the free Apple Development certificate Xcode makes for your Apple Account.

### First launch

1. Plug in the receiver and click the menu bar icon.
2. macOS asks for the microphone the first time you record.
3. It asks for the camera, and for Screen & System Audio Recording, when you first switch on **CAMERA** or **SCREEN**. macOS applies a new screen recording permission only after Lavdeck reopens, so quit and reopen it after you allow it.
4. To start Lavdeck when you log in, turn on **Open at login** in **Settings › General**.

### Updates

Right-click the menu bar icon and choose **Check for Updates…** to look for a new version. Lavdeck also checks about a minute after launch if the last check was more than a day ago, then daily while it runs. To stop these checks, turn off **Check for updates automatically** in Settings › General.

When a new version is out, a notification says so and the menu shows **Update to Lavdeck X.Y.Z…**. The first time this happens, macOS asks whether Lavdeck may show notifications. Choose the menu item or click the notification to read the release notes, then click **Install and Relaunch**.

Lavdeck downloads the release from GitHub and installs it only if the download matches the checksum GitHub lists for it and the app inside is signed by Lavdeck's developer and notarized by Apple. The old copy goes to the Trash and the new version opens in its place. During a take, the install waits until you stop recording and the take is saved. If your account can't make changes in the folder Lavdeck is in, Lavdeck opens the release page in your browser instead.

### Uninstall

1. Turn off **Open at login** in Settings › General, then right-click the menu bar icon and choose **Quit Lavdeck**.
2. Move `/Applications/Lavdeck.app` to the Trash.
3. Run `defaults delete io.github.piedol.lavdeck` to remove its settings and mic names.
4. Move `~/Library/Logs/Lavdeck`, `~/Library/Caches/Lavdeck` (left by an update) and `~/Library/Application Support/Lavdeck` (installed plugins) to the Trash.
5. Your recordings stay in `~/Recordings/Lavdeck`, or the folder you chose, until you remove them.
6. Remove Lavdeck from System Settings › Privacy & Security › Microphone, Camera, and Screen & System Audio Recording.

## Privacy and permissions

- Lavdeck checks GitHub once a day for a new version. You can turn this off in Settings › General. Lavdeck has no analytics. Plugins are separate programs, so check what a plugin does before you install it.
- The microphone is on only between Record and Stop, and Lavdeck records only the receiver.
- The screen and camera are recorded only during a take, and only when SCREEN or CAMERA is on. The camera preview shows only while recording.
- Lavdeck registers **⌥⌘R** as a system-wide hot key, which doesn't need Accessibility access.

What Lavdeck keeps on your Mac:

| What | Where |
|---|---|
| Recordings, their `.take.json` (start time, length, mode, mic names and Bluetooth IDs) and any video | `~/Recordings/Lavdeck`, or the folder you chose |
| Settings, including mic names stored by Bluetooth ID | `~/Library/Preferences/io.github.piedol.lavdeck.plist` |
| A line each time a transmitter's charging state or battery code changes, with the mic's name | `~/Library/Logs/Lavdeck/battery.log` |
| Installed plugins | `~/Library/Application Support/Lavdeck/Plugins` |
| An update while it downloads and is checked | `~/Library/Caches/Lavdeck/Updates` |

When TX1 or TX2 is armed alone, the take is recorded to a temporary file first, which Lavdeck deletes after that mic's channel is saved.

The receiver also has a firmware-update channel on the same USB connection. Lavdeck builds its four commands from a closed list and has no way to send anything else. [docs/PROTOCOL.md](docs/PROTOCOL.md) has the details.

## Plugins

A plugin is a folder with a `plugin.json` file and a program in any language. The program reads a JSON request on standard input and reports progress and results as JSON lines.

| Kind | What it does |
|---|---|
| Event | Runs automatically when a take finishes, for example to transcribe it |
| Action | Runs on a take from the Recordings window |
| Capture | Records another source, such as a second camera, alongside the mics, lined up with the audio to the frame |
| App | Adds an **Open in …** button that opens the take in a companion app, such as an editor |

A plugin that writes Lavdeck's transcript format gets search, speaker names and click-to-play in the Recordings window.

Install a plugin with **Settings › Plugins › Install Plugin…**, or with `lavdeck plugins install <folder>`. Plugins are installed switched off. A plugin declares the permissions it needs, such as the camera. Lavdeck requests them from macOS, and the plugin runs under Lavdeck's permissions. A plugin runs as you and isn't sandboxed, so install only plugins you trust.

[docs/PLUGINS.md](docs/PLUGINS.md) documents the plugin protocol, and [`Examples/plugins/take-info`](Examples/plugins/take-info) is an example in Python that writes a summary of each take. [docs/PROTOCOL.md](docs/PROTOCOL.md) describes the receiver's USB protocol, as far as it's been mapped.

## Troubleshooting

| Problem | Fix |
|---|---|
| The deck says the receiver isn't connected | Plug the receiver into the Mac directly, or use a hub or cable that carries data, not just power. |
| Recording doesn't start, or ⌥⌘R beeps | Both mics are **OFF**. Click **OFF** under a mic to arm it. |
| "Microphone access is off" | Turn on Lavdeck in System Settings › Privacy & Security › Microphone. |
| SCREEN is on but takes have no screen recording | Turn on Lavdeck in System Settings › Privacy & Security › Screen & System Audio Recording, then quit and reopen Lavdeck. |
| macOS asks for permissions again after every build | Your build is signed ad hoc. Sign it with a stable identity; see [Build from source](#build-from-source). |
| Both meters move together | The receiver is in mono, which mixes both mics. Switch to stereo to see each mic. |
| A mic's link button changes the Mac's volume | Turn on **Tap a mic's link button to start or stop recording** in Settings › General. |
| A strip shows "Level 3" or similar instead of a battery level | The receiver reported a battery code Lavdeck hasn't mapped yet. It's logged in `~/Library/Logs/Lavdeck/battery.log`; a report that includes the battery level DJI's app showed at the time helps map it. |
| A take has no transcript | Lavdeck transcribes through plugins. Install a transcription plugin and switch it on in Settings › Plugins. |
| An update opens the release page instead of installing | Your account can't make changes in the folder Lavdeck is in. Download the zip from that page and drag **Lavdeck** into Applications. |

## Development

| Path | What it does |
|---|---|
| `crates/kit` | The USB link, DUML framing, the status decoder, the allowlisted commands, the recorder, the take library, the plugin protocol and the plugin and capture runners. Its tests run the decoder and framing on frames captured from the receiver |
| `crates/capture` | Screen and camera recording, the camera preview and each take's finished video |
| `crates/app` | The AppKit menu bar app: the deck, Recordings, Settings, permissions, updates, and the demo and snapshot modes |
| `crates/cli` | The `lavdeck` command-line tool, for scripting and debugging |
| `locales` | The translations, keyed by the English text |
| `sdk/swift` | `LavdeckPluginKit` and the take library reader, for plugins written in Swift (`Package.swift` builds only these) |
| `Examples/plugins` | The example plugin and its Python helper |
| `scripts` | The app bundle, the icon and the README images |

```bash
cargo test                                           # build everything and run the tests
scripts/bundle.sh                                    # build and sign build/Lavdeck.app
scripts/bundle.sh --notarize                         # also notarize it and zip dist/Lavdeck-macOS-arm64.zip
build/Lavdeck.app/Contents/MacOS/Lavdeck --demo      # run with made-up mics, no receiver needed
build/Lavdeck.app/Contents/MacOS/Lavdeck --demo --snapshot /tmp/deck   # draw the deck and menu bar icons to PNG
LAVDECK_UPDATE_TEST=install build/Lavdeck.app/Contents/MacOS/Lavdeck   # check now and install a newer release without asking
swift scripts/make-showcase.swift                    # redraw docs/images/showcase.png and social-preview.png
```

Add `-AppleLanguages '(de)'` to run or draw it in another language.

`LAVDECK_UPDATE_TEST=install` runs with the demo's mics, so it never opens the receiver or touches your settings. `LAVDECK_UPDATE_FEED=<url>` reads the release from that URL in place of GitHub, and a `file://` feed may point at a local zip.

`--notarize` needs a Developer ID Application certificate and a notarytool keychain profile, named by `NOTARY_PROFILE` (default `piedol-notary`).

The `lavdeck` tool reads and changes the receiver from the terminal. Quit the app before running commands that use the receiver, because two Lavdeck processes can't talk to it at once. `takes`, `plugins` and `run` don't use the receiver.

```bash
cargo build --release -p lavdeck-cli
target/release/lavdeck status --json           # live status (read-only)
target/release/lavdeck caps                    # what the receiver supports (read-only)
target/release/lavdeck set channel stereo      # or mono
target/release/lavdeck set gain 6              # −12 to +12 dB
target/release/lavdeck takes                   # list recordings
target/release/lavdeck run <plugin-id> <action> <take-id>
```

### Contributing

Bug reports and pull requests are welcome, especially from people with other DJI receivers or transmitters. `lavdeck status --json` and `lavdeck caps` are read-only. Include their output in your report, with the serial numbers and Bluetooth IDs removed. Before opening a pull request, run `cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`.

## Credits

Lavdeck uses Apple's frameworks (AppKit, AVFoundation, ScreenCaptureKit, IOKit and IOUSBHost) through the [objc2](https://github.com/madsmtm/objc2) Rust bindings. It contains no DJI code. [docs/PROTOCOL.md](docs/PROTOCOL.md) explains how the receiver's protocol was worked out.

DJI and DJI Mic are trademarks of SZ DJI Technology Co., Ltd., used here only to say which hardware Lavdeck works with.

## License

Lavdeck is free software under the [GNU General Public License v3.0 or later](LICENSE). You can use, study, share and change it, including at work. If you distribute a modified version, you must release its source under the same license.

Copyright 2026 piedol
