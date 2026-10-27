//! The system-wide ⌥⌘R shortcut, through Carbon's `RegisterEventHotKey` (no Accessibility
//! permission needed).

use std::ffi::c_void;

type OSStatus = i32;
type EventTargetRef = *mut c_void;
type EventHandlerRef = *mut c_void;
type EventHotKeyRef = *mut c_void;
type EventRef = *mut c_void;
type EventHandlerCallRef = *mut c_void;
type Handler = extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OSStatus;

#[repr(C)]
#[derive(Default)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> EventTargetRef;
    fn InstallEventHandler(
        target: EventTargetRef,
        handler: Handler,
        count: u32,
        types: *const EventTypeSpec,
        user_data: *mut c_void,
        out: *mut EventHandlerRef,
    ) -> OSStatus;
    fn RegisterEventHotKey(
        code: u32,
        modifiers: u32,
        id: EventHotKeyID,
        target: EventTargetRef,
        options: u32,
        out: *mut EventHotKeyRef,
    ) -> OSStatus;
    fn GetEventParameter(
        event: EventRef,
        name: u32,
        desired_type: u32,
        actual_type: *mut u32,
        size: usize,
        actual_size: *mut usize,
        data: *mut c_void,
    ) -> OSStatus;
}

const fn four(s: &[u8; 4]) -> u32 {
    (s[0] as u32) << 24 | (s[1] as u32) << 16 | (s[2] as u32) << 8 | s[3] as u32
}

/// Lavdeck's hot key signature (the Swift app's `DJMC`).
const SIGNATURE: u32 = four(b"DJMC");
const KEYBOARD: u32 = four(b"keyb");
const HOT_KEY_PRESSED: u32 = 5;
const DIRECT_OBJECT: u32 = four(b"----");
const HOT_KEY_ID_TYPE: u32 = four(b"hkid");
const CMD: u32 = 1 << 8;
const OPTION: u32 = 1 << 11;
/// The R key.
const KEY_R: u32 = 15;

extern "C" fn pressed(_call: EventHandlerCallRef, event: EventRef, _data: *mut c_void) -> OSStatus {
    let mut id = EventHotKeyID::default();
    // SAFETY: reads the hot key id Carbon attached to its own event.
    let status = unsafe {
        GetEventParameter(
            event,
            DIRECT_OBJECT,
            HOT_KEY_ID_TYPE,
            std::ptr::null_mut(),
            std::mem::size_of::<EventHotKeyID>(),
            std::ptr::null_mut(),
            &mut id as *mut _ as *mut c_void,
        )
    };
    if status == 0 && id.signature == SIGNATURE {
        crate::util::main_async(crate::store::toggle_recording);
    }
    0
}

/// Registers ⌥⌘R for Record / Stop. Returns false when another app already holds it.
pub fn register() -> bool {
    let spec = EventTypeSpec {
        event_class: KEYBOARD,
        event_kind: HOT_KEY_PRESSED,
    };
    let mut handler: EventHandlerRef = std::ptr::null_mut();
    let mut hot_key: EventHotKeyRef = std::ptr::null_mut();
    // SAFETY: Carbon's documented registration calls; the handler is a plain function and
    // both registrations live for the app's lifetime.
    unsafe {
        let target = GetApplicationEventTarget();
        if InstallEventHandler(
            target,
            pressed,
            1,
            &spec,
            std::ptr::null_mut(),
            &mut handler,
        ) != 0
        {
            return false;
        }
        RegisterEventHotKey(
            KEY_R,
            CMD | OPTION,
            EventHotKeyID {
                signature: SIGNATURE,
                id: 1,
            },
            target,
            0,
            &mut hot_key,
        ) == 0
    }
}
