//! The macOS privacy permissions quietmouse needs, and asking macOS for them.
//!
//! Input Monitoring lets it open the mouse, and Accessibility lets it send
//! keystrokes for button and gesture actions. macOS ties both to the exact
//! binary, so every new build, including every Homebrew upgrade, needs them
//! again. Asking from the agent itself makes macOS prompt and list it, ready to
//! switch on, rather than leaving someone to find the file in Finder. The ask
//! has to come from the agent: a command run from a terminal would ask on the
//! terminal's behalf instead.

/// What macOS currently allows this program to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    /// Reading input devices, needed to open the mouse.
    pub input_monitoring: bool,
    /// Sending keystrokes, needed for button and gesture actions.
    pub accessibility: bool,
}

#[cfg(not(target_os = "macos"))]
pub fn request() -> Permissions {
    Permissions {
        input_monitoring: true,
        accessibility: true,
    }
}

/// Asks macOS for Input Monitoring if it hasn't been decided yet, then reports
/// what both permissions are set to. Asking a second time is harmless: macOS
/// only prompts while the answer is undecided.
#[cfg(target_os = "macos")]
pub fn request() -> Permissions {
    if ffi::input_monitoring() == ffi::ACCESS_UNKNOWN {
        // Shows the system prompt, and lists quietmouse under Input Monitoring. On
        // macOS 27 it isn't listed there: switching it on under Device Control and
        // Data Access answers this as allowed too.
        ffi::request_input_monitoring();
    }
    Permissions {
        input_monitoring: ffi::input_monitoring() == ffi::ACCESS_GRANTED,
        accessibility: ffi::accessibility(),
    }
}

/// The three system calls behind [`request`]. They take and return plain
/// integers, with no pointers and nothing to free. The workspace denies
/// `unsafe_code` apart from this module, the macOS and Windows device watchers
/// in `hotplug`, and the Windows stop event in `service`.
#[cfg(target_os = "macos")]
#[allow(
    unsafe_code,
    reason = "the macOS permission APIs are C functions with no safe wrapper"
)]
mod ffi {
    /// `kIOHIDRequestTypeListenEvent`: reading input devices.
    const LISTEN_EVENT: u32 = 1;
    /// `kIOHIDAccessTypeGranted`.
    pub const ACCESS_GRANTED: u32 = 0;
    /// `kIOHIDAccessTypeUnknown`: never asked, so asking will prompt.
    pub const ACCESS_UNKNOWN: u32 = 2;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOHIDCheckAccess(request: u32) -> u32;
        fn IOHIDRequestAccess(request: u32) -> bool;
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }

    pub fn input_monitoring() -> u32 {
        unsafe { IOHIDCheckAccess(LISTEN_EVENT) }
    }

    pub fn request_input_monitoring() -> bool {
        unsafe { IOHIDRequestAccess(LISTEN_EVENT) }
    }

    pub fn accessibility() -> bool {
        unsafe { AXIsProcessTrusted() }
    }
}

/// Whether anything is still refused.
impl Permissions {
    pub fn all_granted(self) -> bool {
        self.input_monitoring && self.accessibility
    }

    /// Whether `self` has gained a permission that `earlier` lacked. macOS
    /// decides once per process whether it may read input devices and keeps to
    /// that answer, so a process refused at startup stays refused however often
    /// it retries: the only way to pick up a new permission is a fresh process.
    pub fn newly_granted_since(self, earlier: Self) -> bool {
        (self.input_monitoring && !earlier.input_monitoring) || (self.accessibility && !earlier.accessibility)
    }
}

/// What to do about a permission macOS hasn't granted, for the log.
pub fn advice(permissions: Permissions) -> Vec<String> {
    let mut advice = Vec::new();
    if !cfg!(target_os = "macos") {
        return advice;
    }
    let settings = "System Settings → Privacy & Security";
    if !permissions.input_monitoring {
        advice.push(format!(
            "macOS is blocking quietmouse from reading devices: allow quietmoused under {settings} → \
             Input Monitoring, or Device Control and Data Access on newer macOS"
        ));
    }
    if !permissions.accessibility {
        advice.push(format!(
            "macOS is blocking quietmouse from sending keystrokes: allow quietmoused under {settings} → \
             Accessibility, or Device Control and Data Access on newer macOS"
        ));
    }
    advice
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spots_a_permission_being_granted() {
        let none = Permissions {
            input_monitoring: false,
            accessibility: false,
        };
        let reading = Permissions {
            input_monitoring: true,
            accessibility: false,
        };
        let all = Permissions {
            input_monitoring: true,
            accessibility: true,
        };
        assert!(reading.newly_granted_since(none));
        assert!(all.newly_granted_since(reading));
        assert!(!none.newly_granted_since(none));
        assert!(!none.newly_granted_since(all));
        assert!(!all.newly_granted_since(all));
        assert!(all.all_granted());
        assert!(!reading.all_granted());
    }

    #[test]
    fn advice_covers_each_missing_permission() {
        let none = Permissions {
            input_monitoring: false,
            accessibility: false,
        };
        let all = Permissions {
            input_monitoring: true,
            accessibility: true,
        };
        assert_eq!(advice(all).len(), 0);
        if cfg!(target_os = "macos") {
            assert_eq!(advice(none).len(), 2);
            assert!(advice(none)[0].contains("Input Monitoring"));
            assert!(advice(none)[1].contains("Accessibility"));
            assert!(
                advice(none)
                    .iter()
                    .all(|line| line.contains("Device Control and Data Access"))
            );
        } else {
            assert_eq!(advice(none).len(), 0);
        }
    }
}
