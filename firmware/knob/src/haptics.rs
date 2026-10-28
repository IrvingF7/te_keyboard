//! The motor task owns its encoder and driver. The UI gets snapshots and can
//! request on/off, but cannot race the task by touching motor hardware directly.
use crate::{encoder::Encoder, motor::Motor};
use board_support::{micros, millis, sys};
use keyboard_core::motor::{electrical_angle, Detents, SettlingWindow};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Aligning,
    On,
    Off,
    Fault,
}
impl State {
    pub fn name(self) -> &'static str {
        match self {
            Self::Aligning => "ALIGNING",
            Self::On => "ON",
            Self::Off => "OFF",
            Self::Fault => "FAULT",
        }
    }
}
#[derive(Clone, Copy)]
pub struct Snapshot {
    pub state: State,
    pub fault: &'static str,
    pub angle: u16,
    pub steps: i32,
    pub good: u32,
    pub bad: u32,
    pub last_good: u32,
    pub max_gap: u32,
    pub cutoff_age: u32,
    pub voltage: f32,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            state: State::Aligning,
            fault: "none",
            angle: 0,
            steps: 0,
            good: 0,
            bad: 0,
            last_good: 0,
            max_gap: 0,
            cutoff_age: 0,
            voltage: 0.0,
        }
    }
}
pub struct Haptics {
    enabled: Arc<AtomicBool>,
    snapshot: Arc<Mutex<Snapshot>>,
}
impl Haptics {
    pub fn new() -> anyhow::Result<Self> {
        let enabled = Arc::new(AtomicBool::new(true));
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        let (command, telemetry) = (enabled.clone(), snapshot.clone());
        thread::Builder::new()
            .name("haptics".into())
            .stack_size(8192)
            .spawn(move || {
                // C6 has one main core. Priority 3 preempts the UI, but the task
                // yields each millisecond so Wi-Fi and idle tasks also get CPU.
                unsafe {
                    sys::vTaskPrioritySet(std::ptr::null_mut(), 3);
                }
                run(command, telemetry);
            })?;
        Ok(Self { enabled, snapshot })
    }
    pub fn enable(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }
    pub fn snapshot(&self) -> Snapshot {
        *self.snapshot.lock().unwrap()
    }
}

struct Control {
    encoder: Encoder,
    motor: Motor,
    detents: Detents,
    settling: SettlingWindow,
    calibrated: bool,
    aligning: bool,
    alignment_started: u32,
    zero: f32,
    data: Snapshot,
}
impl Control {
    fn new() -> anyhow::Result<Self> {
        Ok(Self {
            encoder: Encoder::new()?,
            motor: Motor::new()?,
            detents: Detents::new(0),
            settling: SettlingWindow::new(0, 0),
            calibrated: false,
            aligning: false,
            alignment_started: 0,
            zero: 0.0,
            data: Snapshot::default(),
        })
    }
    fn fail(&mut self, reason: &'static str) {
        self.motor.coast();
        if self.data.state == State::Fault {
            return;
        }
        self.data.state = State::Fault;
        self.data.fault = reason;
        self.data.voltage = 0.0;
        self.data.cutoff_age = self.motor.cutoff_age();
    }
    fn sample(&mut self, enabled: bool, now: u32, gap: u32) {
        let Some(angle) = self.encoder.read() else {
            self.data.bad = self.data.bad.wrapping_add(1);
            self.fail("encoder-read");
            return;
        };
        self.data.angle = angle;
        self.data.good = self.data.good.wrapping_add(1);
        self.data.last_good = now;
        if self.calibrated {
            self.detents.update(angle, gap as f32 * 0.000_001);
            self.data.steps = self.detents.steps;
        }
        if self.data.state == State::Fault {
            return;
        }
        if self.motor.timed_out() {
            self.fail("control-gap");
            return;
        }
        if self.motor.fault() {
            self.fail("driver-DIAG");
            return;
        }
        if matches!(self.data.state, State::On | State::Aligning) && gap > 20_000 {
            self.fail("control-gap");
            return;
        }
        if !enabled {
            self.motor.coast();
            self.data.voltage = 0.0;
            self.data.state = State::Off;
            self.aligning = false;
            return;
        }
        if !self.calibrated {
            if !self.aligning {
                self.aligning = true;
                self.alignment_started = now;
                self.settling = SettlingWindow::new(angle, now);
            }
            self.data.state = State::Aligning;
            let settled = self.settling.update(angle, now);
            let elapsed = now.wrapping_sub(self.alignment_started);
            // Ramp a fixed field to 1 V over 250 ms. Keep hands off for the
            // first second; this establishes the encoder/electrical zero.
            self.data.voltage = (elapsed as f32 / 250.0).min(1.0);
            if self
                .motor
                .torque(self.data.voltage, 1.5 * std::f32::consts::PI)
                .is_err()
            {
                self.fail("motor-write");
                return;
            }
            #[cfg(feature = "watchdog-self-test")]
            if elapsed >= 300 {
                let start = micros();
                // Interrupts stay enabled: GPTimer must stop the motor while
                // THIS task is intentionally unable to check its own deadline.
                while micros().wrapping_sub(start) < 50_000 {
                    std::hint::spin_loop();
                }
                self.fail("watchdog-self-test");
                return;
            }
            if elapsed >= 1000 && settled {
                self.zero = electrical_angle(angle);
                self.calibrated = true;
                self.detents = Detents::new(angle);
                self.motor.coast();
                self.data.voltage = 0.0;
                self.data.state = State::On;
            } else if elapsed >= 2000 {
                self.fail("alignment-moving");
            }
            return;
        }
        self.data.state = State::On;
        self.data.voltage = self.detents.voltage();
        if self.data.voltage == 0.0 {
            self.motor.coast();
        } else if self
            .motor
            .torque(self.data.voltage, electrical_angle(angle) - self.zero)
            .is_err()
        {
            self.fail("motor-write");
        }
    }
}
fn run(enabled: Arc<AtomicBool>, telemetry: Arc<Mutex<Snapshot>>) {
    let mut control = match Control::new() {
        Ok(control) => control,
        Err(error) => {
            eprintln!("HAPTICS initialization failed: {error:#}");
            let mut data = telemetry.lock().unwrap();
            data.state = State::Fault;
            data.fault = "initialization";
            return;
        }
    };
    let mut wake = unsafe { sys::xTaskGetTickCount() };
    let mut previous = micros();
    loop {
        let time = micros();
        let gap = time.wrapping_sub(previous);
        previous = time;
        control.data.max_gap = control.data.max_gap.max(gap);
        control.sample(enabled.load(Ordering::Relaxed), millis(), gap);
        // try_lock makes telemetry expendable: the motor never waits for USB/UI.
        if let Ok(mut destination) = telemetry.try_lock() {
            *destination = control.data;
        }
        unsafe {
            sys::xTaskDelayUntil(&mut wake, 1);
        }
    }
}
