//! USB Serial/JTAG, the same USB port used for flashing. This is not USB HID.
use crate::{check, sys};

pub struct Serial;
impl Serial {
    pub fn new() -> anyhow::Result<Self> {
        let mut config = sys::usb_serial_jtag_driver_config_t {
            tx_buffer_size: 1024,
            rx_buffer_size: 256,
        };
        check(unsafe { sys::usb_serial_jtag_driver_install(&mut config) })?;
        // Route std's stdout through the buffered driver, rather than ROM USB.
        unsafe {
            sys::esp_vfs_usb_serial_jtag_use_driver();
        }
        Ok(Self)
    }
    pub fn read(&mut self) -> Option<u8> {
        let mut byte = 0u8;
        // A zero timeout makes this a poll: never wait for someone to type.
        let count = unsafe { sys::usb_serial_jtag_read_bytes((&mut byte as *mut u8).cast(), 1, 0) };
        (count == 1).then_some(byte)
    }
}
