//! A small volume instrument for the 284 × 76 display.
//!
//! The renderer knows only pixels and display state, so the exact same drawing
//! code runs on the LCD and in the desktop preview. No allocation is needed.
#![no_std]

use embedded_graphics::{
    mono_font::{ascii::FONT_5X7, MonoTextStyle},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Line, PrimitiveStyle, Rectangle},
    text::{Baseline, Text},
};

pub const WIDTH: u32 = 284;
pub const HEIGHT: u32 = 76;

const BACKGROUND: Rgb565 = Rgb565::new(2, 4, 2);
const PAPER: Rgb565 = Rgb565::new(29, 58, 26);
const ORANGE: Rgb565 = Rgb565::new(31, 21, 3);
const QUIET: Rgb565 = Rgb565::new(15, 31, 14);
const TRACK: Rgb565 = Rgb565::new(5, 10, 5);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Connection {
    #[default]
    Ready,
    NoHost,
    NoKnob,
    KnobFault,
}

/// A brief visual acknowledgement of a queued volume command.
/// The caller controls its lifetime; the renderer does not own a clock.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Activity {
    #[default]
    Idle,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VolumeDisplay {
    /// An authoritative host level. `None` means the level is unknown, never 0.
    /// Boost above 100 is shown numerically; values above 999 are clamped.
    pub level: Option<u16>,
    pub muted: bool,
    pub connection: Connection,
    pub activity: Activity,
}

/// Draw a complete screen, replacing the previous frame.
pub fn draw<D>(target: &mut D, model: &VolumeDisplay) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    target.clear(BACKGROUND)?;
    let ready = model.connection == Connection::Ready;
    let host_connected = model.connection != Connection::NoHost;
    let level = model.level.map(|value| value.min(999));
    let number_color = if model.muted || !ready { QUIET } else { PAPER };

    rect(target, 9, 8, 5, 7, ORANGE)?;
    label(target, "VOLUME", 20, 8, PAPER)?;
    line(target, (110, 8), (110, 67), TRACK)?;
    label(target, "OUTPUT", 123, 8, QUIET)?;
    label(
        target,
        "USB",
        248,
        8,
        if host_connected { PAPER } else { QUIET },
    )?;
    rect(
        target,
        270,
        9,
        4,
        4,
        if host_connected { ORANGE } else { TRACK },
    )?;

    match level {
        Some(value) => {
            // Fixed-size stack storage avoids formatting strings on the MCU.
            let digits = [value / 100, (value / 10) % 10, value % 10];
            let first = if value >= 100 { 0 } else { 1 };
            let count = 3 - first;
            let left = 94 - (count as i32 * 30 - 5);
            for (index, digit) in digits[first..].iter().enumerate() {
                draw_digit(
                    target,
                    *digit as u8,
                    left + index as i32 * 30,
                    24,
                    number_color,
                )?;
            }
            label(target, "%", 98, 59, QUIET)?;
        }
        None => {
            rect(target, 39, 41, 22, 6, number_color)?;
            rect(target, 69, 41, 22, 6, number_color)?;
        }
    }

    // Twenty-four separate bars make the coarse level readable at a glance.
    // Unknown levels use a thin neutral ruler, never an invented filled meter.
    let filled = level.map(|value| (u32::from(value.min(100)) * 24).div_ceil(100));
    for index in 0..24 {
        let x = 123 + index * 6;
        let active = ready && !model.muted && filled.is_some_and(|n| index < n as i32);
        let color = if active {
            if filled == Some((index + 1) as u32) {
                ORANGE
            } else {
                PAPER
            }
        } else {
            TRACK
        };
        let height = if level.is_some() { 17 } else { 7 };
        rect(target, x, 42 - height, 4, height as u32, color)?;
        if index % 6 == 0 || index == 23 {
            rect(target, x + 1, 45, 1, 2, QUIET)?;
        }
    }
    if level.is_some() {
        label(target, "0", 123, 49, QUIET)?;
        label(target, "50", 190, 49, QUIET)?;
        label(target, "100", 256, 49, QUIET)?;
    } else {
        label(target, "-", 123, 49, QUIET)?;
        label(target, "+", 266, 49, QUIET)?;
    }

    let (status, status_color) = match model.connection {
        Connection::NoHost => ("CONNECT USB", QUIET),
        Connection::NoKnob => ("FINDING KNOB", QUIET),
        Connection::KnobFault => ("KNOB FAULT", ORANGE),
        Connection::Ready if model.muted => ("MUTED", ORANGE),
        Connection::Ready if level.is_none() => ("RELATIVE", QUIET),
        Connection::Ready if level.is_some_and(|value| value > 100) => ("BOOST", ORANGE),
        Connection::Ready => ("HOST LEVEL", QUIET),
    };
    label(target, status, 123, 63, status_color)?;
    if ready {
        draw_activity(target, model.activity)?;
    }
    Ok(())
}

