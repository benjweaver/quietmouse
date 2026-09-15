//! Scripted links and fake devices for unit tests.

use std::collections::VecDeque;
use std::time::Duration;

use crate::{Link, Report, Result};

type Responder = Box<dyn FnMut(&Report) -> Vec<Report>>;

/// A link whose replies come from a closure called on every sent report.
pub(crate) struct MockLink {
    pub sent: Vec<Report>,
    inbound: VecDeque<Report>,
    responder: Responder,
}

impl MockLink {
    pub fn new(responder: impl FnMut(&Report) -> Vec<Report> + 'static) -> Self {
        Self {
            sent: Vec::new(),
            inbound: VecDeque::new(),
            responder: Box::new(responder),
        }
    }
}

impl Link for MockLink {
    fn send(&mut self, report: &Report) -> Result<()> {
        self.sent.push(*report);
        let replies = (self.responder)(report);
        self.inbound.extend(replies);
        Ok(())
    }

    fn recv(&mut self, _timeout: Duration) -> Result<Option<Report>> {
        Ok(self.inbound.pop_front())
    }
}

/// A HID++ 2.0 device at `index` with `features` at feature indices 1, 2, ...
///
/// Root lookups and pings are answered automatically. Every other call goes to
/// `handler(feature, function, params)`, which returns the reply parameters, or
/// `None` to answer with an "invalid function" error.
pub(crate) fn fake_device(
    index: u8,
    features: &[u16],
    mut handler: impl FnMut(u16, u8, &[u8]) -> Option<Vec<u8>> + 'static,
) -> MockLink {
    let features = features.to_vec();
    MockLink::new(move |request| {
        if request.device_index() != index {
            return Vec::new();
        }
        let reply = match request.sub_id() {
            0 => match request.function() {
                0 => {
                    let id = request.u16_at(0);
                    let position = features.iter().position(|&f| f == id).map_or(0, |p| p + 1);
                    Some(vec![u8::try_from(position).unwrap(), 0, 0])
                }
                1 => Some(vec![4, 5, request.param(2)]),
                _ => None,
            },
            i => features
                .get(usize::from(i) - 1)
                .and_then(|&feature| handler(feature, request.function(), request.params())),
        };
        match reply {
            Some(params) => vec![Report::long(index, request.sub_id(), request.address(), &params)],
            None => vec![Report::long(index, 0xFF, request.sub_id(), &[request.address(), 0x07])],
        }
    })
}
