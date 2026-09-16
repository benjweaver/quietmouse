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

/// One press of a gesture button. A swipe fires as soon as the movement is both
/// far enough and clearly along one axis, at most once per press. A press with
/// no swipe is a tap.
#[derive(Debug, Clone)]
pub struct Gesture {
    dx: i32,
    dy: i32,
    threshold: u16,
    straightness: f32,
    far_enough: bool,
    fired: bool,
}

impl Gesture {
    /// `threshold` is how far the pointer must travel; `straightness` is how
    /// much further along one axis than the other it must be before the
    /// direction counts, where 1 means whichever axis moved more.
    pub fn new(threshold: u16, straightness: f32) -> Self {
        Self {
            dx: 0,
            dy: 0,
            threshold: threshold.max(1),
            straightness: straightness.max(1.0),
            far_enough: false,
            fired: false,
        }
    }

    /// Adds pointer movement, returning the swipe direction the first time the
    /// movement is far enough and pointed clearly enough one way.
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

    #[test]
    fn swipes_fire_once_on_the_axis_that_moved_more() {
        let mut gesture = Gesture::new(50, 1.0);
        assert_eq!(gesture.movement(10, -20), None);
        assert_eq!(gesture.movement(5, -30), Some(Direction::Up));
        assert_eq!(gesture.movement(0, -500), None);
        assert!(!gesture.is_tap());

        let mut sideways = Gesture::new(50, 1.0);
        assert_eq!(sideways.movement(-60, 10), Some(Direction::Left));
        let mut down = Gesture::new(50, 1.0);
        assert_eq!(down.movement(0, 50), Some(Direction::Down));
    }

    #[test]
    fn small_movement_is_still_a_tap() {
        let mut gesture = Gesture::new(50, 1.0);
        assert_eq!(gesture.movement(20, 20), None);
        assert!(gesture.is_tap());
    }

    #[test]
    fn straightness_waits_for_a_clear_direction() {
        let mut gesture = Gesture::new(50, 2.0);
        // Far enough, but nearly diagonal: no direction yet.
        assert_eq!(gesture.movement(45, 40), None);
        // Carrying on sideways settles it.
        assert_eq!(gesture.movement(60, 0), Some(Direction::Right));
        assert!(!gesture.is_tap());
    }

    #[test]
    fn a_swipe_that_never_settles_is_not_a_tap() {
        let mut gesture = Gesture::new(50, 2.0);
        assert_eq!(gesture.movement(45, 45), None);
        assert!(!gesture.is_tap());
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
