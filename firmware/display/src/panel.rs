//! T-Display-Bar ST7789 initialization and its unusual 76x284 glass offsets.
//! Register values come from LILYGO's reference; rendering is entirely Rust.
use board_support::{output, set_level, spi::SpiWriter};
use embedded_graphics::{
    draw_target::DrawTarget,
    geometry::{OriginDimensions, Size},
    pixelcolor::{IntoStorage, Rgb565},
    Pixel,
};
use std::{convert::Infallible, thread::sleep, time::Duration};

pub const WIDTH: usize = 284;
pub const HEIGHT: usize = 76;
pub struct Frame {
    bytes: Vec<u8>,
}
impl Frame {
    fn new() -> Self {
        Self {
            bytes: vec![0; WIDTH * HEIGHT * 2],
        }
    }
}
impl OriginDimensions for Frame {
    fn size(&self) -> Size {
        Size::new(WIDTH as u32, HEIGHT as u32)
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
            if point.x >= 0 && point.y >= 0 && point.x < WIDTH as i32 && point.y < HEIGHT as i32 {
                let index = (point.y as usize * WIDTH + point.x as usize) * 2;
                self.bytes[index..index + 2].copy_from_slice(&color.into_storage().to_be_bytes());
            }
        }
        Ok(())
    }
    fn clear(&mut self, color: Rgb565) -> Result<(), Self::Error> {
        let pixel = color.into_storage().to_be_bytes();
        for bytes in self.bytes.chunks_exact_mut(2) {
            bytes.copy_from_slice(&pixel);
        }
        Ok(())
    }
}
pub struct Panel {
    spi: SpiWriter,
    pub frame: Frame,
}
impl Panel {
    pub fn new() -> anyhow::Result<Self> {
        output(15, true)?; // Backlight is active LOW; begin dark.
        output(5, false)?;
        output(40, true)?;
        sleep(Duration::from_millis(5));
        set_level(40, false)?;
        sleep(Duration::from_millis(20));
        set_level(40, true)?;
        sleep(Duration::from_millis(150));
        let mut panel = Self {
            // Match the vendor ST7789 driver: mode 3 means the clock idles HIGH
            // and data is sampled on its rising (second) edge. Mode 0 also
            // samples rising edges, but starts from LOW; that difference can
            // prevent this panel from recognizing commands. A successful SPI
            // write only confirms transmission, not that the LCD understood it.
            spi: SpiWriter::new(6, 7, 8, 40_000_000, 3)?,
            frame: Frame::new(),
        };
        panel.command(0x11, &[])?; // Sleep out.
        sleep(Duration::from_millis(120));
        // Panel-specific voltages/gamma are preserved from the working driver.
        for (command, bytes) in [
            (0xb2, &[0x0c, 0x0c, 0x00, 0x33, 0x33][..]),
            (0xb0, &[0x00, 0xe0]),
            (0x36, &[0x00]),
            (0x3a, &[0x05]),
            (0xb7, &[0x45]),
            (0xbb, &[0x1d]),
            (0xc0, &[0x2c]),
            (0xc2, &[0x01]),
            (0xc3, &[0x19]),
            (0xc4, &[0x20]),
            (0xc6, &[0x0f]),
            (0xd0, &[0xa4, 0xa1]),
            (0xd6, &[0xa1]),
            (
                0xe0,
                &[
                    0xd0, 0x10, 0x21, 0x14, 0x15, 0x2d, 0x41, 0x44, 0x4f, 0x28, 0x0e, 0x0c, 0x1d,
                    0x1f,
                ],
            ),
            (
                0xe1,
                &[
                    0xd0, 0x0f, 0x1b, 0x0d, 0x0d, 0x26, 0x42, 0x54, 0x50, 0x3e, 0x1a, 0x18, 0x22,
                    0x25,
                ],
            ),
            (0x36, &[0x60]), // MX|MV: landscape, RGB order.
            (0x29, &[]),
        ] {
            panel.command(command, bytes)?;
        }
        sleep(Duration::from_millis(120));
        Ok(panel)
    }
    pub fn command(&mut self, command: u8, bytes: &[u8]) -> anyhow::Result<()> {
        set_level(5, false)?;
        self.spi.write(&[command])?;
        if !bytes.is_empty() {
            set_level(5, true)?;
            self.spi.write(bytes)?;
        }
        Ok(())
    }
    pub fn backlight(&self, enabled: bool) -> anyhow::Result<()> {
        set_level(15, !enabled)?;
        Ok(())
    }
    pub fn flush(&mut self) -> anyhow::Result<()> {
        // The glass is a window into a larger controller RAM. Landscape origin
        // is (18,82), not (0,0); omitting this gives a shifted/cropped image.
        self.command(0x2a, &[0, 18, 1, 45])?; // x = 18..301, inclusive.
        self.command(0x2b, &[0, 82, 0, 157])?; // y = 82..157.
        self.command(0x2c, &[])?;
        set_level(5, true)?;
        self.spi.write(&self.frame.bytes)
    }
}
