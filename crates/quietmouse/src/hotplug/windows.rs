//! Windows: configuration manager notifications for HID interfaces arriving and
//! leaving. They're delivered on a system thread pool, so no window or message
//! loop is needed, and registering for them needs no admin rights.

#![allow(
    unsafe_code,
    reason = "CM_Register_Notification takes a C callback and has no safe wrapper"
)]

use std::ffi::c_void;
use std::ptr;

use anyhow::bail;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_NOTIFY_ACTION, CM_NOTIFY_EVENT_DATA, CM_NOTIFY_FILTER, CM_NOTIFY_FILTER_0, CM_NOTIFY_FILTER_0_0,
    CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE, CM_Register_Notification, CR_SUCCESS, HCMNOTIFICATION,
};
use windows_sys::core::GUID;

/// `GUID_DEVINTERFACE_HID`: every HID collection, one arrival each.
const HID_INTERFACE: GUID = GUID::from_u128(0x4d1e55b2_f16f_11cf_88cb_001111000030);
const ERROR_SUCCESS: u32 = 0;

/// Registers for notifications. The registration is never removed: it lasts as
/// long as the agent does.
pub fn start() -> anyhow::Result<()> {
    let filter = CM_NOTIFY_FILTER {
        cbSize: size_of::<CM_NOTIFY_FILTER>() as u32,
        FilterType: CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
        u: CM_NOTIFY_FILTER_0 {
            DeviceInterface: CM_NOTIFY_FILTER_0_0 {
                ClassGuid: HID_INTERFACE,
            },
        },
        ..Default::default()
    };
    let mut registration: HCMNOTIFICATION = ptr::null_mut();
    let result = unsafe { CM_Register_Notification(&filter, ptr::null(), Some(changed), &mut registration) };
    if result != CR_SUCCESS {
        bail!("CM_Register_Notification failed with {result:#x}");
    }
    Ok(())
}

/// Called on a thread pool thread when a HID interface arrives or leaves.
extern "system" fn changed(
    _registration: HCMNOTIFICATION,
    _context: *const c_void,
    _action: CM_NOTIFY_ACTION,
    _data: *const CM_NOTIFY_EVENT_DATA,
    _size: u32,
) -> u32 {
    super::changed();
    ERROR_SUCCESS
}
