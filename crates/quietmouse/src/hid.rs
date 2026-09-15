//! HID transport: finding HID++ interfaces with hidapi and turning them into [`Link`]s.

use std::ffi::{CStr, CString};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, select};
use hidapi::{BusType, DeviceInfo, HidApi, HidDevice};
use hidpp::{LOGITECH_VID, LONG_ID, Link, Report, SHORT_ID};

/// Vendor page for HID++ on USB receivers and wired devices: usage 1 is short reports, 2 long.
const HIDPP_USB_PAGE: u16 = 0xFF00;
/// Vendor page for HID++ on Bluetooth LE devices, which only use long reports.
const HIDPP_BLE_PAGE: u16 = 0xFF43;
const HIDPP_BLE_USAGE: u16 = 0x0202;
/// How often reader threads wake to notice that their link was dropped.
const READ_POLL_MS: i32 = 1000;
const MAX_REPORT_LEN: usize = 64;

#[derive(Debug, Clone)]
pub struct Interface {
    pub path: CString,
    pub report_ids: Vec<u8>,
}

/// One HID++ endpoint: a receiver, or a device connected directly.
#[derive(Debug, Clone)]
pub struct Endpoint {
    /// Identifies the endpoint across rescans.
    pub key: String,
    pub product_id: u16,
    pub product: String,
    pub bluetooth: bool,
    pub interfaces: Vec<Interface>,
}

impl Endpoint {
    pub fn receiver_kind(&self) -> Option<&'static str> {
        if self.bluetooth {
            None
        } else {
            hidpp::receiver::kind(self.product_id)
        }
    }

    pub fn describe(&self) -> String {
        let name = if self.product.is_empty() {
            "Logitech device"
        } else {
            &self.product
        };
        let via = if self.bluetooth { "Bluetooth" } else { "USB" };
        format!("{name} ({via}, product {:#06x})", self.product_id)
    }
}

/// Every HID++ endpoint currently attached, with the collections of one device grouped together.
pub fn discover(api: &HidApi) -> Vec<Endpoint> {
    let mut endpoints: Vec<Endpoint> = Vec::new();
    for info in api.device_list() {
        let Some(report_id) = hidpp_report_id(info) else {
            continue;
        };
        let path = info.path().to_owned();
        let key = endpoint_key(&path.to_string_lossy());
        let index = match endpoints.iter().position(|endpoint| endpoint.key == key) {
            Some(index) => index,
            None => {
                endpoints.push(Endpoint {
                    key,
                    product_id: info.product_id(),
                    product: info.product_string().unwrap_or_default().to_owned(),
                    bluetooth: matches!(info.bus_type(), BusType::Bluetooth),
                    interfaces: Vec::new(),
                });
                endpoints.len() - 1
            }
        };
        let interfaces = &mut endpoints[index].interfaces;
        match interfaces.iter_mut().find(|interface| interface.path == path) {
            Some(interface) if !interface.report_ids.contains(&report_id) => interface.report_ids.push(report_id),
            Some(_) => {}
            None => interfaces.push(Interface {
                path,
                report_ids: vec![report_id],
            }),
        }
    }
    endpoints
}

fn hidpp_report_id(info: &DeviceInfo) -> Option<u8> {
    let logitech = info.vendor_id() == LOGITECH_VID;
    match (info.usage_page(), info.usage()) {
        (HIDPP_USB_PAGE, 0x0001) if logitech => Some(SHORT_ID),
        (HIDPP_USB_PAGE, 0x0002) if logitech => Some(LONG_ID),
        (HIDPP_BLE_PAGE, HIDPP_BLE_USAGE) => Some(LONG_ID),
        _ => None,
    }
}

/// Windows gives each top-level collection its own path (`...&col01#...`,
/// `...&col02#...`); dropping that part groups them. Elsewhere one path covers all.
fn endpoint_key(path: &str) -> String {
    let lower = path.to_ascii_lowercase();
    let Some(start) = lower.find("&col") else {
        return lower;
    };
    let digits = &lower[start + 4..];
    let end = start + 4 + digits.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(digits.len());
    format!("{}{}", &lower[..start], &lower[end..])
}

enum Inbound {
    Report(Report),
    Closed,
}

/// A [`Link`] over one endpoint's HID interfaces.
pub struct HidLink {
    writers: Vec<(Vec<u8>, HidDevice)>,
    inbound: Receiver<Inbound>,
    stop: Receiver<()>,
    stopping: bool,
    alive: Arc<AtomicBool>,
}

