//! `~/Library/Logs/Lavdeck/battery.log`: a line whenever a linked transmitter's battery code,
//! charging flag or raw status word changes, to keep studying what the receiver reports (its
//! battery codes aren't mapped yet). Byte-compatible with the Swift app's log.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Write;

use crate::model::AppState;

thread_local! {
    /// Each transmitter's state as last logged.
    static LOGGED: RefCell<BTreeMap<u8, String>> = const { RefCell::new(BTreeMap::new()) };
}

/// The receiver's battery code as the log names it: code 1 is confirmed "High".
fn label(code: i32) -> String {
    if code == 1 {
        "High".into()
    } else {
        format!("Level {code}")
    }
}

/// One transmitter's state, as the log line words it.
pub fn state(code: i32, charging: Option<bool>, raw: Option<u32>) -> String {
    format!(
        "battery code {code} ({}), charging {}, raw {:08x}",
        label(code),
        if charging == Some(true) { "yes" } else { "no" },
        raw.unwrap_or(0)
    )
}

/// Local `yyyy-MM-dd HH:mm:ss`.
fn timestamp() -> String {
    // SAFETY: localtime_r fills the zeroed tm from the current time.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}

pub fn log(s: &AppState) {
    for &index in &s.status.receiver.connected_transmitters {
        let Some(tx) = s.transmitter(index) else {
            continue;
        };
        let Some(code) = tx.battery_level else {
            continue;
        };
        let line_state = state(code, tx.is_charging, tx.raw_flags);
        let changed = LOGGED.with(|l| {
            l.borrow_mut().insert(index, line_state.clone()).as_ref() != Some(&line_state)
        });
        if !changed {
            continue;
        }
        let folder = crate::util::home().join("Library/Logs/Lavdeck");
        let _ = std::fs::create_dir_all(&folder);
        let line = format!(
            "{} TX{index} {}: {line_state}\n",
            timestamp(),
            s.display_name(index)
        );
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(folder.join("battery.log"))
        {
            let _ = file.write_all(line.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::state;

    #[test]
    fn lines_read_as_the_swift_app_wrote_them() {
        assert_eq!(
            state(1, Some(false), Some(0x0000_0400)),
            "battery code 1 (High), charging no, raw 00000400"
        );
        assert_eq!(
            state(3, Some(true), None),
            "battery code 3 (Level 3), charging yes, raw 00000000"
        );
    }
}
