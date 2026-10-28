mod encoder;
mod haptics;
mod motor;
use board_support::{input_pullup, is_low, millis, radio::Radio, serial::Serial};
use esp_idf_svc::hal::peripherals::Peripherals;
use haptics::{Haptics, State};
use keyboard_core::{controls::Button, protocol::*};
use std::{thread::sleep, time::Duration};

fn run() -> anyhow::Result<()> {
    motor::safe_pins()?;
    let peripherals = Peripherals::take()?;
    let mut serial = Serial::new()?;
    input_pullup(9)?;
    let mut button = Button::new(is_low(9), millis());
    // Finish potentially slow radio initialization before motor alignment.
    let mut radio = Radio::new(peripherals.modem, Role::Knob)?;
    println!("T-Knob Rust: 24 detents; BOOT/'m' haptics; keep hands off for ~1 second");
    let haptics = Haptics::new()?;
    let mut enabled = true;
    let (mut last_print, mut last_state, mut last_connected) = (0, State::Off, false);
    loop {
        let now = millis();
        let pressed = button.update(is_low(9), now);
        let command = serial.read();
        if pressed || command == Some(b'm') {
            enabled = !enabled;
            haptics.enable(enabled);
            println!(
                "Haptics requested {} (faults require reset)",
                if enabled { "ON" } else { "OFF" }
            );
        }
        let data = haptics.snapshot();
        // Sample time AFTER the snapshot: the higher-priority task might have
        // produced it after the loop's initial timestamp.
        let fresh = data.good != 0 && millis().wrapping_sub(data.last_good) < 500;
        let flags = if fresh { ENCODER_VALID } else { 0 }
            | if button.pressed() { BUTTON_PRESSED } else { 0 }
            | if data.state == State::On {
                HAPTICS_ACTIVE
            } else {
                0
            }
            | if data.state == State::Fault {
                HAPTICS_FAULT
            } else {
                0
            };
        radio.tick(now, data.angle, data.steps, flags);
        let connected = radio.link.connected(now);
        if now.wrapping_sub(last_print) >= 1000
            || data.state != last_state
            || connected != last_connected
        {
            println!("LINK {} tx={} rx={} errors={} | HAPTICS {} fault={} angle={:.2} steps={} ok={} errors={} Uq={:.3}V max_gap_us={} cutoff_us={}",
                radio.link.status(now), radio.sent, radio.link.received, radio.errors,
                data.state.name(), data.fault, data.angle as f32 * 360.0 / 16384.0,
                data.steps, data.good, data.bad, data.voltage, data.max_gap, data.cutoff_age);
            last_print = now;
            last_state = data.state;
            last_connected = connected;
        }
        sleep(Duration::from_millis(5));
    }
}
fn main() {
    esp_idf_svc::sys::link_patches();
    if let Err(error) = run() {
        eprintln!("Knob startup error: {error:#}");
    }
}
