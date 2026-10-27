//! Lavdeck: a menu bar deck for the DJI Mic Series Mobile RX. Live mic status, mono or
//! stereo, gain, recording with screen and camera, a Recordings window and plugins.
//!
//! Phase 1 of the Rust port: the whole interface over a view model, driven by the demo
//! backend (`--demo`); the kit and capture crates plug in behind [`backend::Backend`].

mod app;
mod backend;
mod canvas;
mod deck;
mod demo;
mod demo_video;
mod detail;
mod draw;
mod drive;
mod format;
mod glyph;
mod hotkey;
mod i18n;
mod i18n_text;
mod login;
mod model;
mod panel;
mod permissions;
mod player;
mod prefs;
mod recordings;
mod settings_window;
mod shots;
mod snapshot;
mod status_item;
mod store;
mod theme;
mod util;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    app::run(snapshot::Args::parse(&args));
}
