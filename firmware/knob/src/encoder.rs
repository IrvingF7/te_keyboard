//! MT6701: 24-bit SPI frame containing angle, status, and CRC.
use board_support::{check, sys};
use keyboard_core::motor::decode_encoder;
use std::ptr;

pub struct Encoder {
    device: sys::spi_device_handle_t,
    // A stable allocation is essential: DMA/interrupt completion may outlive
    // a timeout. After timeout we never reuse or free the in-flight storage.
    transaction: &'static mut sys::spi_transaction_t,
    failed: bool,
}
impl Encoder {
    pub fn new() -> anyhow::Result<Self> {
        let mut bus = sys::spi_bus_config_t::default();
        bus.__bindgen_anon_1.mosi_io_num = -1;
        bus.__bindgen_anon_2.miso_io_num = 15;
        bus.sclk_io_num = 14;
        bus.__bindgen_anon_3.quadwp_io_num = -1;
        bus.__bindgen_anon_4.quadhd_io_num = -1;
        bus.data4_io_num = -1;
        bus.data5_io_num = -1;
        bus.data6_io_num = -1;
        bus.data7_io_num = -1;
        check(unsafe {
            sys::spi_bus_initialize(
                sys::spi_host_device_t_SPI2_HOST,
                &bus,
                sys::spi_common_dma_t_SPI_DMA_CH_AUTO,
            )
        })?;
        let config = sys::spi_device_interface_config_t {
            mode: 1,
            cs_ena_pretrans: 4,
            clock_speed_hz: 4_000_000,
            spics_io_num: 7,
            queue_size: 1,
            ..Default::default()
        };
        let mut device = ptr::null_mut();
        check(unsafe {
            sys::spi_bus_add_device(sys::spi_host_device_t_SPI2_HOST, &config, &mut device)
        })?;
        Ok(Self {
            device,
            transaction: Box::leak(Box::default()),
            failed: false,
        })
    }
    pub fn read(&mut self) -> Option<u16> {
        if self.failed {
            return None;
        }
        *self.transaction = sys::spi_transaction_t {
            flags: sys::SPI_TRANS_USE_RXDATA,
            length: 24,
            rxlength: 24,
            ..Default::default()
        };
        let mut completed = ptr::null_mut();
        // CONFIG_FREERTOS_HZ=1000: two ticks means a two-millisecond bound.
        if unsafe { sys::spi_device_queue_trans(self.device, self.transaction, 2) } != 0
            || unsafe { sys::spi_device_get_trans_result(self.device, &mut completed, 2) } != 0
        {
            self.failed = true;
            return None;
        }
        if !std::ptr::eq(completed, self.transaction) {
            self.failed = true;
            return None;
        }
        // Completion returned ownership of the buffer to us before this read.
        let bytes = unsafe { self.transaction.__bindgen_anon_2.rx_data };
        decode_encoder(u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]))
    }
}