impl HidLink {
    /// Opens every interface of `endpoint`. When `stop` disconnects (its sender is
    /// dropped), the next receive returns [`hidpp::Error::Stopped`] once, and the
    /// link keeps working afterwards so its owner can restore the device.
    pub fn open(api: &HidApi, endpoint: &Endpoint, stop: Receiver<()>) -> anyhow::Result<Self> {
        let (tx, inbound) = crossbeam_channel::unbounded();
        let alive = Arc::new(AtomicBool::new(true));
        let mut writers = Vec::new();
        // A hidapi handle can't be shared between threads, so each interface is opened
        // twice: a reader thread blocks on one handle, requests go out through the other.
        for interface in &endpoint.interfaces {
            let reader = open(api, &interface.path)?;
            let writer = open(api, &interface.path)?;
            spawn_reader(reader, tx.clone(), Arc::clone(&alive))?;
            writers.push((interface.report_ids.clone(), writer));
        }
        Ok(Self {
            writers,
            inbound,
            stop,
            stopping: false,
            alive,
        })
    }

    /// The handle that accepts `report`, re-framing short reports as long where only long is taken.
    fn writer_for(&self, report: &Report) -> Option<(Report, &HidDevice)> {
        let accepting = |id: u8| {
            self.writers
                .iter()
                .find(|(ids, _)| ids.contains(&id))
                .map(|(_, device)| device)
        };
        accepting(report.report_id())
            .map(|device| (*report, device))
            .or_else(|| accepting(LONG_ID).map(|device| (report.to_long(), device)))
    }
}

impl Drop for HidLink {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}

impl Link for HidLink {
    fn send(&mut self, report: &Report) -> hidpp::Result<()> {
        let (framed, device) = self
            .writer_for(report)
            .ok_or_else(|| hidpp::Error::Io(format!("no interface takes report {:#04x}", report.report_id())))?;
        device
            .write(framed.as_bytes())
            .map_err(|e| hidpp::Error::Io(e.to_string()))?;
        Ok(())
    }

    fn recv(&mut self, timeout: Duration) -> hidpp::Result<Option<Report>> {
        let message = if self.stopping {
            match self.inbound.recv_timeout(timeout) {
                Ok(message) => message,
                Err(RecvTimeoutError::Timeout) => return Ok(None),
                Err(RecvTimeoutError::Disconnected) => return Err(hidpp::Error::Disconnected),
            }
        } else {
            select! {
                recv(self.inbound) -> message => message.map_err(|_| hidpp::Error::Disconnected)?,
                recv(self.stop) -> _ => {
                    self.stopping = true;
                    return Err(hidpp::Error::Stopped);
                }
                default(timeout) => return Ok(None),
            }
        };
        match message {
            Inbound::Report(report) => Ok(Some(report)),
            Inbound::Closed => Err(hidpp::Error::Disconnected),
        }
    }
}

fn open(api: &HidApi, path: &CStr) -> anyhow::Result<HidDevice> {
    api.open_path(path).map_err(|error| {
        let message = error.to_string();
        let hint = if message.contains("not permitted") {
            "\nmacOS blocked access: allow this app under System Settings → Privacy & Security → Input Monitoring"
        } else if message.contains("ermission denied") {
            "\nInstall packaging/linux/70-quietmouse.rules so your user can open Logitech devices"
        } else {
            ""
        };
        anyhow::anyhow!("can't open {}: {message}{hint}", path.to_string_lossy())
    })
}

fn spawn_reader(device: HidDevice, tx: Sender<Inbound>, alive: Arc<AtomicBool>) -> std::io::Result<()> {
    thread::Builder::new().name("hid-reader".into()).spawn(move || {
        let mut buf = [0u8; MAX_REPORT_LEN];
        while alive.load(Ordering::Relaxed) {
            match device.read_timeout(&mut buf, READ_POLL_MS) {
                Ok(0) => {}
                Ok(len) => {
                    if let Some(report) = Report::from_bytes(&buf[..len])
                        && tx.send(Inbound::Report(report)).is_err()
                    {
                        return;
                    }
                }
                Err(error) => {
                    log::debug!("HID read ended: {error}");
                    let _ = tx.send(Inbound::Closed);
                    return;
                }
            }
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_windows_collections() {
        let short = r"\\?\HID#VID_046D&PID_C548&MI_02&Col01#8&2b1c&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        let long = r"\\?\HID#VID_046D&PID_C548&MI_02&Col02#8&2b1c&0&0001#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        assert_eq!(endpoint_key(short).replace("0000#", "0001#"), endpoint_key(long));
        assert!(!endpoint_key(short).contains("&col"));
    }

    #[test]
    fn leaves_other_paths_alone() {
        assert_eq!(endpoint_key("DevSrvsID:4295538853"), "devsrvsid:4295538853");
        assert_eq!(endpoint_key("/dev/hidraw3"), "/dev/hidraw3");
    }
}
