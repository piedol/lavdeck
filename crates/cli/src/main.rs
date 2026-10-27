//! `lavdeck`: the command-line tool. Reads the receiver's status, version and capabilities, sets
//! its channel mode and gain, records, lists takes, and installs and runs plugins.
//!
//! Output matches the Swift tool's line for line, so scripts written against it keep working.

use std::collections::HashMap;
use std::ffi::CStr;
use std::path::{Component, PathBuf};
use std::process::exit;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use lavdeck_kit::{
    ButtonMonitor, CancelToken, ChannelMode, MicCapabilities, MicController, MicRecorder,
    MicStatus, PluginMessageKind, PluginStore, RecorderError, RecordingFormat, defaults, json,
    recorder, runner, takes,
};
use objc2_foundation::{NSDate, NSDateFormatter, NSDateFormatterStyle};
use serde_json::{Value, json};

const USAGE: &str = "usage: lavdeck <command>
  status [--watch SECONDS] [--json]   live receiver/transmitter status (read-only)
  version                             receiver hardware name (read-only)
  caps                                receiver capabilities (read-only)
  set channel mono|stereo             switch the receiver's channel mode
  set gain <dB>                       set receiver gain (-12...12, steps from caps)
  record <file.wav|file.m4a> [--seconds N]
  buttons [--seconds N] [--seize]     print transmitter/receiver button events
  takes                               list recordings (newest first)
  plugins                             list installed plugins
  plugins install <folder>            install a plugin folder (symlinked)
  plugins enable|disable <id>         turn a plugin on or off
  run <plugin-id> <action> <take-id>  run a plugin action (or event) on a take, e.g. 2026-10-11/12.34.54
  (takes and run accept --root <folder> to use another recordings folder)";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first() else {
        println!("{USAGE}");
        exit(2);
    };
    match command.as_str() {
        "status" => status(&args),
        "version" => version(),
        "caps" => caps(),
        "set" => set(&args),
        "record" => record(&args),
        "buttons" => buttons_command(&args),
        "takes" => list_takes(),
        "plugins" => plugins(&args),
        "run" => run(&args),
        _ => {
            println!("{USAGE}");
            exit(2);
        }
    }
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    exit(1);
}

fn option<'a>(name: &str, args: &'a [String]) -> Option<&'a str> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).map(String::as_str)
}

/// `--root` anywhere on the command line, else the app's recordings folder.
fn recordings_root() -> PathBuf {
    let argv: Vec<String> = std::env::args().collect();
    if let Some(root) = option("--root", &argv) {
        return absolute(root);
    }
    defaults::recordings_root(&defaults::domain())
}

fn mic_names() -> HashMap<String, String> {
    defaults::mic_names(&defaults::domain())
}

