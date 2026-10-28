//! Motor policy and encoder decoding, with no peripheral or thread dependencies.
use std::f32::consts::TAU;

pub const RADIANS_PER_COUNT: f32 = TAU / 16384.0;
pub const VOLTAGE_LIMIT: f32 = 0.8;
const COUNTS_PER_STEP: f32 = 16384.0 / 24.0;

pub fn decode_encoder(frame: u32) -> Option<u16> {
    // CRC-6 x^6+x+1 covers angle AND status. A valid checksum does not imply
    // a valid measurement: reject weak/strong fields and loss of tracking too.
    let mut crc = 0u32;
    for bit in (6..24).rev() {
        let feedback = ((crc >> 5) ^ (frame >> bit)) & 1;
        crc = (crc << 1) & 0x3f;
        if feedback != 0 {
            crc ^= 3;
        }
    }
    if frame > 0xffffff || crc != frame & 0x3f || (frame >> 6) & 0x0b != 0 {
        None
    } else {
        Some((frame >> 10) as u16)
    }
}

fn delta(angle: u16, previous: u16) -> i32 {
    let d = i32::from(angle) - i32::from(previous);
    if d > 8192 {
        d - 16384
    } else if d < -8192 {
        d + 16384
    } else {
        d
    }
}

pub struct Detents {
    previous: u16,
    offset: f32,
    velocity: f32,
    pub steps: i32,
    pub position: i64,
}
impl Detents {
    pub fn new(angle: u16) -> Self {
        Self {
            previous: angle,
            offset: 0.0,
            velocity: 0.0,
            steps: 0,
            position: 0,
        }
    }
    pub fn update(&mut self, angle: u16, dt: f32) {
        let movement = delta(angle, self.previous);
        self.previous = angle;
        self.position = self.position.wrapping_add(i64::from(movement));
        self.offset += movement as f32;
        // The 55% threshold adds hysteresis: a little noise at the midpoint
        // cannot repeatedly flip the target between adjacent 15-degree detents.
        while self.offset > 0.55 * COUNTS_PER_STEP {
            self.offset -= COUNTS_PER_STEP;
            self.steps = self.steps.wrapping_add(1);
        }
        while self.offset < -0.55 * COUNTS_PER_STEP {
            self.offset += COUNTS_PER_STEP;
            self.steps = self.steps.wrapping_sub(1);
        }
        if dt > 0.0 {
            let measured = movement as f32 * RADIANS_PER_COUNT / dt;
            self.velocity += dt / (0.010 + dt) * (measured - self.velocity);
        }
    }
    pub fn voltage(&self) -> f32 {
        let error = -self.offset * RADIANS_PER_COUNT;
        if error.abs() < 0.003 && self.velocity.abs() < 0.05 {
            return 0.0;
        }
        // P acts like a spring; D damps motion. This is voltage-mode control,
        // NOT measured torque/current and NOT a cap on USB power consumption.
        (4.0 * error - 0.025 * self.velocity).clamp(-VOLTAGE_LIMIT, VOLTAGE_LIMIT)
    }
}

pub fn electrical_angle(angle: u16) -> f32 {
    // Seven pole pairs. The vendor negates its sensor angle and then uses -1
    // for direction. Our raw sensor counts therefore need effective sign +1.
    7.0 * angle as f32 * RADIANS_PER_COUNT
}
pub fn phase_ticks(q: f32, angle: f32) -> [u32; 3] {
    let alpha = -q * angle.sin();
    let beta = q * angle.cos();
    [
        alpha,
        (-alpha + 1.732_050_8 * beta) / 2.0,
        (-alpha - 1.732_050_8 * beta) / 2.0,
    ]
    .map(|v| (((v + 2.5) / 5.0 * 500.0) as i32).clamp(5, 495) as u32)
}

pub struct SettlingWindow {
    anchor: u16,
    started: u32,
    min: i32,
    max: i32,
}
impl SettlingWindow {
    pub fn new(angle: u16, now: u32) -> Self {
        Self {
            anchor: angle,
            started: now,
            min: 0,
            max: 0,
        }
    }
    pub fn update(&mut self, angle: u16, now: u32) -> bool {
        let offset = delta(angle, self.anchor);
        self.min = self.min.min(offset);
        self.max = self.max.max(offset);
        // Adjacent samples can look steady during a slow drift. Check the
        // span of the entire 200 ms window, including encoder/clock wrap.
        if self.max - self.min > 8 {
            *self = Self::new(angle, now);
        }
        now.wrapping_sub(self.started) >= 200
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sensor_known_vectors() {
        assert_eq!(decode_encoder(0x13483e), Some(1234));
        assert_eq!(decode_encoder(0x134932), Some(1234)); // Push is allowed.
        for frame in [0x13487d, 0x1348b8, 0x1348fb, 0x134a26, 0x13483f] {
            assert_eq!(decode_encoder(frame), None);
        }
    }
    #[test]
    fn restoring_direction_limits_and_zero() {
        assert!(electrical_angle(1100) > electrical_angle(1000));
        let mut d = Detents::new(1000);
        assert_eq!(d.voltage(), 0.0);
        d.update(1100, 0.001);
        assert!(d.voltage() < 0.0);
        assert!(d.voltage().abs() <= VOLTAGE_LIMIT);
        let mut d = Detents::new(1000);
        d.update(900, 0.001);
        assert!(d.voltage() > 0.0);
        assert_eq!(phase_ticks(0.0, 1.0), [250; 3]);
    }
    #[test]
    fn wrap_hysteresis_and_many_turns() {
        let mut d = Detents::new(16380);
        d.update(4, 0.001);
        assert_eq!(d.position, 8);
        let mut d = Detents::new(0);
        d.update(375, 0.001);
        assert_eq!(d.steps, 0);
        d.update(376, 0.001);
        assert_eq!(d.steps, 1);
        d.update(350, 0.001);
        assert_eq!(d.steps, 1);
        d.update(300, 0.001);
        assert_eq!(d.steps, 0);
        for direction in [-1i32, 1] {
            let mut d = Detents::new(0);
            for i in 1..=16384 * 100 {
                d.update((i * direction).rem_euclid(16384) as u16, 0.001);
                assert!(d.voltage().abs() <= VOLTAGE_LIMIT);
            }
            assert_eq!(d.steps, direction * 2400);
            assert_eq!(d.position, i64::from(direction) * 16384 * 100);
        }
    }
    #[test]
    fn alignment_rejects_slow_drift() {
        let mut w = SettlingWindow::new(0, 0);
        for ms in 1..=2000 {
            assert!(!w.update(ms as u16, ms));
        }
        let mut w = SettlingWindow::new(16382, u32::MAX - 100);
        assert!(!w.update(1, 98));
        assert!(w.update(16381, 99));
    }
}
