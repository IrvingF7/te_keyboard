//! Small state machines replace blocking waits and repeated button actions.

pub struct Button {
    raw: bool,
    stable: bool,
    changed: u32,
}

impl Button {
    pub fn new(pressed: bool, now: u32) -> Self {
        Self {
            raw: pressed,
            stable: pressed,
            changed: now,
        }
    }
    pub fn update(&mut self, pressed: bool, now: u32) -> bool {
        if pressed != self.raw {
            self.raw = pressed;
            self.changed = now;
        }
        // wrapping_sub handles the millisecond counter wrapping after ~50 days.
        if self.raw == self.stable || now.wrapping_sub(self.changed) < 30 {
            return false;
        }
        self.stable = self.raw;
        self.stable // One event on press; release and holding are not events.
    }
    pub fn pressed(&self) -> bool {
        self.stable
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Sleep,
    Wake,
    Redraw,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Awake,
    Sleeping,
    Asleep,
    Waking,
}

pub struct PanelPower {
    state: State,
    requested: bool,
    backlight: bool,
    changed: u32,
}
impl Default for PanelPower {
    fn default() -> Self {
        Self {
            state: State::Awake,
            requested: true,
            backlight: true,
            changed: 0,
        }
    }
}
impl PanelPower {
    pub fn toggle_panel(&mut self) {
        self.requested = !self.requested;
    }
    pub fn toggle_backlight(&mut self) {
        self.backlight = !self.backlight;
    }
    pub fn requested(&self) -> bool {
        self.requested
    }
    pub fn backlight_requested(&self) -> bool {
        self.backlight
    }
    pub fn awake(&self) -> bool {
        self.state == State::Awake
    }
    pub fn backlight_on(&self) -> bool {
        self.awake() && self.requested && self.backlight
    }
    pub fn status(&self) -> &'static str {
        match self.state {
            State::Awake => "ON",
            State::Sleeping => "SLEEPING",
            State::Asleep => "OFF",
            State::Waking => "WAKING",
        }
    }
    pub fn tick(&mut self, now: u32) -> Action {
        if matches!(self.state, State::Sleeping | State::Waking) {
            if now.wrapping_sub(self.changed) < 120 {
                return Action::None;
            }
            self.state = if self.state == State::Sleeping {
                State::Asleep
            } else {
                State::Awake
            };
            if self.awake() && self.requested {
                return Action::Redraw;
            }
        }
        // Remember quick button presses, but never reverse a hardware command
        // until the panel's required settling interval has elapsed.
        if self.awake() && !self.requested {
            self.state = State::Sleeping;
            self.changed = now;
            return Action::Sleep;
        }
        if self.state == State::Asleep && self.requested {
            self.state = State::Waking;
            self.changed = now;
            return Action::Wake;
        }
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounce_hold_release_and_clock_wrap() {
        let mut b = Button::new(false, u32::MAX - 20);
        assert!(!b.update(true, u32::MAX - 10));
        assert!(!b.update(false, u32::MAX - 5));
        assert!(!b.update(true, u32::MAX));
        assert!(!b.update(true, 28));
        assert!(b.update(true, 29));
        assert!(!b.update(true, 1000));
        assert!(!b.update(false, 1001));
        assert!(!b.update(false, 1031));
        assert!(!b.pressed());
    }
    #[test]
    fn sleep_wake_preserves_dark_preference() {
        let mut p = PanelPower::default();
        p.toggle_backlight();
        p.toggle_panel();
        assert_eq!(p.tick(10), Action::Sleep);
        p.toggle_panel();
        assert_eq!(p.tick(129), Action::None);
        assert_eq!(p.tick(130), Action::Wake);
        assert_eq!(p.tick(249), Action::None);
        assert_eq!(p.tick(250), Action::Redraw);
        assert!(!p.backlight_on());
        p.toggle_backlight();
        assert!(p.backlight_on());
    }
    #[test]
    fn cancel_wake_and_wrap_settle_timer() {
        let mut p = PanelPower::default();
        p.toggle_panel();
        assert_eq!(p.tick(u32::MAX - 50), Action::Sleep);
        p.toggle_panel();
        assert_eq!(p.tick(69), Action::Wake);
        p.toggle_panel();
        assert_eq!(p.tick(188), Action::None);
        assert_eq!(p.tick(189), Action::Sleep);
        assert!(!p.backlight_on());
    }
}
