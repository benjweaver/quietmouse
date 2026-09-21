//! Interpreting diverted input: which buttons changed, which way a gesture
//! swiped, and how many steps the thumb wheel turned.

use std::time::{Duration, Instant};

/// A swipe is one continuous movement. When the pointer sits still for this
/// long mid-press, the travel so far is forgotten, so slow drift while holding
/// the button for a tap never adds up to a swipe. Long enough to ride out the
/// gaps a Bluetooth link puts between reports during a real swipe.
pub const DRIFT_RESET: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// Controls pressed and released between two snapshots of held controls.
pub fn button_changes(before: &[u16], after: &[u16]) -> (Vec<u16>, Vec<u16>) {
    let pressed = after.iter().copied().filter(|cid| !before.contains(cid)).collect();
    let released = before.iter().copied().filter(|cid| !after.contains(cid)).collect();
    (pressed, released)
}

/// One press of a gesture button. A swipe fires as soon as one unbroken
/// movement is both far enough and clearly along one axis, at most once per
/// press. A press with no swipe is a tap.
///
/// Three things keep a swipe from firing by accident. `threshold` is the
/// deadzone: nudging the mouse while pressing the button never reaches it.
/// [`DRIFT_RESET`] means the travel only counts while the pointer keeps moving,
/// so holding the button still for a tap can't slowly wander into a swipe. And
/// the first movement report of a press is ignored, because it isn't movement
/// during the press at all: the mouse hands over whatever its sensor gathered
/// before the button went down, all at once. On an MX Master 3S that was
/// hundreds of counts after moving over to click a window, against one or two
/// per report while the button is held, so a tap straight after moving the
/// mouse fired as a swipe back the way the hand had come.
#[derive(Debug, Clone)]
pub struct Gesture {
    dx: i32,
    dy: i32,
    threshold: u16,
    straightness: f32,
    /// When the last movement arrived, to spot the pointer going still.
    moved: Option<Instant>,
    /// Whether the report carrying movement from before the press has gone by.
    flushed: bool,
    far_enough: bool,
    fired: bool,
}

impl Gesture {
    /// `threshold` is how far the pointer must travel in one movement;
    /// `straightness` is how much further along one axis than the other it must
    /// be before the direction counts, where 1 means whichever axis moved more.
    pub fn new(threshold: u16, straightness: f32) -> Self {
        Self {
            dx: 0,
            dy: 0,
            threshold: threshold.max(1),
            straightness: straightness.max(1.0),
            moved: None,
            flushed: false,
            far_enough: false,
            fired: false,
        }
    }

    /// Adds pointer movement reported at `now`, returning the swipe direction
    /// the first time one unbroken movement is far enough and pointed clearly
    /// enough one way.
    pub fn movement(&mut self, dx: i16, dy: i16, now: Instant) -> Option<Direction> {
        if self.fired {
            return None;
        }
        // The first report holds movement from before the press; see [`Gesture`].
        if !self.flushed {
            self.flushed = true;
            return None;
        }
        // Some devices keep reporting while the pointer sits still. Those say
        // nothing about whether a movement is still going, so they're ignored
        // rather than allowed to hold the pause below open.
        if dx == 0 && dy == 0 {
            return None;
        }
        // A pause means the last movement ended, whatever it added up to.
        if self
            .moved
            .is_some_and(|last| now.saturating_duration_since(last) >= DRIFT_RESET)
        {
            self.dx = 0;
            self.dy = 0;
        }
        self.moved = Some(now);
        self.dx = self.dx.saturating_add(i32::from(dx));
        self.dy = self.dy.saturating_add(i32::from(dy));
        let distance_squared = i64::from(self.dx).pow(2) + i64::from(self.dy).pow(2);
        if distance_squared < i64::from(self.threshold).pow(2) {
            return None;
        }
        self.far_enough = true;
        let direction = self.direction()?;
        self.fired = true;
        Some(direction)
    }

    /// Which way the movement points, or `None` while it's too diagonal to tell.
    fn direction(&self) -> Option<Direction> {
        let sideways = self.dx.unsigned_abs() as f32;
        let vertical = self.dy.unsigned_abs() as f32;
        // HID pointer y grows downwards.
        if sideways > vertical * self.straightness {
            Some(if self.dx > 0 { Direction::Right } else { Direction::Left })
        } else if vertical >= sideways * self.straightness {
            Some(if self.dy > 0 { Direction::Down } else { Direction::Up })
        } else {
            None
        }
    }

    /// Whether releasing now counts as a tap: the pointer never went far enough
    /// to be a swipe. A swipe that stayed too diagonal to place is neither.
    pub fn is_tap(&self) -> bool {
        !self.far_enough
    }
}

/// Converts thumb wheel rotation into whole steps, carrying the remainder over.
#[derive(Debug, Clone)]
pub struct Ticker {
    step: i32,
    carry: i32,
}

impl Ticker {
    pub fn new(step: u16) -> Self {
        Self {
            step: i32::from(step.max(1)),
            carry: 0,
        }
    }

    pub fn reset(&mut self) {
        self.carry = 0;
    }

