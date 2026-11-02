mod panel;
use board_support::{input_pullup, is_low, millis, radio::Radio, usb::UsbDevice};
use esp_idf_svc::hal::peripherals::Peripherals;
use keyboard_core::{
    controls::{Action, Button, PanelPower},
    feedback::{ConsoleCommand, ConsoleInput, HostFeedback},
    protocol::*,
    volume::{VolumeKeys, VOLUME_DOWN, VOLUME_UP},
};
use keyboard_display_ui::{Activity, Connection, VolumeDisplay};
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
fn draw(panel: &mut Panel, display: &VolumeDisplay) -> anyhow::Result<()> {
    keyboard_display_ui::draw(&mut panel.frame, display)?;
    panel.flush()
}
fn run() -> anyhow::Result<()> {
    let peripherals = Peripherals::take()?;
    let mut usb = UsbDevice::new()?;
    let mut volume = VolumeKeys::default();
    let mut usb_connection = 0;
    let mut last_packet = None;
    let mut input_active = false;
    let mut console = ConsoleInput::default();
    let mut feedback = HostFeedback::default();
    let mut activity = None;
    input_pullup(0)?;
    input_pullup(38)?;
    let mut panel_button = Button::new(is_low(0), millis());
    let mut backlight_button = Button::new(is_low(38), millis());
    let mut power = PanelPower::default();
    let mut panel = Panel::new()?;
    let mut radio = Radio::new(peripherals.modem, Role::Display)?;
    usb.log(format_args!(
        "T-Display-Bar: volume UI; d/b/? + Enter; optional host feedback: cargo display volume"
    ));
    let mut last_frame = VolumeDisplay {
        level: None,
        muted: false,
        connection: Connection::NoHost,
        activity: Activity::Idle,
    };
    draw(&mut panel, &last_frame)?;
    panel.backlight(true)?;
    print_power(&mut usb, &power);
    let (mut last_draw, mut last_print) = (millis(), millis());
    loop {
        let now = millis();
        let (connection, online) = usb.connection();
        if connection != usb_connection || !online {
            feedback.set(None, now);
            console = ConsoleInput::default();
            activity = None;
        }
        let mut changed = false;
        if panel_button.update(is_low(0), now) {
            power.toggle_panel();
            changed = true;
        }
        if backlight_button.update(is_low(38), now) {
            power.toggle_backlight();
            changed = true;
        }
        // Drain a bounded burst: a whole snapshot fits in one tick, while a
        // chatty host cannot starve the radio, HID releases, or panel buttons.
        for _ in 0..64 {
            let Some(byte) = usb.read() else { break };
            let Some(command) = console.push(byte) else {
                continue;
            };
            match command {
                ConsoleCommand::TogglePanel => {
                    power.toggle_panel();
                    changed = true;
                }
                ConsoleCommand::ToggleBacklight => {
                    power.toggle_backlight();
                    changed = true;
                }
                ConsoleCommand::PowerStatus => print_power(&mut usb, &power),
                ConsoleCommand::Bootloader => usb.enter_flash_mode(),
                ConsoleCommand::Volume(value) => {
                    if online {
                        feedback.set(value, now);
                    }
                }
            }
        }
        radio.tick(now, 0, 0, 0);
        let remote = radio.link.remote;
        let packet_id = remote.map(|p| (p.session, p.sequence));
        let valid = radio.link.connected(now)
            && radio.link.age(now) < 500
            && remote.is_some_and(|p| p.flags & ENCODER_VALID != 0 && p.flags & HAPTICS_FAULT == 0);
        if connection != usb_connection || (input_active && (!online || !valid)) {
            volume.reset();
            usb.cancel_pending();
            activity = None;
        }
        if connection != usb_connection || !online || !valid {
            // Wait for a NEW packet before establishing the next baseline.
            // Otherwise an old cached reading can replay movement made offline.
            last_packet = packet_id;
        } else if packet_id != last_packet {
            if let Some(packet) = remote {
                if last_packet.is_some_and(|(session, _)| session != packet.session) {
                    usb.cancel_pending();
                    activity = None;
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
                match report {
                    VOLUME_UP => activity = Some((Activity::Up, now)),
                    VOLUME_DOWN => activity = Some((Activity::Down, now)),
                    _ => (),
                }
            }
        }
        let host_volume = feedback.current(now);
        let display = VolumeDisplay {
            level: host_volume.map(|value| value.percent),
            muted: host_volume.is_some_and(|value| value.muted),
            connection: if !online {
                Connection::NoHost
            } else if radio.link.connected(now)
                && radio.link.age(now) < 500
                && remote.is_some_and(|packet| packet.flags & HAPTICS_FAULT != 0)
            {
                Connection::KnobFault
            } else if !valid {
                Connection::NoKnob
            } else {
                Connection::Ready
            },
            // This pulse acknowledges a queued key, not an inferred volume.
            activity: activity
                .filter(|(_, started)| now.wrapping_sub(*started) < 180)
                .map_or(Activity::Idle, |(direction, _)| direction),
        };
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
                draw(&mut panel, &display)?;
                last_frame = display;
                last_draw = now;
            }
            Action::None => (),
        }
        panel.backlight(power.backlight_on())?;
        if changed || action != Action::None {
            print_power(&mut usb, &power);
        }
        // A full frame costs ~9 ms on the wire. Only transfer changed images,
        // capped at 25 Hz, keeping the input loop responsive when idle.
        if power.awake() && display != last_frame && now.wrapping_sub(last_draw) >= 40 {
            last_draw = now;
            draw(&mut panel, &display)?;
            last_frame = display;
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
