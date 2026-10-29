//! S3 USB device: Consumer Control HID plus a nonblocking CDC console.
use crate::{check, millis, sys};
use keyboard_core::usb::{CONFIGURATION_DESCRIPTOR, HID_REPORT_DESCRIPTOR};
use std::{
    ffi::CString,
    fmt::Arguments,
    sync::{
        atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering},
        Mutex,
    },
};

static TAKEN: AtomicBool = AtomicBool::new(false);
// Bit 0 is online; the other bits change on every connection transition.
static CONNECTION: AtomicU32 = AtomicU32::new(0);
static CURRENT_REPORT: AtomicU16 = AtomicU16::new(0);

struct PendingReport {
    connection: u32,
    usage: u16,
    queued_at: u32,
    submitted: bool,
}
static PENDING_REPORT: Mutex<Option<PendingReport>> = Mutex::new(None);

pub struct UsbDevice {
    _private: (),
}

impl UsbDevice {
    pub fn new() -> anyhow::Result<Self> {
        anyhow::ensure!(
            !TAKEN.swap(true, Ordering::AcqRel),
            "USB already initialized"
        );
        let mut mac = [0u8; 6];
        check(unsafe { sys::esp_efuse_mac_get_default(mac.as_mut_ptr()) })?;
        let serial = CString::new(format!(
            "{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        ))?;
        // TinyUSB retains these pointers. This one-time allocation deliberately
        // lasts for the firmware's lifetime, including any USB reconnects.
        let strings = Box::leak(Box::new([
            b"\x09\x04\0".as_ptr().cast(),
            c"T-Keyboard".as_ptr(),
            c"Volume Display".as_ptr(),
            serial.into_raw().cast_const(),
            c"Console".as_ptr(),
            c"Volume controls".as_ptr(),
        ]));
        let config = sys::tinyusb_config_t {
            string_descriptor: strings.as_mut_ptr(),
            string_descriptor_count: strings.len() as i32,
            __bindgen_anon_2: sys::tinyusb_config_t__bindgen_ty_2 {
                __bindgen_anon_1: sys::tinyusb_config_t__bindgen_ty_2__bindgen_ty_1 {
                    configuration_descriptor: CONFIGURATION_DESCRIPTOR.as_ptr(),
                },
            },
            ..Default::default()
        };
        // The default device descriptor supplies Espressif's development VID,
        // the CDC+HID PID, and the composite-device class required by Windows.
        check(unsafe { sys::tinyusb_driver_install(&config) })?;
        if let Err(error) =
            check(unsafe { sys::tusb_cdc_acm_init(&sys::tinyusb_config_cdcacm_t::default()) })
        {
            connection_changed(false);
            unsafe {
                sys::tud_disconnect();
            }
            return Err(error.into());
        }
        Ok(Self { _private: () })
    }

    pub fn connection(&self) -> (u32, bool) {
        let state = CONNECTION.load(Ordering::Acquire);
        (state, state & 1 != 0)
    }

    pub fn read(&mut self) -> Option<u8> {
        let mut byte = 0u8;
        let count = unsafe { sys::tud_cdc_n_read(0, (&mut byte as *mut u8).cast(), 1) };
        (count == 1).then_some(byte)
    }

    /// Best-effort console output: a closed/slow terminal must not stall HID.
    pub fn log(&mut self, args: Arguments<'_>) {
        if !unsafe { sys::tud_cdc_n_connected(0) } {
            return;
        }
        let text = format!("{args}\r\n");
        unsafe {
            sys::tinyusb_cdcacm_write_queue(0, text.as_ptr().cast(), text.len());
            sys::tinyusb_cdcacm_write_flush(0, 0);
        }
    }

    /// Accept one report for delivery. A full mailbox applies backpressure to
    /// the caller until the preceding USB transfer completes.
    pub fn send_report(&mut self, connection: u32, usage: u16) -> bool {
        let mut pending = PENDING_REPORT.lock().unwrap();
        if connection & 1 == 0 || CONNECTION.load(Ordering::Acquire) != connection {
            return false;
        }
        if pending.is_some() {
            return false;
        }
        *pending = Some(PendingReport {
            connection,
            usage,
            queued_at: millis(),
            submitted: false,
        });
        true
    }

    /// Cancel a press that has not reached the endpoint yet when input is lost.
    /// A submitted press completes normally and is followed by the caller's release.
    pub fn cancel_pending(&mut self) {
        let mut pending = PENDING_REPORT.lock().unwrap();
        if pending.as_ref().is_some_and(|report| !report.submitted) {
            *pending = None;
        }
    }
}

impl Drop for UsbDevice {
    fn drop(&mut self) {
        // In particular, an LCD/SPI error must not leave a volume key held while
        // TinyUSB continues running. Physical USB detach clears the host state.
        connection_changed(false);
        unsafe {
            sys::tud_disconnect();
        }
        // Keep the static USB task/storage alive until reset. The component's
        // uninstall kills its task without joining it, which could orphan a
        // Rust mutex if the other CPU deletes a callback mid-lock.
    }
}

fn connection_changed(online: bool) {
    let mut pending = PENDING_REPORT.lock().unwrap();
    *pending = None;
    let previous = CONNECTION.load(Ordering::Relaxed);
    CURRENT_REPORT.store(0, Ordering::Release);
    CONNECTION.store(
        (previous.wrapping_add(2) & !1) | u32::from(online),
        Ordering::Release,
    );
}

#[no_mangle]
extern "C" fn tud_sof_cb(_frame_count: u32) {
    // Never wait for the UI while servicing USB events.
    let Ok(mut pending) = PENDING_REPORT.try_lock() else {
        return;
    };
    let Some(report) = pending.as_mut() else {
        return;
    };
    if report.submitted {
        return;
    }
    if CONNECTION.load(Ordering::Acquire) != report.connection
        || (report.usage != 0 && millis().wrapping_sub(report.queued_at) >= 500)
    {
        // Expired presses are discarded; releases must continue to be retried.
        drop(pending);
        connection_changed(CONNECTION.load(Ordering::Relaxed) & 1 != 0);
        return;
    }
    let bytes = report.usage.to_le_bytes();
    // TinyUSB copies the report into its own buffer before returning. Lifecycle
    // callbacks cannot run concurrently with this callback in the same task.
    if unsafe {
        sys::tud_hid_n_ready(0)
            && sys::tud_hid_n_report(0, 0, bytes.as_ptr().cast(), bytes.len() as u16)
    } {
        report.submitted = true;
        CURRENT_REPORT.store(report.usage, Ordering::Release);
    }
}

#[no_mangle]
extern "C" fn tud_hid_report_complete_cb(_instance: u8, _report: *const u8, _len: u16) {
    let mut pending = PENDING_REPORT.lock().unwrap();
    if pending.as_ref().is_some_and(|report| report.submitted) {
        *pending = None;
    }
}

#[no_mangle]
extern "C" fn tud_mount_cb() {
    connection_changed(true);
    // Bus reset clears SOF subscriptions, so reenable on EVERY enumeration.
    // This callback and SOF/report callbacks all run in TinyUSB's task.
    unsafe { sys::tud_sof_cb_enable(true) };
}

#[no_mangle]
extern "C" fn tud_umount_cb() {
    connection_changed(false);
}

#[no_mangle]
extern "C" fn tud_suspend_cb(_remote_wakeup_enabled: bool) {
    connection_changed(false);
}

#[no_mangle]
extern "C" fn tud_resume_cb() {
    connection_changed(true);
    unsafe { sys::tud_sof_cb_enable(true) };
}

#[no_mangle]
extern "C" fn tud_hid_descriptor_report_cb(_instance: u8) -> *const u8 {
    HID_REPORT_DESCRIPTOR.as_ptr()
}

#[no_mangle]
extern "C" fn tud_hid_get_report_cb(
    instance: u8,
    report_id: u8,
    report_type: sys::hid_report_type_t,
    buffer: *mut u8,
    requested: u16,
) -> u16 {
    if instance != 0
        || report_id != 0
        || report_type != sys::hid_report_type_t_HID_REPORT_TYPE_INPUT
        || buffer.is_null()
    {
        return 0;
    }
    let bytes = CURRENT_REPORT.load(Ordering::Acquire).to_le_bytes();
    let len = usize::from(requested).min(bytes.len());
    // TinyUSB supplies a writable buffer of at least `requested` bytes.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, len) };
    len as u16
}

#[no_mangle]
extern "C" fn tud_hid_set_report_cb(
    _instance: u8,
    _report_id: u8,
    _report_type: sys::hid_report_type_t,
    _buffer: *const u8,
    _size: u16,
) {
    // Our descriptor has only input reports.
}

#[no_mangle]
extern "C" fn tud_hid_report_failed_cb(
    _instance: u8,
    _report_type: sys::hid_report_type_t,
    _report: *const u8,
    _transferred: u16,
) {
    // Force the main loop to discard backlog and send a release again, even
    // when a release transfer itself failed.
    connection_changed(CONNECTION.load(Ordering::Relaxed) & 1 != 0);
}
