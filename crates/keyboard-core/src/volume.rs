//! Interpret cumulative knob counts as signed movement for volume control.

/// Remembers the last reading so a new boot session can start from a baseline.
///
/// Feed valid, ordered knob readings into this tracker. Packet validation,
/// reconnect policy, and conversion into USB volume keys belong to the caller.
#[derive(Default)]
pub struct VolumeSteps {
    // None means we have not seen a reading yet.
    previous: Option<(u32, i32)>,
}

impl VolumeSteps {
    /// Returns signed knob steps since the previous reading in this boot session.
    /// The first reading and a changed boot session establish a baseline and
    /// return zero. Each reading becomes the baseline for the next update.
    ///
    /// Wrapping subtraction matches the knob's wrapping counter. Direction is
    /// unambiguous when successive readings differ by fewer than 2^31 steps.
    pub fn update(&mut self, session: u32, steps: i32) -> i32 {
        let movement = match self.previous {
            Some((previous_session, previous_steps)) if previous_session == session => {
                steps.wrapping_sub(previous_steps)
            }
            _ => 0,
        };
        self.previous = Some((session, steps));
        movement
    }
}

/// USB Consumer-page usages. Zero releases the previously pressed media key.
pub const VOLUME_UP: u16 = 0x00e9;
pub const VOLUME_DOWN: u16 = 0x00ea;
const MAX_PENDING_STEPS: i32 = 32;
const MAX_PENDING_AGE_MS: u32 = 500;

/// Nonblocking press/release sequencing, independent of the USB driver.
pub struct VolumeKeys {
    steps: VolumeSteps,
    session: Option<u32>,
    pending: i32,
    last_movement: u32,
    release_pending: bool,
}

impl Default for VolumeKeys {
    fn default() -> Self {
        Self {
            steps: VolumeSteps::default(),
            session: None,
            pending: 0,
            last_movement: 0,
            // Synchronize the host to "no key pressed" on first connection.
            release_pending: true,
        }
    }
}

impl VolumeKeys {
    /// Forget offline movement while retaining a release owed to the host.
    pub fn reset(&mut self) {
        self.steps = VolumeSteps::default();
        self.session = None;
        self.pending = 0;
        self.release_pending = true;
    }

    pub fn observe(&mut self, session: u32, steps: i32, now: u32) {
        if self.session != Some(session) {
            self.reset();
            self.session = Some(session);
        }
        self.expire(now);
        let movement = self.steps.update(session, steps);
        if movement != 0 {
            // Bound the work after a large jump or a slow host. New movement
            // in the other direction cancels still-unsent movement.
            self.pending = self
                .pending
                .saturating_add(movement)
                .clamp(-MAX_PENDING_STEPS, MAX_PENDING_STEPS);
            self.last_movement = now;
        }
    }

    /// Peek at the next report. Retry this report if USB cannot accept it yet.
    pub fn next_report(&mut self, now: u32) -> Option<u16> {
        self.expire(now);
        if self.release_pending {
            Some(0)
        } else {
            match self.pending.cmp(&0) {
                std::cmp::Ordering::Greater => Some(VOLUME_UP),
                std::cmp::Ordering::Less => Some(VOLUME_DOWN),
                std::cmp::Ordering::Equal => None,
            }
        }
    }

    /// Call only after USB accepts the report returned by `next_report`.
    pub fn report_sent(&mut self, report: u16) {
        match report {
            0 => self.release_pending = false,
            VOLUME_UP => {
                self.pending -= 1;
                self.release_pending = true;
            }
            VOLUME_DOWN => {
                self.pending += 1;
                self.release_pending = true;
            }
            _ => unreachable!("unexpected volume report"),
        }
    }

