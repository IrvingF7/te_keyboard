//! `cargo run -p keyboard-display-ui --example preview -- /tmp/volume-preview`
//! Writes native-resolution PPM files using the actual firmware renderer.
use embedded_graphics::{pixelcolor::Rgb565, prelude::*};
use keyboard_display_ui::{draw, Activity, Connection, VolumeDisplay, HEIGHT, WIDTH};
use std::{convert::Infallible, fs, io::Write, path::Path};

struct Frame(Vec<Rgb565>);

impl OriginDimensions for Frame {
    fn size(&self) -> Size {
        Size::new(WIDTH, HEIGHT)
    }
}

impl DrawTarget for Frame {
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(
        &mut self,
        pixels: I,
    ) -> Result<(), Self::Error> {
        for Pixel(point, color) in pixels {
            // Fail loudly if a layout accidentally extends beyond the glass.
            assert!(point.x >= 0 && point.x < WIDTH as i32);
            assert!(point.y >= 0 && point.y < HEIGHT as i32);
            self.0[point.y as usize * WIDTH as usize + point.x as usize] = color;
        }
        Ok(())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "volume-preview".into());
    let directory = Path::new(&directory);
    fs::create_dir_all(directory)?;
    let samples = [
        (
            "volume-64",
            VolumeDisplay {
                level: Some(64),
                activity: Activity::Up,
                ..Default::default()
            },
        ),
        (
            "volume-00",
            VolumeDisplay {
                level: Some(0),
                ..Default::default()
            },
        ),
        (
            "volume-100",
            VolumeDisplay {
                level: Some(100),
                ..Default::default()
            },
        ),
        (
            "volume-125",
            VolumeDisplay {
                level: Some(125),
                ..Default::default()
            },
        ),
        (
            "muted",
            VolumeDisplay {
                level: Some(64),
                muted: true,
                ..Default::default()
            },
        ),
        ("relative", VolumeDisplay::default()),
        (
            "relative-up",
            VolumeDisplay {
                activity: Activity::Up,
                ..Default::default()
            },
        ),
        (
            "relative-down",
            VolumeDisplay {
                activity: Activity::Down,
                ..Default::default()
            },
        ),
        (
            "no-host",
            VolumeDisplay {
                connection: Connection::NoHost,
                ..Default::default()
            },
        ),
        (
            "no-knob",
            VolumeDisplay {
                connection: Connection::NoKnob,
                ..Default::default()
            },
        ),
        (
            "knob-fault",
            VolumeDisplay {
                connection: Connection::KnobFault,
                ..Default::default()
            },
        ),
    ];
    for (name, model) in samples {
        let mut frame = Frame(vec![Rgb565::BLACK; (WIDTH * HEIGHT) as usize]);
        draw(&mut frame, &model).unwrap();
        let path = directory.join(format!("{name}.ppm"));
        let mut file = fs::File::create(&path)?;
        write!(file, "P6\n{WIDTH} {HEIGHT}\n255\n")?;
        for pixel in frame.0 {
            file.write_all(&[
                (u16::from(pixel.r()) * 255 / 31) as u8,
                (u16::from(pixel.g()) * 255 / 63) as u8,
                (u16::from(pixel.b()) * 255 / 31) as u8,
            ])?;
        }
        println!("{}", path.display());
    }
    Ok(())
}
