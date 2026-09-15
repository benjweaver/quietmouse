//! Sending keystrokes, media keys, clicks and desktop actions to the OS on
//! behalf of mouse buttons.
//!
//! One thread owns the platform backend; everything else sends it [`Output`]s.

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
    /// Starts the output thread. The backend is created straight away so that
    /// permission prompts (macOS Accessibility) appear at startup rather than on
    /// the first button press.
    pub fn spawn() -> std::io::Result<Self> {
        let (tx, rx) = crossbeam_channel::unbounded::<Output>();
        std::thread::Builder::new().name("inject".into()).spawn(move || {
            let mut backend = match Backend::new() {
                Ok(backend) => backend,
                Err(error) => {
                    log::error!("can't send keystrokes: {error:#}");
                    return;
                }
            };
            for output in rx {
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
