//! Sending keystrokes, media keys, clicks and desktop actions to the OS on
//! behalf of mouse buttons.
//!
//! One thread owns the platform backend; everything else sends it [`Output`]s.

use std::time::{Duration, Instant};

use crossbeam_channel::Sender;

use crate::keys::{Chord, Desktop, MediaKey, MouseButton};

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod enigo_backend;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use enigo_backend::Backend;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
mod uinput;
#[cfg(target_os = "linux")]
use uinput::Backend;

#[derive(Debug, Clone)]
pub enum Output {
    Chord(Chord),
    Media(MediaKey),
    Click(MouseButton),
    Desktop(Desktop),
}

#[derive(Clone)]
pub struct Injector {
    tx: Sender<Output>,
}

impl Injector {
    /// Starts the output thread. It tries to open the backend straight away, so
    /// that permission prompts appear at startup rather than on the first button
    /// press, and keeps trying if that's refused, so granting the permission
    /// later starts working without a restart.
    pub fn spawn() -> std::io::Result<Self> {
        let (tx, rx) = crossbeam_channel::unbounded::<Output>();
        std::thread::Builder::new().name("inject".into()).spawn(move || {
            let mut opener = BackendOpener::default();
            opener.backend();
            let mut pacer = Pacer::new(DESKTOP_SWITCH_GAP);
            for output in rx {
                let wait = pacer.wait_before(&output, Instant::now());
                if !wait.is_zero() {
                    log::debug!("waiting {wait:?} for the previous desktop switch to finish");
                    std::thread::sleep(wait);
                }
                let Some(backend) = opener.backend() else {
                    continue;
                };
                if let Err(error) = backend.perform(&output) {
                    log::warn!("couldn't send {output:?}: {error:#}");
                }
            }
        })?;
        Ok(Self { tx })
    }

    pub fn send(&self, output: Output) {
        if self.tx.send(output).is_err() {
            log::warn!("keystroke output isn't available");
        }
    }
}

/// Time for a desktop switch's slide animation to finish. macOS, and other
/// desktops with animated switching, drop a switch requested while the previous
/// one is still sliding.
const DESKTOP_SWITCH_GAP: Duration = Duration::from_millis(450);
/// How often to try opening the backend again while it's refused, usually
/// because macOS hasn't been given Accessibility yet.
const BACKEND_RETRY: Duration = Duration::from_secs(5);

/// Opens the platform backend, retrying while it's refused so that granting the
/// permission takes effect without restarting quietmouse.
#[derive(Default)]
struct BackendOpener {
    backend: Option<Backend>,
    last_try: Option<Instant>,
    reported: bool,
}

impl BackendOpener {
    fn backend(&mut self) -> Option<&mut Backend> {
        if self.backend.is_none() && self.last_try.is_none_or(|last| last.elapsed() >= BACKEND_RETRY) {
            self.last_try = Some(Instant::now());
            match Backend::new() {
                Ok(backend) => {
                    if self.reported {
                        log::info!("keystrokes are working now");
                    }
                    self.backend = Some(backend);
                }
                Err(error) => {
                    if !self.reported {
                        log::error!("can't send keystrokes yet: {error:#}");
                        self.reported = true;
                    }
                }
            }
        }
        self.backend.as_mut()
    }
}

/// Keeps desktop switches at least `gap` apart, so a quick second swipe waits
/// its turn instead of being lost mid-animation. Other output is never delayed
/// by it, beyond keeping its place in the queue.
struct Pacer {
    gap: Duration,
    next_switch: Option<Instant>,
}

impl Pacer {
    fn new(gap: Duration) -> Self {
        Self { gap, next_switch: None }
    }

    /// How long to wait before performing `output`, taken from the queue at `now`.
    fn wait_before(&mut self, output: &Output, now: Instant) -> Duration {
        if !matches!(output, Output::Desktop(Desktop::Left | Desktop::Right)) {
            return Duration::ZERO;
        }
        let start = self.next_switch.map_or(now, |earliest| earliest.max(now));
        self.next_switch = Some(start + self.gap);
        start - now
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAP: Duration = Duration::from_millis(450);

    fn switch() -> Output {
        Output::Desktop(Desktop::Right)
    }

    #[test]
    fn queues_back_to_back_switches() {
        let mut pacer = Pacer::new(GAP);
        let start = Instant::now();
        assert_eq!(pacer.wait_before(&switch(), start), Duration::ZERO);
        assert_eq!(
            pacer.wait_before(&switch(), start + Duration::from_millis(100)),
            Duration::from_millis(350)
        );
        // A third quick swipe queues behind the second.
        assert_eq!(
            pacer.wait_before(&switch(), start + Duration::from_millis(150)),
            Duration::from_millis(750)
        );
    }

    #[test]
    fn leaves_other_output_and_spaced_out_switches_alone() {
        let mut pacer = Pacer::new(GAP);
        let start = Instant::now();
        assert_eq!(pacer.wait_before(&switch(), start), Duration::ZERO);
        assert_eq!(
            pacer.wait_before(&Output::Desktop(Desktop::Overview), start),
            Duration::ZERO
        );
        assert_eq!(pacer.wait_before(&Output::Media(MediaKey::Mute), start), Duration::ZERO);
        assert_eq!(
            pacer.wait_before(&switch(), start + Duration::from_secs(1)),
            Duration::ZERO
        );
    }
}
