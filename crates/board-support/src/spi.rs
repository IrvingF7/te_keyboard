//! A small synchronous write adapter. The sensor uses its own bounded queued
//! transfer because a motor must stop promptly if its SPI transfer stalls.
use crate::{check, sys};
use std::ptr;

pub struct SpiWriter {
    device: sys::spi_device_handle_t,
}
impl SpiWriter {
    /// One SPI2 owner per firmware; these pins are supplied by the board module.
    pub fn new(mosi: i32, clock: i32, cs: i32, hz: i32, mode: u8) -> anyhow::Result<Self> {
        let mut bus = sys::spi_bus_config_t::default();
        bus.__bindgen_anon_1.mosi_io_num = mosi;
        bus.__bindgen_anon_2.miso_io_num = -1;
        bus.sclk_io_num = clock;
        bus.__bindgen_anon_3.quadwp_io_num = -1;
        bus.__bindgen_anon_4.quadhd_io_num = -1;
        bus.data4_io_num = -1;
        bus.data5_io_num = -1;
        bus.data6_io_num = -1;
        bus.data7_io_num = -1;
        bus.max_transfer_sz = 4096;
        check(unsafe {
            sys::spi_bus_initialize(
                sys::spi_host_device_t_SPI2_HOST,
                &bus,
                sys::spi_common_dma_t_SPI_DMA_CH_AUTO,
            )
        })?;
        let config = sys::spi_device_interface_config_t {
            clock_speed_hz: hz,
            mode,
            spics_io_num: cs,
            queue_size: 1,
            ..Default::default()
        };
        let mut device = ptr::null_mut();
        check(unsafe {
            sys::spi_bus_add_device(sys::spi_host_device_t_SPI2_HOST, &config, &mut device)
        })?;
        Ok(Self { device })
    }
    pub fn write(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        for chunk in bytes.chunks(4092) {
            let mut transaction = sys::spi_transaction_t {
                length: chunk.len() * 8,
                ..Default::default()
            };
            transaction.__bindgen_anon_1.tx_buffer = chunk.as_ptr().cast();
            // The call finishes before returning, so the borrowed slice and
            // stack transaction both outlive the peripheral's access to them.
            check(unsafe { sys::spi_device_transmit(self.device, &mut transaction) })?;
        }
        Ok(())
    }
}
