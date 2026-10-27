//! Records a few seconds from a named audio input in both formats, then splits the WAV into
//! per-channel tracks: a check of the recorder without the receiver.
//!
//! `cargo run -p lavdeck-kit --example record_input -- "MacBook Pro Microphone" <folder> [seconds]`

use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use lavdeck_kit::recorder::{self, MicPermission};
use lavdeck_kit::{HostClock, MicRecorder, RecordingFormat, audio, run_current_run_loop};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [name, folder, rest @ ..] = &args[..] else {
        eprintln!("usage: record_input <input name> <folder> [seconds]");
        std::process::exit(2);
    };
    let seconds: f64 = rest.first().and_then(|s| s.parse().ok()).unwrap_or(1.5);
    if recorder::microphone_permission() != MicPermission::Authorized {
        eprintln!("microphone access isn't granted; not asking");
        std::process::exit(1);
    }
    for format in RecordingFormat::ALL {
        let path = Path::new(folder).join(format!("input.{}", format.raw()));
        let recorder = MicRecorder::with_device_name(name);
        let (sender, finished) = mpsc::channel();
        recorder.set_on_finish(move |path, error| sender.send((path, error)).unwrap());
        recorder.start(&path, format).expect("start");
        run_current_run_loop(Duration::from_secs_f64(seconds));
        let levels = recorder.channel_levels();
        // Stop from another thread while the main run loop turns, as the app may.
        let stopper = recorder.clone();
        thread::spawn(move || stopper.stop());
        let deadline = Instant::now() + Duration::from_secs(10);
        let (written, error) = loop {
            if let Ok(done) = finished.try_recv() {
                break done;
            }
            assert!(Instant::now() < deadline, "no finish callback");
            run_current_run_loop(Duration::from_millis(50));
        };
        let start = recorder.start_host_time().expect("first sample time");
        println!(
            "{}: {:.2}s, error {error:?}, levels {levels:?}, first sample {:.2}s ago, recording {}",
            written.display(),
            audio::duration(&written).unwrap_or(0.0),
            HostClock::now() - start,
            recorder.is_recording()
        );
        if format == RecordingFormat::Wav {
            let (left, right) = (
                Path::new(folder).join("input TX1.wav"),
                Path::new(folder).join("input TX2.wav"),
            );
            audio::split(&written, &left, &right).expect("split");
            println!(
                "split: {:.2}s + {:.2}s",
                audio::duration(&left).unwrap_or(0.0),
                audio::duration(&right).unwrap_or(0.0)
            );
        } else {
            let solo = Path::new(folder).join("input TX1.m4a");
            println!("extract m4a: {:?}", audio::extract(&written, 0, &solo));
        }
    }
}
