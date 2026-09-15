//! Interpreting diverted input: which buttons changed, which way a gesture
//! swiped, and how many steps the thumb wheel turned.

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

/// One press of a gesture button. A swipe fires as soon as the movement crosses
/// the threshold, at most once per press; a press with no swipe is a tap.
#[derive(Debug, Clone)]
pub struct Gesture {
    dx: i32,
    dy: i32,
    threshold: u16,
    fired: bool,
}

impl Gesture {
    pub fn new(threshold: u16) -> Self {
        Self {
            dx: 0,
            dy: 0,
            threshold: threshold.max(1),
            fired: false,
        }
    }

    /// Adds pointer movement; returns the swipe direction the first time the
    /// distance travelled reaches the threshold.
    pub fn movement(&mut self, dx: i16, dy: i16) -> Option<Direction> {
        if self.fired {
            return None;
        }
        self.dx = self.dx.saturating_add(i32::from(dx));
        self.dy = self.dy.saturating_add(i32::from(dy));
        let distance_squared = i64::from(self.dx).pow(2) + i64::from(self.dy).pow(2);
        if distance_squared < i64::from(self.threshold).pow(2) {
            return None;
        }
        self.fired = true;
        // HID pointer y grows downwards.
        Some(if self.dx.abs() > self.dy.abs() {
            if self.dx > 0 { Direction::Right } else { Direction::Left }
        } else if self.dy > 0 {
            Direction::Down
        } else {
            Direction::Up
        })
    }

    /// Whether releasing now counts as a tap.
    pub fn is_tap(&self) -> bool {
        !self.fired
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

    #[test]
    fn swipes_fire_once_on_the_dominant_axis() {
        let mut gesture = Gesture::new(50);
        assert_eq!(gesture.movement(10, -20), None);
        assert_eq!(gesture.movement(5, -30), Some(Direction::Up));
        assert_eq!(gesture.movement(0, -500), None);
        assert!(!gesture.is_tap());

        let mut sideways = Gesture::new(50);
        assert_eq!(sideways.movement(-60, 10), Some(Direction::Left));
        let mut down = Gesture::new(50);
        assert_eq!(down.movement(0, 50), Some(Direction::Down));
    }

    #[test]
    fn small_movement_is_still_a_tap() {
        let mut gesture = Gesture::new(50);
        assert_eq!(gesture.movement(20, 20), None);
        assert!(gesture.is_tap());
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
