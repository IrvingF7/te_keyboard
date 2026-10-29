mod panel;
use board_support::{input_pullup, is_low, millis, radio::Radio, usb::UsbDevice};
use embedded_graphics::{
    mono_font::{ascii::FONT_8X13, MonoTextStyle},
    pixelcolor::Rgb565,
    prelude::*,
    text::{Baseline, Text},
};
use esp_idf_svc::hal::peripherals::Peripherals;
use keyboard_core::{
    controls::{Action, Button, PanelPower},
    protocol::*,
    volume::VolumeKeys,
};
use panel::Panel;
use std::{thread::sleep, time::Duration};

fn print_power(usb: &mut UsbDevice, power: &PanelPower) {
    usb.log(format_args!(
        "DISPLAY panel={} requested={} backlight={} saved={}",
        power.status(),
        on_off(power.requested()),
        on_off(power.backlight_on()),
        on_off(power.backlight_requested())
    ));
}
fn on_off(on: bool) -> &'static str {
    if on {
        "ON"
    } else {
        "OFF"
    }
}
fn motor(flags: u8) -> &'static str {
    if flags & HAPTICS_FAULT != 0 {
        "FAULT"
    } else if flags & HAPTICS_ACTIVE != 0 {
        "ON"
    } else {
        "OFF/ALIGN"
    }
}
fn draw(panel: &mut Panel, radio: &Radio, power: &PanelPower, now: u32) -> anyhow::Result<()> {
    panel.frame.clear(Rgb565::BLACK)?;
    let remote = radio.link.remote;
    let lines = [
        (
            format!("ESP-NOW TX:{} RX:{}", radio.sent, radio.link.received),
            Rgb565::CYAN,
        ),
        (
            radio.link.status(now).to_owned(),
            if radio.link.connected(now) {
                Rgb565::GREEN
            } else {
                Rgb565::YELLOW
            },
        ),
        (
            remote.map_or_else(
                || "Power on the knob to connect".to_owned(),
                |p| {
                    if p.flags & ENCODER_VALID != 0 {
                        format!(
                            "Angle:{:.1} Steps:{}",
                            p.angle as f32 * 360.0 / 16384.0,
                            p.steps
                        )
                    } else {
                        "Encoder: waiting for valid data".to_owned()
                    }
                },
            ),
            Rgb565::WHITE,
        ),
        (
            remote.map_or_else(
                || "BOOT:LCD BTN38:backlight".to_owned(),
                |p| {
                    format!(
                        "Motor:{} BL:{} RX:{}ms",
                        motor(p.flags),
                        on_off(power.backlight_on()),
                        radio.link.age(now)
                    )
                },
            ),
            Rgb565::WHITE,
        ),
    ];
    for (row, (text, color)) in lines.iter().enumerate() {
        Text::with_baseline(
            text,
            Point::new(4, (row * 19) as i32),
            MonoTextStyle::new(&FONT_8X13, *color),
            Baseline::Top,
        )
        .draw(&mut panel.frame)?;
    }
    panel.flush()
}
fn run() -> anyhow::Result<()> {
    let peripherals = Peripherals::take()?;
    let mut usb = UsbDevice::new()?;
    let mut volume = VolumeKeys::default();
    let mut usb_connection = 0;
    let mut last_packet = None;
    let mut input_active = false;
    input_pullup(0)?;
    input_pullup(38)?;
    let mut panel_button = Button::new(is_low(0), millis());
    let mut backlight_button = Button::new(is_low(38), millis());
    let mut power = PanelPower::default();
    let mut panel = Panel::new()?;
    let mut radio = Radio::new(peripherals.modem, Role::Display)?;
    usb.log(format_args!(
        "T-Display-Bar: USB volume keys; BOOT/'d' LCD; GPIO38/'b' backlight; '?' state"
    ));
    draw(&mut panel, &radio, &power, millis())?;
    panel.backlight(true)?;
    print_power(&mut usb, &power);
    let (mut last_draw, mut last_print) = (millis(), millis());
    loop {
        let now = millis();
        let mut changed = false;
        if panel_button.update(is_low(0), now) {
            power.toggle_panel();
            changed = true;
        }
        if backlight_button.update(is_low(38), now) {
            power.toggle_backlight();
            changed = true;
        }
        if let Some(command) = usb.read() {
            match command {
                b'd' => {
                    power.toggle_panel();
                    changed = true;
                }
                b'b' => {
                    power.toggle_backlight();
                    changed = true;
                }
                b'?' => print_power(&mut usb, &power),
                _ => (),
            }
        }
        radio.tick(now, 0, 0, 0);
        let (connection, online) = usb.connection();
        let remote = radio.link.remote;
        let packet_id = remote.map(|p| (p.session, p.sequence));
        let valid = radio.link.connected(now)
            && radio.link.age(now) < 500
            && remote.is_some_and(|p| p.flags & ENCODER_VALID != 0 && p.flags & HAPTICS_FAULT == 0);
        if connection != usb_connection || (input_active && (!online || !valid)) {
            volume.reset();
            usb.cancel_pending();
        }
        if connection != usb_connection || !online || !valid {
            // Wait for a NEW packet before establishing the next baseline.
            // Otherwise an old cached reading can replay movement made offline.
            last_packet = packet_id;
        } else if packet_id != last_packet {
            if let Some(packet) = remote {
                if last_packet.is_some_and(|(session, _)| session != packet.session) {
                    usb.cancel_pending();
                }
                volume.observe(packet.session, packet.steps, now);
            }
            last_packet = packet_id;
        }
        input_active = online && valid;
        usb_connection = connection;
        // A release is still sent when the radio disappears. Send before LCD
        // drawing; USB endpoint readiness serializes press and release reports.
        if let Some(report) = volume.next_report(now) {
            if usb.send_report(connection, report) {
                volume.report_sent(report);
            }
        }
        let action = power.tick(now);
        match action {
            Action::Sleep => {
                panel.backlight(false)?;
                panel.command(0x28, &[])?;
                panel.command(0x10, &[])?;
            }
            Action::Wake => panel.command(0x11, &[])?,
            Action::Redraw => {
                panel.command(0x29, &[])?;
                draw(&mut panel, &radio, &power, now)?;
            }
            Action::None => (),
        }
        panel.backlight(power.backlight_on())?;
        if changed || action != Action::None {
            print_power(&mut usb, &power);
        }
        if power.awake() && now.wrapping_sub(last_draw) >= 250 {
            last_draw = now;
            draw(&mut panel, &radio, &power, now)?;
        }
        if now.wrapping_sub(last_print) >= 1000 {
            last_print = now;
            usb.log(format_args!("USB {} LINK {} tx={} rx={} errors={} age_ms={} angle={} steps={} motor={} panel={} backlight={}",
                if online { "READY" } else { "OFFLINE" },
                radio.link.status(now), radio.sent, radio.link.received, radio.errors, radio.link.age(now),
                radio.link.remote.map_or(0, |p| p.angle), radio.link.remote.map_or(0, |p| p.steps),
                radio.link.remote.map_or("UNKNOWN", |p| motor(p.flags)), power.status(), on_off(power.backlight_on())));
        }
        sleep(Duration::from_millis(10));
    }
}
fn main() {
    esp_idf_svc::sys::link_patches();
    if let Err(error) = run() {
        eprintln!("Display startup/runtime error: {error:#}");
    }
}