/// An absolute path with `.` and `..` resolved lexically, like `URL.standardizedFileURL`.
fn absolute(path: &str) -> PathBuf {
    let joined = std::env::current_dir().unwrap_or_default().join(path);
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// One `printf`-style number, so rounding matches `String(format:)` exactly.
fn cformat(format: &CStr, value: f64) -> String {
    let mut buffer = [0u8; 64];
    // SAFETY: the format takes exactly one double; snprintf NUL-terminates within the buffer.
    unsafe {
        libc::snprintf(
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            format.as_ptr(),
            value,
        )
    };
    CStr::from_bytes_until_nul(&buffer)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// MARK: Receiver

fn describe(status: &MicStatus) -> String {
    let rx = &status.receiver;
    let or_q = |s: &Option<String>| s.clone().unwrap_or_else(|| "?".into());
    let noise = match rx.noise_cancelling {
        Some(true) => {
            if rx.noise_cancelling_level == Some(1) {
                "high"
            } else {
                "low"
            }
        }
        Some(false) => "off",
        None => "?",
    };
    let connected: Vec<String> = rx
        .connected_transmitters
        .iter()
        .map(|i| format!("TX{i}"))
        .collect();
    let mut lines = vec![format!(
        "RX  {}  fw {}  {}  gain {}  noise-cancel {noise}  connected {}",
        rx.identity
            .product_name
            .clone()
            .unwrap_or_else(|| "receiver".into()),
        or_q(&rx.identity.firmware),
        rx.channel_mode
            .map(|m| m.to_string())
            .unwrap_or_else(|| "?".into()),
        rx.gain_db
            .map(|g| format!("{g} dB"))
            .unwrap_or_else(|| "?".into()),
        connected.join(","),
    )];
    for (index, tx) in &status.transmitters {
        lines.push(format!(
            "TX{index} {} [{}]  fw {}  {}  {}  meter {}",
            or_q(&tx.identity.product_name),
            or_q(&tx.identity.bluetooth_id),
            or_q(&tx.identity.firmware),
            if rx.connected_transmitters.contains(index) {
                "connected"
            } else {
                "not connected"
            },
            if tx.is_charging == Some(true) {
                "charging"
            } else {
                "on battery"
            },
            tx.meter_level
                .map(|m| m.to_string())
                .unwrap_or_else(|| "?".into()),
        ));
    }
    lines.join("\n")
}

fn status_json(status: &MicStatus) -> String {
    let rx = &status.receiver;
    let transmitters: Vec<Value> = status
        .transmitters
        .iter()
        .map(|(index, tx)| {
            json!({
                "index": index,
                "name": tx.identity.product_name,
                "firmware": tx.identity.firmware,
                "rawFlags": tx.raw_flags.map(|w| format!("{w:x}")),
                "bluetoothID": tx.identity.bluetooth_id,
                "batteryLevel": tx.battery_level,
                "charging": tx.is_charging,
                "meter": tx.meter_level,
            })
        })
        .collect();
    let object = json!({
        "receiver": {
            "name": rx.identity.product_name,
            "firmware": rx.identity.firmware,
            "serial": rx.identity.serial_number,
            "channel": rx.channel_mode.map(|m| m.to_string()),
            "gainDB": rx.gain_db,
            "noiseCancelling": rx.noise_cancelling,
            "noiseCancellingLevel": rx.noise_cancelling_level,
            "connected": rx.connected_transmitters.iter().collect::<Vec<_>>(),
        },
        "transmitters": transmitters,
    });
    json::to_pretty_sorted(&object)
}

/// Opens the receiver and waits (at most 4 s) for its first status push.
fn connected_controller() -> MicController {
    let controller = MicController::new();
    controller.start();
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        if controller.is_connected() && controller.status().receiver.channel_mode.is_some() {
            return controller;
        }
        thread::sleep(Duration::from_millis(100));
    }
    fail(if controller.is_connected() {
        "Receiver connected but sent no status"
    } else {
        "Receiver not found on USB"
    });
}

fn status(args: &[String]) {
    let controller = connected_controller();
    let as_json = args.iter().any(|a| a == "--json");
    if let Some(seconds) = option("--watch", args).and_then(|s| s.parse::<f64>().ok()) {
        controller.set_on_status(move |status| {
            if as_json {
                println!("{}", status_json(&status))
            } else {
                println!("{}\n", describe(&status))
            }
        });
        thread::sleep(Duration::from_secs_f64(seconds.max(0.0)));
    } else {
        thread::sleep(Duration::from_secs(1));
        let status = controller.status();
        println!(
            "{}",
            if as_json {
                status_json(&status)
            } else {
                describe(&status)
            }
        );
    }
    controller.stop();
}

fn version() {
    let controller = connected_controller();
    match controller.version() {
        Ok(name) => println!("{name}"),
        Err(error) => fail(&error.to_string()),
    }
    controller.stop();
}

fn caps() {
    let controller = connected_controller();
    let caps = controller
        .capabilities(0)
        .unwrap_or_else(|e| fail(&e.to_string()));
    if let Some(range) = caps.receiver_gain_range() {
        println!(
            "RX gain: {}...{} dB, step {}",
            range.start(),
            range.end(),
            caps.receiver_gain_step().unwrap_or(0)
        );
    }
    if let Some(mask) = caps.receiver_channel_mode_mask() {
        println!("RX channel mode mask: 0x{mask:02x}");
    }
    for (index, items) in &caps.devices {
        let list: Vec<String> = items.keys().map(|id| format!("0x{id:02x}")).collect();
        let device = if *index == 0 {
            "RX".to_string()
        } else {
            format!("TX{index}")
        };
        println!("{device} capability ids: {}", list.join(" "));
    }
    controller.stop();
}

fn set(args: &[String]) {
    if args.len() != 3 {
        fail(USAGE);
    }
    let controller = connected_controller();
    let result = match args[1].as_str() {
        "channel" => {
            let mode = match args[2].as_str() {
                "mono" => ChannelMode::Mono,
                "stereo" => ChannelMode::Stereo,
                _ => fail("channel must be mono or stereo"),
            };
            controller
                .set_channel(mode)
                .map(|()| println!("channel set to {mode}"))
        }
        "gain" => {
            let Ok(db) = args[2].parse::<i32>() else {
                fail("gain must be an integer dB value")
            };
            controller
                .capabilities(MicCapabilities::GAIN_ID)
                .and_then(|caps| {
                    if let (Some(range), Some(step)) =
                        (caps.receiver_gain_range(), caps.receiver_gain_step())
                        && step > 0
                        && (!range.contains(&db) || (db - range.start()) % step != 0)
                    {
                        fail(&format!(
                            "gain must be within {}...{} in steps of {step}",
                            range.start(),
                            range.end()
                        ));
                    }
                    controller
                        .set_receiver_gain(db)
                        .map(|()| println!("receiver gain set to {db} dB"))
                })
        }
        _ => fail(USAGE),
    };
    if let Err(error) = result {
        fail(&error.to_string());
    }
    controller.stop();
}

// MARK: Recording and buttons

fn record(args: &[String]) {
    if args.len() < 2 {
        fail(USAGE);
    }
    let path = absolute(&args[1]);
    let is_m4a = path
        .extension()
        .is_some_and(|e| e.to_string_lossy().to_lowercase() == "m4a");
    let format = if is_m4a {
        RecordingFormat::M4a
    } else {
        RecordingFormat::Wav
    };
    let seconds = option("--seconds", args)
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(5.0);
    if !recorder::request_microphone_permission() {
        fail(&RecorderError::PermissionDenied.to_string());
    }
    let recorder = MicRecorder::new();
    let (done, finished) = mpsc::channel();
    recorder.set_on_finish(move |path, error| {
        match error {
            Some(error) => println!("finished with error: {error}"),
            None => println!("saved {}", path.display()),
        }
        let _ = done.send(());
    });
    if let Err(error) = recorder.start(&path, format) {
        fail(&error.to_string());
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    println!("recording {}s to {name}…", json::swift_double(seconds));
    // AVFoundation finishes the file on the main thread, so keep its run loop going.
    lavdeck_kit::run_current_run_loop(Duration::from_secs_f64(seconds.max(0.0)));
    recorder.stop();
    while finished.try_recv().is_err() {
        lavdeck_kit::run_current_run_loop(Duration::from_millis(50));
    }
}

fn buttons_command(args: &[String]) {
    let seconds = option("--seconds", args)
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(30.0);
    let mut monitor = ButtonMonitor::new(args.iter().any(|a| a == "--seize"));
    let formatter = NSDateFormatter::new();
    formatter.setDateStyle(NSDateFormatterStyle::NoStyle);
    formatter.setTimeStyle(NSDateFormatterStyle::MediumStyle);
    monitor.set_on_event(move |event| {
        let date =
            NSDate::dateWithTimeIntervalSince1970(json::seconds_since_epoch(event.timestamp));
        let time = formatter.stringFromDate(&date);
        println!(
            "{time}  {} {}",
            event.button.name(),
            if event.is_down { "down" } else { "up" }
        );
    });
    monitor.set_on_raw_report(|id, bytes| {
        let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
        println!("  raw report 0x{id:02x}: {}", hex.join(" "));
    });
    let result = monitor.start();
    println!(
        "listening for {}s (open result 0x{:x})…",
        cformat(c"%.0f", seconds),
        result as u32
    );
    lavdeck_kit::run_current_run_loop(Duration::from_secs_f64(seconds.max(0.0)));
    monitor.stop();
}

// MARK: Takes and plugins

fn list_takes() {
    for take in takes::scan(&recordings_root(), &mic_names()) {
        let d = &take.descriptor;
        let speakers: Vec<&str> = d.speakers.iter().map(|s| s.name.as_str()).collect();
        let duration = d
            .duration
            .map(|s| cformat(c"%5.1fs", s))
            .unwrap_or_else(|| "    ?".into());
        println!(
            "{}  {duration}  {}  [{}]{}",
            d.id,
            d.channel_mode.as_deref().unwrap_or("?"),
            speakers.join(", "),
            if take.has_transcript() {
                "  transcript"
            } else {
                ""
            }
        );
    }
}

fn plugins(args: &[String]) {
    let store = PluginStore::new();
    if args.len() >= 3 && args[1] == "install" {
        match store.install(&absolute(&args[2])) {
            Ok(plugin) => println!("installed {} ({})", plugin.manifest.name, plugin.id()),
            Err(error) => fail(&error.to_string()),
        }
    } else if args.len() >= 3 && (args[1] == "enable" || args[1] == "disable") {
        store.set_enabled(&args[2], args[1] == "enable");
        println!("{} {}d", args[2], args[1]);
    } else {
        let (plugins, problems) = store.discover();
        if plugins.is_empty() {
            println!("no plugins in {}", PluginStore::plugins_folder().display());
        }
        for p in &plugins {
            let mark = if store.is_enabled(p.id()) {
                "●"
            } else {
                "○"
            };
            println!(
                "{mark} {} {}  {}",
                p.manifest.name,
                p.manifest.version,
                p.id()
            );
            let events = p.manifest.events.clone().unwrap_or_default().join(", ");
            let actions: Vec<&str> = p
                .manifest
                .actions
                .iter()
                .flatten()
                .map(|a| a.id.as_str())
                .collect();
            println!("    events: {events}  actions: {}", actions.join(", "));
        }
        for problem in &problems {
            println!("! {problem}");
        }
    }
}

fn run(args: &[String]) {
    if args.len() < 4 {
        fail(USAGE);
    }
    let store = PluginStore::new();
    let Some(plugin) = store.discover().0.into_iter().find(|p| p.id() == args[1]) else {
        fail(&format!("no plugin {}", args[1]));
    };
    let parts: Vec<&str> = args[3].split('/').filter(|p| !p.is_empty()).collect();
    let take = match parts[..] {
        [day, stem] => takes::take(&recordings_root().join(day), stem, None, &mic_names()),
        _ => None,
    };
    let Some(take) = take else {
        fail(&format!("no take {}", args[3]))
    };
    let kind = if plugin.handles(&args[2]) {
        "event"
    } else {
        "action"
    };
    let request = plugin.request(kind, &args[2], &take.descriptor, store.settings(&plugin));
    let started = Instant::now();
    let on_message = |message: &lavdeck_kit::PluginMessage| {
        let text = message.message.clone().unwrap_or_default();
        match message.kind {
            PluginMessageKind::Progress => {
                println!(
                    "  {}  {text}",
                    cformat(c"%3.0f%%", message.fraction.unwrap_or(0.0) * 100.0)
                )
            }
            PluginMessageKind::Output => println!(
                "  → {} ({})",
                message.path.as_deref().unwrap_or(""),
                message.output_kind.as_deref().unwrap_or("")
            ),
            PluginMessageKind::Log => println!("  · {text}"),
            PluginMessageKind::Done => match &message.message {
                Some(m) => println!("  done: {m}"),
                None => println!("  done"),
            },
            PluginMessageKind::Error => println!("  error: {text}"),
        }
    };
    match runner::run(&plugin, &request, on_message, &CancelToken::new()) {
        Ok(result) => println!(
            "finished in {}s, {} output(s)",
            cformat(c"%.1f", started.elapsed().as_secs_f64()),
            result.outputs.len()
        ),
        Err(error) => fail(&error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_round_like_string_format() {
        assert_eq!(cformat(c"%5.1fs", 0.25), "  0.2s");
        assert_eq!(cformat(c"%5.1fs", 125.2405298948288), "125.2s");
        assert_eq!(cformat(c"%3.0f%%", 50.0), " 50%");
    }

    #[test]
    fn paths_are_standardised() {
        assert_eq!(absolute("/a/./b/../c"), std::path::Path::new("/a/c"));
    }

    #[test]
    fn status_reads_like_the_swift_tool() {
        let mut status = MicStatus::new();
        status.receiver.identity.product_name = Some("DJI Mic Mini 2".into());
        status.receiver.identity.firmware = Some("02.03.11.00".into());
        status.receiver.channel_mode = Some(ChannelMode::Stereo);
        status.receiver.gain_db = Some(0);
        status.receiver.noise_cancelling = Some(true);
        status.receiver.noise_cancelling_level = Some(1);
        status.receiver.connected_transmitters = [1].into();
        let mut tx = lavdeck_kit::TransmitterStatus::new(1);
        tx.identity.product_name = Some("DJI Mic Mini".into());
        tx.meter_level = Some(41);
        tx.raw_flags = Some(0x2590_1a00);
        status.transmitters.insert(1, tx);
        assert_eq!(
            describe(&status),
            "RX  DJI Mic Mini 2  fw 02.03.11.00  stereo  gain 0 dB  noise-cancel high  connected TX1\n\
             TX1 DJI Mic Mini [?]  fw ?  connected  on battery  meter 41"
        );
        let json = status_json(&status);
        assert!(json.contains("\"rawFlags\" : \"25901a00\""));
        assert!(json.contains("\"serial\" : null"));
        assert!(json.starts_with("{\n  \"receiver\" : {\n    \"channel\" : \"stereo\",\n    \"connected\" : [\n      1\n    ],"));
    }
}
