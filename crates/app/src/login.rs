//! Open at login, through ServiceManagement's main-app service.

use objc2_service_management::{SMAppService, SMAppServiceStatus};

pub fn enabled() -> bool {
    // SAFETY: plain ServiceManagement calls.
    unsafe { SMAppService::mainAppService().status() == SMAppServiceStatus::Enabled }
}

/// Registers or unregisters Lavdeck as a login item.
pub fn set(on: bool) -> Result<(), String> {
    // SAFETY: plain ServiceManagement calls.
    let service = unsafe { SMAppService::mainAppService() };
    let r = if on {
        unsafe { service.registerAndReturnError() }
    } else {
        unsafe { service.unregisterAndReturnError() }
    };
    r.map_err(|e| e.localizedDescription().to_string())
}
