//! Seconds on the host clock (mach absolute time), shared by Lavdeck, AVFoundation and ScreenCaptureKit.
//! Mirrors `HostClock` in the plugin kit; the kit crate has its own copy until the two are merged.

use std::sync::OnceLock;

#[repr(C)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

// libc marks these deprecated in favour of the mach2 crate; they're plain libSystem calls.
unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
}

fn timebase() -> (u32, u32) {
    static TIMEBASE: OnceLock<(u32, u32)> = OnceLock::new();
    *TIMEBASE.get_or_init(|| {
        let mut info = MachTimebaseInfo { numer: 0, denom: 0 };
        // SAFETY: `info` is a valid out pointer.
        unsafe { mach_timebase_info(&mut info) };
        (info.numer, info.denom)
    })
}

pub fn seconds_from_ticks(ticks: u64) -> f64 {
    let (numer, denom) = timebase();
    ticks as f64 * numer as f64 / denom as f64 / 1_000_000_000.0
}

pub fn now() -> f64 {
    // SAFETY: no preconditions.
    seconds_from_ticks(unsafe { mach_absolute_time() })
}

#[cfg(test)]
mod tests {
    use objc2_core_media::CMClock;

    #[test]
    fn matches_core_media_host_clock() {
        // Sample buffers are stamped on CMClockGetHostTimeClock; our seconds must be the same clock.
        let before = super::now();
        let cm = unsafe { CMClock::host_time_clock().time().seconds() };
        let after = super::now();
        assert!(
            before <= cm + 1e-6 && cm <= after + 1e-6,
            "{before} {cm} {after}"
        );
    }
}
