//! Showing a Bolt passkey, and how far the person has got entering it.
//!
//! A mouse takes its passkey as ten left and right clicks, which is easy to
//! lose count of, so the steps are laid out in numbered columns with a row
//! beneath that ticks each one off as the device reports it.

use std::io::{IsTerminal, Write};

use hidpp::receiver::Passkey;

/// A passkey on screen, updated as the device reports each key or click.
pub struct Prompt {
    device: String,
    /// What to press, one entry per step: "left"/"right", or a digit.
    steps: Vec<String>,
    /// What to do once every step is in.
    then: &'static str,
    /// Clicks rather than keys, for the wording.
    clicks: bool,
    entered: u8,
    /// Redrawing the progress row in place, rather than printing each update.
    live: bool,
    /// The live progress row still needs ending with a newline.
    row_open: bool,
}

impl Prompt {
    pub fn new(passkey: &Passkey) -> Self {
        let clicks = passkey.clicks().filter(|_| !passkey.typed);
        let (steps, then) = match clicks {
            Some(clicks) => (
                clicks
                    .iter()
                    .map(|&right| if right { "right" } else { "left" }.to_owned())
                    .collect(),
                "press the left and right buttons together",
            ),
            None => (passkey.digits.chars().map(String::from).collect(), "press Enter"),
        };
        Self {
            device: passkey.device.clone(),
            steps,
            then,
            clicks: clicks.is_some(),
            entered: 0,
            live: std::io::stdout().is_terminal(),
            row_open: false,
        }
    }

    /// Prints the instructions and the steps, then the progress row.
    pub fn show(&mut self) {
        println!("\n{}\n", self.instructions());
        println!("{}", self.numbers());
        println!("{}", self.labels());
        self.draw();
    }

    /// Records that the device has had `entered` steps, and redraws.
    pub fn update(&mut self, entered: u8) {
        self.entered = entered;
        self.draw();
    }

    /// Ends the progress row once the device has submitted the passkey.
    pub fn submitted(&mut self) {
        self.close();
        println!("Checking the passkey...");
    }

    /// Ends the progress row, if it's still open, so later output starts on its own line.
    pub fn close(&mut self) {
        if self.row_open {
            println!();
            self.row_open = false;
        }
    }

    /// Why a passkey was probably rejected, going by how much of it was entered.
    pub fn rejection_hint(&self) -> String {
        let expected = self.steps.len();
        let entered = usize::from(self.entered);
        let unit = if self.clicks { "clicks" } else { "digits" };
        let cause = if entered == expected {
            if self.clicks {
                "one of them was probably the wrong button".to_owned()
            } else {
                "one of them was probably wrong".to_owned()
            }
        } else {
            format!("{} got {entered} of the {expected} {unit}", self.device)
        };
        format!(
            "the receiver rejected the passkey: {cause}. Run `quietmouse pair` again; \
             each attempt gets a new passkey"
        )
    }

    fn instructions(&self) -> String {
        let count = self.steps.len();
        if self.clicks {
            format!(
                "Enter the passkey on {}: click these {count} buttons in order, then {}.",
                self.device, self.then
            )
        } else {
            format!(
                "Enter the passkey on {}: type these {count} digits, then {}.",
                self.device, self.then
            )
        }
    }

    fn width(&self) -> usize {
        self.steps.iter().map(String::len).max().unwrap_or(1).max(2) + 1
    }

    fn row(&self, cells: impl Iterator<Item = String>) -> String {
        let width = self.width();
        let row: String = cells.map(|cell| format!("{cell:>width$}")).collect();
        format!(" {row}")
    }

    fn numbers(&self) -> String {
        self.row((1..=self.steps.len()).map(|n| n.to_string()))
    }

    fn labels(&self) -> String {
        self.row(self.steps.iter().cloned())
    }

    /// Ticks under the steps entered and an arrow under the next, with a status after them.
    fn progress(&self) -> String {
        let entered = usize::from(self.entered);
        let expected = self.steps.len();
        let marks = self.row((0..expected).map(|i| match i.cmp(&entered) {
            std::cmp::Ordering::Less => "✓".to_owned(),
            std::cmp::Ordering::Equal => "↑".to_owned(),
            std::cmp::Ordering::Greater => String::new(),
        }));
        let unit = if self.clicks { "clicks" } else { "digits" };
        let status = match entered.cmp(&expected) {
            std::cmp::Ordering::Less => format!("{entered} of {expected}"),
            std::cmp::Ordering::Equal => format!("all {expected} in; now {}", self.then),
            std::cmp::Ordering::Greater => {
                format!(
                    "{entered} {unit}, {} too many; this attempt will fail",
                    entered - expected
                )
            }
        };
        // Every column is padded, blank ones too, so the status always starts
        // just after the last column.
        format!("{marks}  {status}")
    }

    fn draw(&mut self) {
        let progress = self.progress();
        if self.live {
            // Back to the start of the row and clear it, then redraw.
            print!("\r\x1b[2K{progress}");
            let _ = std::io::stdout().flush();
            self.row_open = true;
        } else {
            println!("{progress}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mouse() -> Prompt {
        let mut prompt = Prompt::new(&Passkey {
            device: "MX Master 3S".to_owned(),
            digits: "000213".to_owned(),
            typed: false,
        });
        prompt.live = false;
        prompt
    }

    #[test]
    fn lays_clicks_out_in_numbered_columns() {
        let prompt = mouse();
        // 213 = 0b00_1101_0101
        assert_eq!(
            prompt.labels(),
            "   left  left right right  left right  left right  left right"
        );
        assert_eq!(
            prompt.numbers(),
            "      1     2     3     4     5     6     7     8     9    10"
        );
    }

    #[test]
    fn ticks_off_what_the_device_has_had() {
        let mut prompt = mouse();
        prompt.entered = 3;
        let progress = prompt.progress();
        assert!(progress.starts_with("      ✓     ✓     ✓     ↑"), "{progress}");
        assert!(progress.ends_with("  3 of 10"), "{progress}");
        // The status lines up after the last column, however far along it is.
        let column = |p: &str| p.find("  3 of").map(|i| p[..i].chars().count());
        assert_eq!(column(&progress), Some(prompt.numbers().chars().count()));

        prompt.entered = 10;
        assert!(
            prompt
                .progress()
                .ends_with("all 10 in; now press the left and right buttons together")
        );
        prompt.entered = 11;
        assert!(
            prompt
                .progress()
                .ends_with("11 clicks, 1 too many; this attempt will fail")
        );
    }

    #[test]
    fn explains_a_rejected_passkey() {
        let mut prompt = mouse();
        prompt.entered = 9;
        assert!(prompt.rejection_hint().contains("MX Master 3S got 9 of the 10 clicks"));
        prompt.entered = 10;
        assert!(prompt.rejection_hint().contains("the wrong button"));
    }

    #[test]
    fn keyboards_type_digits() {
        let mut prompt = Prompt::new(&Passkey {
            device: "MX Keys".to_owned(),
            digits: "482913".to_owned(),
            typed: true,
        });
        prompt.live = false;
        assert_eq!(prompt.labels(), "   4  8  2  9  1  3");
        assert!(prompt.instructions().contains("type these 6 digits, then press Enter"));
    }
}
