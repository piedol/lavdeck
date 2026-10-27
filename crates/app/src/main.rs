//! Lavdeck: a menu bar deck for the DJI Mic Series Mobile RX. Live mic status, mono or
//! stereo, gain, recording with screen and camera, a Recordings window and plugins.
//!
//! The interface works over a view model ([`store`]) and a [`backend::Backend`]: the real one
//! over the kit and capture crates, or the demo's made-up mics and takes (`--demo`).

mod app;
mod backend;
mod battery;
mod canvas;
mod convert;
mod deck;
mod demo;
mod demo_video;
mod detail;
mod draw;
mod drive;
mod errors;
mod format;
mod glyph;
mod hotkey;
mod i18n;
mod i18n_text;
mod live;
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
