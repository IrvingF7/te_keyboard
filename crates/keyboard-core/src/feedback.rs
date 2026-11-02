//! Optional host-volume snapshots over CDC, independent of HID volume keys.

/// A host-reported percentage, including software gain above 100%.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostVolume {
    pub percent: u16,
    pub muted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleCommand {
    TogglePanel,
    ToggleBacklight,
    PowerStatus,
    Bootloader,
    Volume(Option<HostVolume>),
}

/// Parse complete lines so a bad feedback packet cannot trigger a button
/// shortcut embedded in its contents. Storage and work per byte are bounded.
#[derive(Default)]
pub struct ConsoleInput {
    bytes: [u8; 32],
    len: usize,
    overflow: bool,
}

impl ConsoleInput {
    pub fn push(&mut self, byte: u8) -> Option<ConsoleCommand> {
        if byte == b'\n' {
            let command = if self.overflow {
                None
            } else {
                parse_line(&self.bytes[..self.len])
            };
            *self = Self::default();
            return command;
        }
        if self.len == self.bytes.len() {
            // Discard the WHOLE line, including any suffix resembling d/b/?.
            self.overflow = true;
        } else if !self.overflow {
            self.bytes[self.len] = byte;
            self.len += 1;
        }
        None
    }
}

fn parse_line(bytes: &[u8]) -> Option<ConsoleCommand> {
    let line = std::str::from_utf8(bytes).ok()?;
    // Accept both LF and CRLF, but not embedded control characters.
    let line = line.strip_suffix('\r').unwrap_or(line);
    match line {
        "d" => return Some(ConsoleCommand::TogglePanel),
        "b" => return Some(ConsoleCommand::ToggleBacklight),
        "?" => return Some(ConsoleCommand::PowerStatus),
        "BOOTLOADER" => return Some(ConsoleCommand::Bootloader),
        "VOL ?" => return Some(ConsoleCommand::Volume(None)),
        _ => (),
    }
    let (percent, muted) = line.strip_prefix("VOL ")?.split_once(' ')?;
    if percent.is_empty() || !percent.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let percent: u16 = percent.parse().ok()?;
    if percent > 999 {
        return None;
    }
    let muted = match muted {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    Some(ConsoleCommand::Volume(Some(HostVolume { percent, muted })))
}

/// A stopped helper must not leave an old volume reading on screen forever.
#[derive(Default)]
pub struct HostFeedback {
    latest: Option<(HostVolume, u32)>,
}

impl HostFeedback {
    pub fn set(&mut self, volume: Option<HostVolume>, now: u32) {
        self.latest = volume.map(|value| (value, now));
    }

    pub fn current(&self, now: u32) -> Option<HostVolume> {
        self.latest
            .filter(|(_, received)| now.wrapping_sub(*received) < 2500)
            .map(|(volume, _)| volume)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(input: &mut ConsoleInput, bytes: &[u8]) -> Vec<ConsoleCommand> {
        bytes.iter().filter_map(|&b| input.push(b)).collect()
    }

    #[test]
    fn fragmented_snapshots_controls_and_crlf_share_the_console() {
        let mut input = ConsoleInput::default();
        assert!(feed(&mut input, b"VOL 1").is_empty());
        assert_eq!(
            feed(&mut input, b"8 0\r\nd\nb\n?\nVOL 125 1\nVOL ?\n"),
            [
                ConsoleCommand::Volume(Some(HostVolume {
                    percent: 18,
                    muted: false
                })),
                ConsoleCommand::TogglePanel,
                ConsoleCommand::ToggleBacklight,
                ConsoleCommand::PowerStatus,
                ConsoleCommand::Volume(Some(HostVolume {
                    percent: 125,
                    muted: true
                })),
                ConsoleCommand::Volume(None),
            ]
        );
    }

    #[test]
    fn malformed_or_overflowing_feedback_never_becomes_a_button_press() {
        let mut input = ConsoleInput::default();
        for invalid in [
            "VOL -1 0\n",
            "VOL +1 0\n",
            "VOL 1000 0\n",
            "VOL 5 2\n",
            "VOL 5 0 b\n",
            "VOL d\n",
            "VOL 6\r0 0\n",
            "VOL 999999999999999999999999999999999999b\n",
        ] {
            assert!(
                feed(&mut input, invalid.as_bytes()).is_empty(),
                "{invalid:?}"
            );
        }
        assert!(feed(&mut input, b"VOL \xff 0\n").is_empty());
        assert_eq!(feed(&mut input, b"?\n"), [ConsoleCommand::PowerStatus]);
    }

    #[test]
    fn flash_handoff_requires_an_exact_complete_command() {
        let mut input = ConsoleInput::default();
        assert!(feed(&mut input, b"BOOTLOAD").is_empty());
        assert_eq!(feed(&mut input, b"ER\r\n"), [ConsoleCommand::Bootloader]);
        for invalid in [
            "bootloader\n",
            "BOOTLOADER now\n",
            "VOL BOOTLOADER\n",
            "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxBOOTLOADER\n",
        ] {
            assert!(feed(&mut input, invalid.as_bytes()).is_empty());
        }
        // The host sends a leading newline to discard any partial old command.
        assert!(feed(&mut input, b"VOL 5").is_empty());
        assert_eq!(
            feed(&mut input, b"\nBOOTLOADER\n"),
            [ConsoleCommand::Bootloader]
        );
    }

    #[test]
    fn feedback_expires_across_timer_wrap_and_can_be_cleared_on_disconnect() {
        let mut feedback = HostFeedback::default();
        let volume = HostVolume {
            percent: 0,
            muted: true,
        };
        assert_eq!(feedback.current(0), None);
        feedback.set(Some(volume), u32::MAX - 1000);
        assert_eq!(feedback.current(1498), Some(volume));
        assert_eq!(feedback.current(1499), None);
        feedback.set(Some(volume), 1500);
        feedback.set(None, 1501);
        assert_eq!(feedback.current(1501), None);
    }
}
