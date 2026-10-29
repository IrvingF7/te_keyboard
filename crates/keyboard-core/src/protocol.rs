//! Fixed 32-byte v1 protocol. Explicit encoding avoids Rust struct padding and
//! remains interoperable with the previous firmware during staged migration.
pub const KNOB_MAC: [u8; 6] = [0x40, 0x4c, 0xca, 0x5c, 0x39, 0x30];
pub const DISPLAY_MAC: [u8; 6] = [0x80, 0xb5, 0x4e, 0xc7, 0xf3, 0xdc];
pub const ENCODER_VALID: u8 = 1;
pub const BUTTON_PRESSED: u8 = 2;
pub const HAPTICS_ACTIVE: u8 = 4;
pub const HAPTICS_FAULT: u8 = 8;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Knob = 1,
    Display = 2,
}
impl Role {
    pub fn peer(self) -> Self {
        if self == Self::Knob {
            Self::Display
        } else {
            Self::Knob
        }
    }
    pub fn mac(self) -> [u8; 6] {
        if self == Self::Knob {
            KNOB_MAC
        } else {
            DISPLAY_MAC
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Packet {
    pub role: Role,
    pub flags: u8,
    pub session: u32,
    pub sequence: u32,
    pub echo_session: u32,
    pub echo_sequence: u32,
    pub steps: i32,
    pub angle: u16,
}
impl Packet {
    pub fn encode(self) -> [u8; 32] {
        let mut b = [0u8; 32]; // Reserved bytes stay zero.
        b[0..4].copy_from_slice(&0x544b4c31u32.to_le_bytes());
        b[4] = 1;
        b[5] = self.role as u8;
        b[6] = self.flags;
        for (start, value) in [
            (8, self.session),
            (12, self.sequence),
            (16, self.echo_session),
            (20, self.echo_sequence),
        ] {
            b[start..start + 4].copy_from_slice(&value.to_le_bytes());
        }
        b[24..28].copy_from_slice(&self.steps.to_le_bytes());
        b[28..30].copy_from_slice(&self.angle.to_le_bytes());
        b
    }
    pub fn decode(b: &[u8], expected: Role) -> Option<Self> {
        if b.len() != 32
            || b[0..4] != 0x544b4c31u32.to_le_bytes()
            || b[4] != 1
            || b[5] != expected as u8
        {
            return None;
        }
        let word = |i| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
        let p = Self {
            role: expected,
            flags: b[6],
            session: word(8),
            sequence: word(12),
            echo_session: word(16),
            echo_sequence: word(20),
            steps: word(24) as i32,
            angle: u16::from_le_bytes([b[28], b[29]]),
        };
        if p.session == 0 || p.sequence == 0 || p.angle >= 16384 {
            None
        } else {
            Some(p)
        }
    }
}
fn newer(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

pub struct LinkState {
    role: Role,
    session: u32,
    sequence: u32,
    last_echo: u32,
    last_send: u32,
    last_steps: i32,
    last_flags: u8,
    last_receive: u32,
    last_round_trip: Option<u32>,
    pub remote: Option<Packet>,
    pub received: u32,
}
impl LinkState {
    pub fn new(role: Role, session: u32) -> Self {
        assert_ne!(session, 0);
        Self {
            role,
            session,
            sequence: 0,
            last_echo: 0,
            last_send: 0,
            last_steps: 0,
            last_flags: 0,
            last_receive: 0,
            last_round_trip: None,
            remote: None,
            received: 0,
        }
    }
    pub fn receive(&mut self, p: Packet, now: u32) {
        if p.role != self.role.peer() {
            return;
        }
        if let Some(old) = self.remote {
            if p.session == old.session && !newer(p.sequence, old.sequence) {
                return;
            }
            if p.session != old.session {
                self.last_round_trip = None;
                self.last_echo = 0;
            }
        }
        self.remote = Some(p);
        self.last_receive = now;
        self.received = self.received.wrapping_add(1);
        if p.echo_session == self.session
            && p.echo_sequence != 0
            && self.sequence.wrapping_sub(p.echo_sequence) <= 8
            && (self.last_echo == 0 || newer(p.echo_sequence, self.last_echo))
        {
            self.last_echo = p.echo_sequence;
            self.last_round_trip = Some(now);
        }
    }
    pub fn outgoing(&mut self, now: u32, angle: u16, steps: i32, flags: u8) -> Option<Packet> {
        let elapsed = now.wrapping_sub(self.last_send);
        let changed =
            self.role == Role::Knob && (steps != self.last_steps || flags != self.last_flags);
        // Keep idle heartbeats at 4 Hz, but deliver turns within 20 ms instead
        // of waiting for the next heartbeat. Packets remain cumulative v1.
        if elapsed < 20 || (!changed && elapsed < 250) {
            return None;
        }
        self.last_send = now;
        self.last_steps = steps;
        self.last_flags = flags;
        self.sequence = self.sequence.wrapping_add(1).max(1);
        Some(Packet {
            role: self.role,
            flags,
            session: self.session,
            sequence: self.sequence,
            echo_session: self.remote.map_or(0, |p| p.session),
            echo_sequence: self.remote.map_or(0, |p| p.sequence),
            steps,
            angle,
        })
    }
    pub fn age(&self, now: u32) -> u32 {
        now.wrapping_sub(self.last_receive)
    }
    pub fn connected(&self, now: u32) -> bool {
        self.remote.is_some()
            && self.age(now) < 2000
            && self
                .last_round_trip
                .is_some_and(|t| now.wrapping_sub(t) < 2000)
    }
    pub fn status(&self, now: u32) -> &'static str {
        if self.connected(now) {
            "CONNECTED"
        } else if self.remote.is_some() {
            "DISCONNECTED"
        } else {
            "WAITING FOR PEER"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packet() -> Packet {
        Packet {
            role: Role::Knob,
            flags: 5,
            session: 0x12345678,
            sequence: 7,
            echo_session: 99,
            echo_sequence: 6,
            steps: -2,
            angle: 1234,
        }
    }
    #[test]
    fn exact_wire_format_and_rejection() {
        let b = packet().encode();
        assert_eq!(
            b,
            [
                0x31, 0x4c, 0x4b, 0x54, 1, 1, 5, 0, 0x78, 0x56, 0x34, 0x12, 7, 0, 0, 0, 99, 0, 0,
                0, 6, 0, 0, 0, 0xfe, 0xff, 0xff, 0xff, 0xd2, 4, 0, 0
            ]
        );
        assert_eq!(Packet::decode(&b, Role::Knob), Some(packet()));
        assert_eq!(Packet::decode(&b, Role::Display), None);
        assert_eq!(Packet::decode(&b[..31], Role::Knob), None);
        for i in [0, 4, 5, 8, 12] {
            let mut bad = b;
            if i == 8 || i == 12 {
                bad[i..i + 4].fill(0);
            } else {
                bad[i] ^= 0xff;
            }
            assert_eq!(Packet::decode(&bad, Role::Knob), None);
        }
    }
    #[test]
    fn round_trip_reboot_and_one_way_loss() {
        let mut k = LinkState::new(Role::Knob, 1);
        let mut d = LinkState::new(Role::Display, 2);
        let p = k.outgoing(250, 0, 0, 0).unwrap();
        d.receive(p, 250);
        assert!(!d.connected(250));
        k.receive(d.outgoing(250, 0, 0, 0).unwrap(), 250);
        assert!(k.connected(250));
        d.receive(k.outgoing(500, 0, 0, 0).unwrap(), 500);
        assert!(d.connected(500));
        assert!(!d.connected(2500));
        let mut rebooted = LinkState::new(Role::Knob, 3);
        d.receive(rebooted.outgoing(2750, 0, 0, 0).unwrap(), 2750);
        assert!(!d.connected(2750));
        // Increasing incoming traffic with the same old echo cannot keep a
        // one-way-broken connection marked as healthy.
        let mut stale = k.outgoing(3000, 0, 0, 0).unwrap();
        stale.echo_session = 2;
        stale.echo_sequence = 1;
        d.receive(stale, 3000);
        stale.sequence += 1;
        d.receive(stale, 5001);
        assert!(!d.connected(5001));
    }
    #[test]
    fn sequence_and_clock_wrap() {
        let mut l = LinkState::new(Role::Knob, 1);
        l.sequence = u32::MAX;
        l.last_send = u32::MAX - 100;
        assert_eq!(l.outgoing(149, 0, 0, 0).unwrap().sequence, 1);
        assert!(newer(1, u32::MAX));
        assert!(!newer(u32::MAX, 1));
    }

    #[test]
    fn knob_changes_are_prompt_bounded_and_repeated_by_heartbeats() {
        let mut knob = LinkState::new(Role::Knob, 1);
        assert!(knob.outgoing(19, 0, 1, ENCODER_VALID).is_none());
        assert_eq!(knob.outgoing(20, 0, 1, ENCODER_VALID).unwrap().steps, 1);
        assert!(knob.outgoing(21, 0, 2, ENCODER_VALID).is_none());
        assert_eq!(knob.outgoing(40, 0, 3, ENCODER_VALID).unwrap().steps, 3);
        assert!(knob.outgoing(289, 0, 3, ENCODER_VALID).is_none());
        assert_eq!(knob.outgoing(290, 0, 3, ENCODER_VALID).unwrap().steps, 3);
        // Display heartbeats retain their 250 ms cadence.
        let mut display = LinkState::new(Role::Display, 2);
        assert!(display.outgoing(20, 0, 1, 1).is_none());
        assert!(display.outgoing(250, 0, 1, 1).is_some());
    }
}
