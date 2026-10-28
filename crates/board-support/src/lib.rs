//! ESP-IDF adapters. Keep hardware access here, and policy in `keyboard-core`.
pub mod radio;
pub mod serial;
pub mod spi;

pub use esp_idf_svc::sys;

pub fn millis() -> u32 {
    (unsafe { sys::esp_timer_get_time() } / 1000) as u32
}
pub fn micros() -> u32 {
    unsafe { sys::esp_timer_get_time() as u32 }
}
pub fn check(code: sys::esp_err_t) -> Result<(), sys::EspError> {
    sys::EspError::convert(code)
}
pub fn output(pin: i32, high: bool) -> Result<(), sys::EspError> {
    // Set the output latch BEFORE enabling the pin, avoiding an unwanted pulse.
    check(unsafe { sys::gpio_set_level(pin, u32::from(high)) })?;
    configure_gpio(pin, sys::gpio_mode_t_GPIO_MODE_OUTPUT, false)
}
pub fn input_pullup(pin: i32) -> Result<(), sys::EspError> {
    configure_gpio(pin, sys::gpio_mode_t_GPIO_MODE_INPUT, true)
}
fn configure_gpio(pin: i32, mode: sys::gpio_mode_t, pullup: bool) -> Result<(), sys::EspError> {
    let Some(pin_bit_mask) = 1u64.checked_shl(pin as u32) else {
        return check(sys::ESP_ERR_INVALID_ARG);
    };
    // A physical pin can serve GPIO, JTAG, RTC, or another peripheral. Setting
    // direction alone does not select GPIO in the IO multiplexer. gpio_config
    // also selects that function and releases any RTC routing. This matters for
    // the bar's LCD reset on GPIO40, which doubles as the JTAG MTDO signal.
    // C6 has an extra hysteresis field; S3 does not. Default fills any
    // chip-specific fields while keeping this adapter shared by both targets.
    #[allow(clippy::needless_update)]
    let config = sys::gpio_config_t {
        pin_bit_mask,
        mode,
        pull_up_en: if pullup {
            sys::gpio_pullup_t_GPIO_PULLUP_ENABLE
        } else {
            sys::gpio_pullup_t_GPIO_PULLUP_DISABLE
        },
        pull_down_en: sys::gpio_pulldown_t_GPIO_PULLDOWN_DISABLE,
        intr_type: sys::gpio_int_type_t_GPIO_INTR_DISABLE,
        ..Default::default()
    };
    check(unsafe { sys::gpio_config(&config) })
}
pub fn set_level(pin: i32, high: bool) -> Result<(), sys::EspError> {
    check(unsafe { sys::gpio_set_level(pin, u32::from(high)) })
}
pub fn is_low(pin: i32) -> bool {
    unsafe { sys::gpio_get_level(pin) == 0 }
}