    /// Signed number of whole steps completed, including this rotation.
    pub fn feed(&mut self, rotation: i16) -> i32 {
        self.carry += i32::from(rotation);
        let steps = self.carry / self.step;
        self.carry -= steps * self.step;
        steps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_presses_and_releases() {
        assert_eq!(button_changes(&[], &[0xC3]), (vec![0xC3], vec![]));
        assert_eq!(button_changes(&[0xC3, 0x53], &[0x53, 0x56]), (vec![0x56], vec![0xC3]));
        assert_eq!(button_changes(&[0xC3], &[]), (vec![], vec![0xC3]));
    }

    /// A press whose first report, the one carrying movement from before the
    /// button went down, has already arrived: the state the other tests start in.
    fn pressed(threshold: u16, straightness: f32) -> Gesture {
        let mut gesture = Gesture::new(threshold, straightness);
        assert_eq!(gesture.movement(0, 0, Instant::now()), None);
        gesture
    }

    #[test]
    fn movement_from_before_the_press_is_not_a_swipe() {
        let mut now = swiping();
        // Captured from an MX Master 3S: a tap just after moving the mouse over
        // to click a window. The first report is the movement that got it there.
        let mut gesture = Gesture::new(150, 1.5);
        assert_eq!(gesture.movement(-754, -17, now()), None);
        for (dx, dy) in [(0, -1), (0, -1), (1, 0)] {
            assert_eq!(gesture.movement(dx, dy, now()), None);
        }
        assert!(gesture.is_tap());

        // A real swipe straight after still fires once the stale report is past.
        let mut swipe = Gesture::new(150, 1.5);
        assert_eq!(swipe.movement(-754, -17, now()), None);
        assert_eq!(swipe.movement(-80, 2, now()), None);
        assert_eq!(swipe.movement(-90, -3, now()), Some(Direction::Left));
    }

    /// Reports arriving in one unbroken movement, a swipe's worth apart.
    fn swiping() -> impl FnMut() -> Instant {
        let mut at = Instant::now();
        move || {
            at += DRIFT_RESET / 10;
            at
        }
    }

    #[test]
    fn swipes_fire_once_on_the_axis_that_moved_more() {
        let mut now = swiping();
        let mut gesture = pressed(50, 1.0);
        assert_eq!(gesture.movement(10, -20, now()), None);
        assert_eq!(gesture.movement(5, -30, now()), Some(Direction::Up));
        assert_eq!(gesture.movement(0, -500, now()), None);
        assert!(!gesture.is_tap());

        let mut sideways = pressed(50, 1.0);
        assert_eq!(sideways.movement(-60, 10, now()), Some(Direction::Left));
        let mut down = pressed(50, 1.0);
        assert_eq!(down.movement(0, 50, now()), Some(Direction::Down));
    }

    #[test]
    fn small_movement_is_still_a_tap() {
        let mut now = swiping();
        let mut gesture = pressed(50, 1.0);
        assert_eq!(gesture.movement(20, 20, now()), None);
        assert!(gesture.is_tap());
    }

    #[test]
    fn straightness_waits_for_a_clear_direction() {
        let mut now = swiping();
        let mut gesture = pressed(50, 2.0);
        // Far enough, but nearly diagonal: no direction yet.
        assert_eq!(gesture.movement(45, 40, now()), None);
        // Carrying on sideways settles it.
        assert_eq!(gesture.movement(60, 0, now()), Some(Direction::Right));
        assert!(!gesture.is_tap());
    }

    #[test]
    fn a_swipe_that_never_settles_is_not_a_tap() {
        let mut now = swiping();
        let mut gesture = pressed(50, 2.0);
        assert_eq!(gesture.movement(45, 45, now()), None);
        assert!(!gesture.is_tap());
    }

    #[test]
    fn drift_between_pauses_never_adds_up_to_a_swipe() {
        let start = Instant::now();
        let mut gesture = pressed(50, 1.0);
        // Holding the button still, wandering a little every so often: each
        // nudge is well inside the deadzone, and the pauses forget the last one.
        for step in 1..20 {
            let at = start + DRIFT_RESET * step;
            assert_eq!(gesture.movement(20, 5, at), None, "drift fired a swipe at step {step}");
        }
        assert!(gesture.is_tap());
    }

    #[test]
    fn reports_with_no_movement_dont_hold_the_pause_open() {
        let start = Instant::now();
        let mut gesture = pressed(50, 1.0);
        assert_eq!(gesture.movement(40, 0, start), None);
        // A device chattering away while the pointer sits still mustn't make
        // the drift look like one continuous movement.
        for step in 1..10 {
            assert_eq!(gesture.movement(0, 0, start + DRIFT_RESET / 4 * step), None);
        }
        let later = start + DRIFT_RESET;
        assert_eq!(
            gesture.movement(40, 0, later),
            None,
            "the pause should have reset the travel"
        );
        assert!(gesture.is_tap());
    }

    #[test]
    fn a_pause_mid_press_starts_the_next_swipe_afresh() {
        let start = Instant::now();
        let mut gesture = pressed(50, 1.0);
        // Half a swipe left, then a pause.
        assert_eq!(gesture.movement(-40, 0, start), None);
        // Carrying on rightwards after the pause is its own movement, so it
        // doesn't cancel out against the travel before it.
        let after = start + DRIFT_RESET;
        assert_eq!(gesture.movement(50, 0, after), Some(Direction::Right));
    }

    #[test]
    fn a_swipe_rides_out_gaps_shorter_than_the_reset() {
        let start = Instant::now();
        let mut gesture = pressed(50, 1.0);
        assert_eq!(gesture.movement(30, 0, start), None);
        let hiccup = start + DRIFT_RESET - Duration::from_millis(1);
        assert_eq!(gesture.movement(25, 0, hiccup), Some(Direction::Right));
    }

    #[test]
    fn ticker_carries_remainders_both_ways() {
        let mut ticker = Ticker::new(5);
        assert_eq!(ticker.feed(3), 0);
        assert_eq!(ticker.feed(3), 1);
        assert_eq!(ticker.feed(-8), -1);
        assert_eq!(ticker.feed(-3), -1);
        ticker.reset();
        assert_eq!(ticker.feed(4), 0);
        assert_eq!(ticker.feed(11), 3);
    }
}