fn label<D: DrawTarget<Color = Rgb565>>(
    target: &mut D,
    text: &str,
    x: i32,
    y: i32,
    color: Rgb565,
) -> Result<(), D::Error> {
    Text::with_baseline(
        text,
        Point::new(x, y),
        MonoTextStyle::new(&FONT_5X7, color),
        Baseline::Top,
    )
    .draw(target)
    .map(|_| ())
}

fn rect<D: DrawTarget<Color = Rgb565>>(
    target: &mut D,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    color: Rgb565,
) -> Result<(), D::Error> {
    Rectangle::new(Point::new(x, y), Size::new(width, height))
        .into_styled(PrimitiveStyle::with_fill(color))
        .draw(target)
}

fn line<D: DrawTarget<Color = Rgb565>>(
    target: &mut D,
    from: (i32, i32),
    to: (i32, i32),
    color: Rgb565,
) -> Result<(), D::Error> {
    Line::new(Point::new(from.0, from.1), Point::new(to.0, to.1))
        .into_styled(PrimitiveStyle::with_stroke(color, 1))
        .draw(target)
}

// A deliberately small geometric numeral alphabet: 35 bits per digit, expanded
// to 25 × 42 pixels. Whole horizontal runs become rectangles, reducing calls.
const DIGITS: [[u8; 7]; 10] = [
    [
        0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
    ],
    [
        0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
    ],
    [
        0b11110, 0b00001, 0b00001, 0b01110, 0b10000, 0b10000, 0b11111,
    ],
    [
        0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
    ],
    [
        0b10001, 0b10001, 0b10001, 0b11111, 0b00001, 0b00001, 0b00001,
    ],
    [
        0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
    ],
    [
        0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
    ],
    [
        0b11111, 0b00001, 0b00001, 0b00010, 0b00100, 0b00100, 0b00100,
    ],
    [
        0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
    ],
    [
        0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
    ],
];

fn draw_digit<D: DrawTarget<Color = Rgb565>>(
    target: &mut D,
    digit: u8,
    x: i32,
    y: i32,
    color: Rgb565,
) -> Result<(), D::Error> {
    for (row, bits) in DIGITS[digit as usize].iter().enumerate() {
        let mut column = 0;
        while column < 5 {
            if bits & (1 << (4 - column)) == 0 {
                column += 1;
                continue;
            }
            let start = column;
            while column < 5 && bits & (1 << (4 - column)) != 0 {
                column += 1;
            }
            rect(
                target,
                x + start * 5,
                y + row as i32 * 6,
                (column - start) as u32 * 5,
                6,
                color,
            )?;
        }
    }
    Ok(())
}

fn draw_activity<D: DrawTarget<Color = Rgb565>>(
    target: &mut D,
    activity: Activity,
) -> Result<(), D::Error> {
    match activity {
        Activity::Idle => rect(target, 267, 66, 3, 3, TRACK),
        Activity::Up | Activity::Down => {
            let (tip, base) = if activity == Activity::Up {
                (62, 69)
            } else {
                (69, 62)
            };
            line(target, (266, base), (266, tip), ORANGE)?;
            let shoulder = if activity == Activity::Up {
                tip + 3
            } else {
                tip - 3
            };
            line(target, (263, shoulder), (266, tip), ORANGE)?;
            line(target, (269, shoulder), (266, tip), ORANGE)
        }
    }
}
