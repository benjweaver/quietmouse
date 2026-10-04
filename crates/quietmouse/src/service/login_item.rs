//! quietmouse.app's login item: the LaunchAgent the bundle carries in
//! `Contents/Library/LaunchAgents`, registered through ServiceManagement.
//!
//! macOS lists an agent registered this way under its app, with the app's name
//! and icon, in System Settings > General > Login Items, and the switch there
//! controls it. A plist written to `~/Library/LaunchAgents` is a "legacy" agent
//! instead, which macOS files under the name on its signing certificate.
//!
//! ServiceManagement registers the agent of the bundle the calling process
//! belongs to, so only quietmoused, running from inside the bundle, can do it.
//! `quietmouse autostart` runs it with [`REGISTER`] or [`UNREGISTER`] and reads
//! the outcome from its exit code.

#![allow(
    unsafe_code,
    reason = "ServiceManagement is an Objective-C API that objc2 only exposes as unsafe"
)]

use std::process::ExitCode;

use objc2::rc::Retained;
use objc2_foundation::NSString;
use objc2_service_management::{SMAppService, SMAppServiceStatus};

/// The agent's label, which is also its plist's name inside the bundle.
pub const LABEL: &str = "io.github.benjweaver.quietmouse";
/// Asks quietmoused to register its login item.
pub const REGISTER: &str = "--register-login-item";
/// Asks quietmoused to unregister its login item.
pub const UNREGISTER: &str = "--unregister-login-item";

/// [`REGISTER`] worked, but the switch in Login Items is off, so macOS won't
/// start the agent until it's switched on.
pub const NEEDS_APPROVAL: u8 = 2;

/// Runs a login item request if quietmoused was started with one.
pub fn handle(argument: &str) -> Option<ExitCode> {
    let result = match argument {
        REGISTER => register(),
        UNREGISTER => unregister().map(|()| ExitCode::SUCCESS),
        _ => return None,
    };
    Some(result.unwrap_or_else(|error| {
        // quietmouse autostart shows this.
        eprintln!("{error}");
        ExitCode::FAILURE
    }))
}

fn service() -> Retained<SMAppService> {
    let plist = NSString::from_str(&format!("{LABEL}.plist"));
    unsafe { SMAppService::agentServiceWithPlistName(&plist) }
}

fn register() -> Result<ExitCode, String> {
    let service = service();
    unsafe { service.registerAndReturnError() }
        .map_err(|error| format!("can't register the login item: {}", error.localizedDescription()))?;
    Ok(match unsafe { service.status() } {
        SMAppServiceStatus::RequiresApproval => ExitCode::from(NEEDS_APPROVAL),
        _ => ExitCode::SUCCESS,
    })
}

fn unregister() -> Result<(), String> {
    let service = service();
    // An agent that was never registered reports NotFound, and unregistering it
    // fails with "Operation not permitted".
    if !matches!(
        unsafe { service.status() },
        SMAppServiceStatus::Enabled | SMAppServiceStatus::RequiresApproval
    ) {
        return Ok(());
    }
    unsafe { service.unregisterAndReturnError() }
        .map_err(|error| format!("can't unregister the login item: {}", error.localizedDescription()))
}