    fn expire(&mut self, now: u32) {
        if now.wrapping_sub(self.last_movement) >= MAX_PENDING_AGE_MS {
            self.pending = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_reading_only_establishes_a_baseline() {
        for steps in [0, 12, -12, i32::MIN, i32::MAX] {
            let mut tracker = VolumeSteps::default();
            assert_eq!(tracker.update(7, steps), 0);
        }
    }

    #[test]
    fn movement_uses_the_most_recent_reading() {
        let mut tracker = VolumeSteps::default();
        assert_eq!(tracker.update(7, 12), 0);
        // Intermediate radio readings can be missed; retain the net movement.
        assert_eq!(tracker.update(7, 15), 3);
        assert_eq!(tracker.update(7, 13), -2);
        assert_eq!(tracker.update(7, 13), 0);
        assert_eq!(tracker.update(7, 14), 1);
        assert_eq!(tracker.update(7, -2), -16);
    }

    #[test]
    fn a_new_boot_replaces_the_baseline_without_movement() {
        let mut tracker = VolumeSteps::default();
        assert_eq!(tracker.update(7, 12), 0);
        assert_eq!(tracker.update(7, 15), 3);
        assert_eq!(tracker.update(8, 0), 0);
        assert_eq!(tracker.update(8, 1), 1);
        // The first reading received after a reboot may already be nonzero.
        assert_eq!(tracker.update(9, 500), 0);
        assert_eq!(tracker.update(9, 499), -1);
    }

    #[test]
    fn counter_rollover_preserves_direction_within_a_boot() {
        let mut tracker = VolumeSteps::default();
        assert_eq!(tracker.update(7, i32::MAX - 1), 0);
        assert_eq!(tracker.update(7, i32::MIN + 1), 3);
        assert_eq!(tracker.update(7, i32::MAX - 1), -3);
        // A new boot still takes precedence over a numerically adjacent count.
        assert_eq!(tracker.update(8, i32::MIN), 0);
        assert_eq!(tracker.update(8, i32::MIN + 1), 1);
    }

    fn send(keys: &mut VolumeKeys, now: u32) -> Option<u16> {
        let report = keys.next_report(now)?;
        keys.report_sent(report);
        Some(report)
    }

    #[test]
    fn media_keys_release_between_each_step_and_retry_when_busy() {
        let mut keys = VolumeKeys::default();
        keys.observe(7, 12, 0);
        assert_eq!(send(&mut keys, 0), Some(0));
        keys.observe(7, 15, 10);
        for _ in 0..3 {
            // A busy endpoint must not consume or duplicate a step.
            assert_eq!(keys.next_report(10), Some(VOLUME_UP));
            assert_eq!(keys.next_report(10), Some(VOLUME_UP));
            assert_eq!(send(&mut keys, 10), Some(VOLUME_UP));
            assert_eq!(send(&mut keys, 10), Some(0));
        }
        assert_eq!(send(&mut keys, 10), None);
        keys.observe(7, 14, 20);
        assert_eq!(send(&mut keys, 20), Some(VOLUME_DOWN));
        assert_eq!(send(&mut keys, 20), Some(0));
        assert_eq!(send(&mut keys, 20), None);
    }

    #[test]
    fn disconnect_and_reboot_drop_backlog_but_still_release() {
        let mut keys = VolumeKeys::default();
        keys.observe(7, 12, 0);
        send(&mut keys, 0);
        keys.observe(7, 15, 10);
        assert_eq!(send(&mut keys, 10), Some(VOLUME_UP));
        keys.reset();
        assert_eq!(send(&mut keys, 20), Some(0));
        keys.observe(7, 40, 30);
        assert_eq!(send(&mut keys, 30), Some(0));
        assert_eq!(send(&mut keys, 30), None);
        keys.observe(7, 42, 40);
        assert_eq!(send(&mut keys, 40), Some(VOLUME_UP));
        keys.observe(8, 0, 50);
        assert_eq!(send(&mut keys, 50), Some(0));
        assert_eq!(send(&mut keys, 50), None);
    }

    #[test]
    fn stale_or_excessive_movement_cannot_make_an_unbounded_burst() {
        let mut keys = VolumeKeys::default();
        keys.observe(7, 0, u32::MAX - 100);
        send(&mut keys, u32::MAX - 100);
        keys.observe(7, 1000, u32::MAX - 100);
        for _ in 0..MAX_PENDING_STEPS {
            assert_eq!(send(&mut keys, u32::MAX - 100), Some(VOLUME_UP));
            assert_eq!(send(&mut keys, u32::MAX - 100), Some(0));
        }
        assert_eq!(send(&mut keys, u32::MAX - 100), None);
        keys.observe(7, 1003, u32::MAX - 100);
        assert_eq!(send(&mut keys, u32::MAX - 100), Some(VOLUME_UP));
        assert_eq!(send(&mut keys, 399), Some(0));
        assert_eq!(send(&mut keys, 399), None);
        // Fresh movement after expiration must not revive the stale backlog.
        keys.observe(7, 1004, 400);
        assert_eq!(send(&mut keys, 400), Some(VOLUME_UP));
        assert_eq!(send(&mut keys, 400), Some(0));
        assert_eq!(send(&mut keys, 400), None);
    }

    #[test]
    fn reversal_cancels_unsent_steps() {
        let mut keys = VolumeKeys::default();
        keys.observe(7, 0, 0);
        send(&mut keys, 0);
        keys.observe(7, 3, 10);
        keys.observe(7, -1, 20);
        assert_eq!(send(&mut keys, 20), Some(VOLUME_DOWN));
        assert_eq!(send(&mut keys, 20), Some(0));
        assert_eq!(send(&mut keys, 20), None);
    }
}
