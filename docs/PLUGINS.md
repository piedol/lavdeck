# Writing Lavdeck plugins

A plugin adds something Lavdeck does with a recording once it exists, such as a transcript, a cleaned-up mix, or an upload. Plugins run **after** recording, never in the live audio path, so a slow or broken plugin can't cost you a take.

A plugin is a folder holding a `plugin.json` manifest and an executable. You can write it in any language. Lavdeck starts the executable once per job, writes one JSON request to its standard input, and reads JSON messages from its standard output until it exits.

- [Install and try one](#install-and-try-one)
- [The manifest](#the-manifest)
- [Jobs: events and actions](#jobs-events-and-actions)
- [The request](#the-request)
- [Messages](#messages)
- [Writing output](#writing-output)
- [Conventions](#conventions)
- [Capture plugins (protocol 2)](#capture-plugins-protocol-2)
- [Helpers](#helpers)
- [Trust](#trust)

## Install and try one

The repository ships a small example, [`Examples/plugins/take-info`](../Examples/plugins/take-info). It writes a Markdown summary next to each take.

1. Open **Settings › Plugins**, click **Install Plugin…** and choose the plugin's folder. Lavdeck symlinks it into `~/Library/Application Support/Lavdeck/Plugins/`, so edits to your folder take effect on the next run.
2. Switch the plugin on. Plugins are installed switched off.
3. Record a take, or open **Recordings**, select a take and choose the plugin's action from **More**.

The `lavdeck` command-line tool does the same thing, which is handy while developing:

```bash
lavdeck plugins install Examples/plugins/take-info
lavdeck plugins enable io.github.piedol.lavdeck.example.take-info
lavdeck takes
lavdeck run io.github.piedol.lavdeck.example.take-info summarise 2026-10-11/12.34.54
```

Only one Lavdeck process can talk to the receiver at a time, but `takes`, `plugins` and `run` don't need the receiver, so they work while the app is open.

## The manifest

`plugin.json`, at the root of the plugin folder:

```json
{
  "id": "io.github.you.lavdeck.example",
  "name": "Example",
  "version": "1.0.0",
  "protocol": 1,
  "summary": "One sentence shown in Settings.",
  "executable": "run.py",
  "arguments": [],
  "events": ["take.finished"],
  "actions": [
    { "id": "summarise", "title": "Write take summary", "summary": "Optional tooltip." }
  ],
  "settings": [
    { "key": "language", "title": "Language", "type": "choice", "default": "auto",
      "options": [ { "value": "auto", "title": "Detect" }, { "value": "en", "title": "English" } ] },
    { "key": "subtitles", "title": "Write subtitles", "type": "bool", "default": true },
    { "key": "threshold", "title": "Threshold (dB)", "type": "number", "default": 6, "help": "Shown as a tooltip." },
    { "key": "model", "title": "Model folder", "type": "path", "default": "~/Models/whisper" }
  ],
  "produces": ["notes"]
}
```

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | Reverse-DNS identifier. Lavdeck keys enabled state and settings by it. |
| `name`, `version` | yes | Shown in Settings. |
| `protocol` | yes | Protocol version the plugin speaks: `1`, or `2` for capture plugins. Lavdeck sends post jobs in the version you declare and lists a plugin with a version it doesn't know as a problem. |
| `kind` | no | `post` (the default), `capture` or `app`. See [Capture plugins](#capture-plugins-protocol-2). |
| `permissions` | no | System permissions the plugin relies on, shown next to its switch: `screen-recording`, `camera`, `microphone`, `network`. |
| `minAppVersion` | no | Oldest Lavdeck version the plugin works with. |
| `executable` | yes | Path relative to the plugin folder. It must be executable (`chmod +x`). |
| `arguments` | no | Extra command-line arguments. |
| `events` | no | Events that run the plugin automatically while it is switched on. |
| `actions` | no | Commands a user can run on any take from the Recordings window. |
| `settings` | no | Lavdeck draws these in Settings › Plugins and sends the values with every request. Types are `bool`, `string`, `number`, `choice` (with `options`) and `path` (a text field plus a folder or file picker). |
| `produces` | no | Output kinds. A plugin that produces `transcript` becomes the one behind **Generate transcript** in the Recordings window. |

## Jobs: events and actions

There are two ways a job starts:

- **Event.** Protocol 1 has one event, `take.finished`. It fires when a recording has stopped and every file is in place: the stereo or mono file, split tracks, single-mic extracts and the take's metadata.
- **Action.** A user picks one of the plugin's `actions` on a take in the Recordings window. Actions work on any take, old or new, so make them safe to run twice. Overwriting your own earlier output is the expected behaviour.

Each job is a separate process. Lavdeck shows progress in the menu bar popover and the Recordings window, and lets the user cancel. Cancelling sends `SIGTERM`.

## The request

Lavdeck writes a single JSON object to standard input, then closes it:

```json
{
  "protocol": 1,
  "kind": "event",
  "name": "take.finished",
  "take": {
    "id": "2026-10-11/13.00.00",
    "folder": "/Users/you/Recordings/Lavdeck/2026-10-11",
    "stem": "13.00.00",
    "started": "2026-10-11T17:00:00Z",
    "duration": 44.0,
    "channelMode": "stereo",
    "files": [
      { "path": ".../13.00.00 Stereo.wav", "role": "stereo" },
      { "path": ".../13.00.00 TX1.wav", "role": "track", "mic": 1 },
      { "path": ".../13.00.00 TX2.wav", "role": "track", "mic": 2 },
      { "path": ".../13.00.00.take.json", "role": "metadata" }
    ],
    "speakers": [
      { "id": "tx1", "mic": 1, "name": "Host", "file": ".../13.00.00 TX1.wav" },
      { "id": "tx2", "mic": 2, "name": "Guest", "file": ".../13.00.00 TX2.wav" }
    ]
  },
  "settings": { "language": "auto", "subtitles": true, "threshold": 6 },
  "outputDirectory": "/Users/you/Recordings/Lavdeck/2026-10-11",
  "pluginDirectory": "/Users/you/plugins/example"
}
```

**`kind` and `name`.** `kind` is `event` or `action`. `name` is the event name or your action's `id`.

**`take.files[].role`** is one of:

| Role | What it is |
|---|---|
| `stereo` | Both mics, TX1 left and TX2 right. |
| `mix` | Mono mix of both mics. |
| `track` | One mic on its own, with `mic` set to 1 or 2. Present when split tracks are on, or when only one mic was armed. |
| `derived` | Anything a plugin wrote, with `kind` set (`transcript`, `audio`, …). |
| `metadata` | Lavdeck's `<stem>.take.json`. |
| `capture` | Recorded by a capture plugin alongside the audio, with `kind` (`video` or `audio`), `source` (`screen`, `camera`, `system-audio`) and `offset`: seconds after the take's first audio sample that the file's first frame happened. |

**`take.speakers`** lists the mics that were armed, with the names the user gave them. A speaker has a `file` when that mic has its own track. When two speakers both have files, the take is a **two-speaker take**: each person's voice is on its own file, with some bleed from the other mic.

**Other fields.** Durations and times are in seconds; dates are ISO 8601. Unknown fields may appear in later minor releases, so ignore what you don't recognise.

**Environment.** Settings also arrive as environment variables, for plugins that are shell scripts: `LAVDECK_SETTING_<KEY>`, with the key upper-cased and `-` turned into `_`. Two more variables are set:
- `LAVDECK_PLUGIN_PROTOCOL` is the protocol version.
- `LAVDECK_PLUGIN_DIR` is the plugin folder, which is also the working directory.

## Messages

Write one JSON object per line to standard output, and flush after each one:

```json
{"type":"progress","fraction":0.4,"message":"Transcribing Host"}
{"type":"log","message":"model loaded in 0.8s"}
{"type":"output","path":"/Users/you/Recordings/Lavdeck/2026-10-11/13.00.00.info.md","kind":"notes"}
{"type":"done","message":"Wrote 13.00.00.info.md"}
```

| `type` | Fields | Effect |
|---|---|---|
| `progress` | `fraction` (0 to 1), `message` | Updates the job's progress bar and status line. |
| `output` | `path`, `kind` | Tells Lavdeck you wrote a file, so the Recordings window refreshes. |
| `log` | `message` | Diagnostic text kept with the job. |
| `done` | `message` | Optional one-line summary of the result. |
| `error` | `message` | A sentence the user will read. The job fails even if you then exit 0. |

Lines that aren't JSON, and everything on standard error, are treated as log text. A non-zero exit status fails the job, and the last few log lines are shown.

## Writing output

Write next to the take, in `outputDirectory`, and name every file with the take's `stem` as a prefix. That is how Lavdeck groups files into takes: anything starting with `13.00.00` belongs to the 13.00.00 take. It also means **Move to Trash** in the Recordings window removes your output together with the recording.

- **Never modify or delete the recording's own files.** Write new files instead, for example `13.00.00 TX1 · voice-ada.wav` beside `13.00.00 TX1.wav`.
- Write to a temporary name in the same folder, then rename, so a cancelled job never leaves a half-written file under the real name.

## Conventions

**Transcripts.** A plugin that produces transcripts writes `<stem>.transcript.json` in this shape, so the Recordings window can show, search and play it:

```json
{
  "version": 1,
  "take": "2026-10-11/13.00.00",
  "model": "whisper-large-v3-turbo (MLX)",
  "language": "en",
  "created": "2026-10-11T17:01:02Z",
  "speakers": [ { "id": "tx1", "name": "Host" }, { "id": "tx2", "name": "Guest" } ],
  "segments": [
    { "start": 0.0, "end": 2.86, "speaker": "tx1", "text": "All right, this is the second test.",
      "words": [ { "start": 0.0, "end": 0.32, "text": "All" } ] }
  ]
}
```

`words` is optional. Speaker ids match `take.speakers[].id`; use `speaker` for a take with no named mics. A transcription plugin can also write `.md`, `.srt` and `.vtt` siblings.

**Audio.** Derived audio keeps the source's sample rate and format, and should say what was done to it in the file name.

## Capture plugins (protocol 2)

A capture plugin records something alongside Lavdeck's audio, such as the screen or a webcam. Declare `"protocol": 2`, `"kind": "capture"` and a `capture` section:

```json
"capture": { "arguments": ["--capture"], "sources": ["screen", "camera"] },
"permissions": ["screen-recording", "camera"]
```

When a recording starts, Lavdeck launches the capture executable (the manifest's `executable` unless `capture.executable` names another) with `capture.arguments`, and the two talk in JSON lines:

| Direction | Line |
|---|---|
| Lavdeck → plugin | `{"type":"start","protocol":2,"take":{"folder":"…","stem":"14.02.11"},"settings":{…},"pluginDirectory":"…","hostTime":12345.678}` |
| plugin → Lavdeck | `{"type":"started","source":"screen","startHostTime":12345.71}` when the first frame is written |
| Lavdeck → plugin | `{"type":"stop"}` when the recording stops, then standard input closes |
| plugin → Lavdeck | `{"type":"output","source":"screen","kind":"video","path":"…/14.02.11 Screen.mov","startHostTime":12345.71}` for each file |
| plugin → Lavdeck | `{"type":"stopped"}`, then exit |

`log` and `error` lines work as they do for post plugins. Lavdeck waits up to 30 seconds after `stop` for files to finish writing.

**Time.** Every `hostTime` and `startHostTime` is seconds on the host clock (mach absolute time), the clock AVFoundation and ScreenCaptureKit already stamp sample buffers with. Report your file's first frame time and Lavdeck lines it up with the audio: it stores each file's offset in `<stem>.take.json` and passes it to later jobs as the file's `offset`. Swift plugins can use `HostClock.now()`.

**Permissions.** The capture process runs as Lavdeck's child, so macOS asks for screen and camera access on Lavdeck's behalf the first time, and the grant applies to Lavdeck.

**Files.** Name files `<stem> <Source>.<ext>`, for example `14.02.11 Screen.mov` and `14.02.11 Camera.mov`. A capture plugin can also handle post events with its main executable, for example to compose a picture-in-picture video after the take.

The `LavdeckPluginKit` Swift library has the types: `CaptureCommand.lines()`, `CaptureEvent.started(…).emit()`, `CaptureSpec`, `HostClock`.

## Helpers

- **Python.** [`Examples/plugins/take-info/lavdeck_plugin.py`](../Examples/plugins/take-info/lavdeck_plugin.py) is a single standard-library file you can copy into your plugin. It handles reading the request, emitting messages, picking the main audio file and naming output files.
- **Swift.** The `LavdeckPluginKit` library product has the protocol types: `PluginRequest.readFromStandardInput()`, `PluginMessage.progress(…).emit()`, `TranscriptDocument`, and others. Add it as a SwiftPM dependency.

## Trust

A plugin is an ordinary program that runs as you, with your permissions. Lavdeck doesn't sandbox it. Install plugins only from people you trust, and read what they do. A plugin stays switched off until you turn it on.
