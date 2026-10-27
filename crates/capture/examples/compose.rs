//! Makes (or remakes) a take's finished video from its folder, as Remake Video does:
//!
//! ```text
//! cargo run -p lavdeck-capture --example compose -- <day folder> <stem> [--h264] [--corner top-left]
//!     [--size small|medium|large] [--shape circle|rounded]
//! ```
//!
//! Reads the take's files (`<stem> Stereo.wav`, `Mix`, `TX1`/`TX2`, `Screen.mov`, `Camera.mov`) and the
//! media offsets in `<stem>.take.json`, then writes `<stem> PiP.mp4`. Needs no permissions.

use std::path::{Path, PathBuf};
use std::time::Instant;

use lavdeck_capture::{
    BubbleShape, BubbleSize, Codec, Corner, TakeDescriptor, TakeFile, TakeRole, VideoComposer,
    VideoSettings,
};

fn offsets(folder: &Path, stem: &str) -> Vec<(String, f64)> {
    let Ok(text) = std::fs::read_to_string(folder.join(format!("{stem}.take.json"))) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    json["media"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| Some((m["file"].as_str()?.to_string(), m["offset"].as_f64()?)))
        .collect()
}

fn take(folder: PathBuf, stem: &str) -> TakeDescriptor {
    let offsets = offsets(&folder, stem);
    let offset = |name: &str| offsets.iter().find(|(f, _)| f == name).map(|(_, o)| *o);
    let mut files = Vec::new();
    let mut names: Vec<String> = std::fs::read_dir(&folder)
        .expect("take folder")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with(&format!("{stem} ")))
        .collect();
    names.sort();
    for name in names {
        let rest = &name[stem.len() + 1..];
        let path = folder.join(&name);
        let file = match rest {
            "Stereo.wav" | "Stereo.m4a" => TakeFile::new(path, TakeRole::Stereo),
            "Mix.wav" | "Mix.m4a" => TakeFile::new(path, TakeRole::Mix),
            r if r.starts_with("TX1.") => TakeFile {
                mic: Some(1),
                ..TakeFile::new(path, TakeRole::Track)
            },
            r if r.starts_with("TX2.") => TakeFile {
                mic: Some(2),
                ..TakeFile::new(path, TakeRole::Track)
            },
            "Screen.mov" => TakeFile::captured(path, "screen", offset(&name)),
            "Camera.mov" => TakeFile::captured(path, "camera", offset(&name)),
            _ => continue,
        };
        files.push(file);
    }
    TakeDescriptor {
        folder,
        stem: stem.into(),
        files,
    }
}

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
    let take = take(PathBuf::from(&args[0]), &args[1]);
    for f in &take.files {
        println!("{:?} {} offset {:?}", f.role, f.path.display(), f.offset);
    }
    let started = Instant::now();
    match VideoComposer::make(&take, &settings, |p| println!("{:.0}%", p * 100.0)) {
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
