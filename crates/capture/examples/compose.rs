//! Makes (or remakes) a take's finished video from its folder, as Remake Video does:
//!
//! ```text
//! cargo run -p lavdeck-capture --example compose -- <day folder> <stem> [--h264] [--corner top-left]
//!     [--size small|medium|large] [--shape circle|rounded]
//! ```
//!
//! Reads the take with the kit's take library (its files and the media offsets in
//! `<stem>.take.json`), then writes `<stem> PiP.mp4`. Needs no permissions.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use lavdeck_capture::{BubbleShape, BubbleSize, Codec, Corner, VideoComposer, VideoSettings};
use lavdeck_kit::takes;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!(
            "usage: compose <day folder> <stem> [--h264] [--corner C] [--size S] [--shape S]"
        );
        std::process::exit(2);
    }
    let mut settings = VideoSettings::default();
    let mut rest = args[2..].iter();
    while let Some(flag) = rest.next() {
        let mut value = || rest.next().map(String::as_str).unwrap_or_default();
        match flag.as_str() {
            "--h264" => settings.codec = Codec::H264,
            "--corner" => settings.bubble_corner = Corner::from_raw(value()).expect("corner"),
            "--size" => settings.bubble_size = BubbleSize::from_raw(value()).expect("size"),
            "--shape" => settings.bubble_shape = BubbleShape::from_raw(value()).expect("shape"),
            other => panic!("unknown option {other}"),
        }
    }
    let Some(take) = takes::take(Path::new(&args[0]), &args[1], None, &HashMap::new()) else {
        eprintln!("No take {} in {}", args[1], args[0]);
        std::process::exit(1);
    };
    for f in &take.descriptor.files {
        println!("{:?} {} offset {:?}", f.role, f.path, f.offset);
    }
    let started = Instant::now();
    match VideoComposer::make(&take.descriptor, &settings, |p| {
        println!("{:.0}%", p * 100.0)
    }) {
        Ok(path) => println!(
            "Saved {} in {:.2} s",
            path.display(),
            started.elapsed().as_secs_f64()
        ),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
