//! Ask the running display to return USB to the hardware Serial/JTAG controller.
//! espflash then uses that controller's normal reset-to-ROM handshake.
use serialport::{SerialPortInfo, SerialPortType};
use std::{env, io::Write, path::Path, time::Duration};

type Error = Box<dyn std::error::Error>;
const VID: u16 = 0x303a;
const RUNTIME_PID: u16 = 0x4005;
const DOWNLOAD_PID: u16 = 0x1001;
const POLL: Duration = Duration::from_millis(100);
const POLLS: usize = 100;
const REQUEST: &[u8] = b"\nBOOTLOADER\n";
const MANUAL: &str = "Hold display BOOT, tap RESET, release BOOT, then retry. Older firmware needs this manual step once to install automatic flashing support.";

trait Backend {
    fn ports(&mut self) -> Result<Vec<SerialPortInfo>, Error>;
    fn request(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error>;
    fn wait(&mut self);
}

struct System;

impl Backend for System {
    fn ports(&mut self) -> Result<Vec<SerialPortInfo>, Error> {
        Ok(serialport::available_ports()?)
    }

    fn request(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error> {
        // Opening a terminal and setting DTR are harmless; only the complete
        // command requests the handoff. The exclusive open respects monitors
        // and the optional volume helper already using this interface.
        let mut port = serialport::new(path, 115_200)
            .timeout(Duration::from_millis(500))
            .open()?;
        port.write_data_terminal_ready(true)?;
        port.write_all(bytes)?;
        // Allow USB to deliver the short request before closing. Do not use
        // flush(): some platforms can wait indefinitely for a detached port.
        std::thread::sleep(POLL);
        Ok(())
    }

    fn wait(&mut self) {
        std::thread::sleep(POLL);
    }
}

fn serial_key(serial: &str) -> Option<String> {
    if serial.contains(':') {
        let parts: Vec<_> = serial.split(':').collect();
        if parts.len() != 6 || parts.iter().any(|part| part.len() != 2) {
            return None;
        }
    }
    let compact = serial.replace(':', "");
    (compact.len() == 12 && compact.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| compact.to_ascii_uppercase())
}

fn is_device(port: &SerialPortInfo, serial: &str, pid: u16) -> bool {
    let SerialPortType::UsbPort(info) = &port.port_type else {
        return false;
    };
    info.vid == VID
        && info.pid == pid
        && serial_key(serial).is_some_and(|expected| {
            info.serial_number.as_deref().and_then(serial_key) == Some(expected)
        })
}

fn same_path(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let com = |path: &str| {
        let name = path.strip_prefix(r"\\.\").unwrap_or(path);
        name.get(..3)
            .filter(|prefix| prefix.eq_ignore_ascii_case("COM"))
            .and_then(|_| name.get(3..))
            .filter(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
            .map(|_| name.to_ascii_uppercase())
    };
    if com(left).is_some_and(|name| com(right) == Some(name)) {
        return true;
    }
    match (
        Path::new(left).canonicalize(),
        Path::new(right).canonicalize(),
    ) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn select(
    ports: &[SerialPortInfo],
    serial: &str,
    pid: u16,
    override_path: Option<&str>,
    setting: &str,
) -> Result<Option<String>, Error> {
    if let Some(path) = override_path {
        if let Some(port) = ports.iter().find(|port| same_path(path, &port.port_name)) {
            if !is_device(port, serial, pid) {
                return Err(format!(
                    "{setting}={path} is not the expected display USB interface (303a:{pid:04x}, serial {serial}). Check the override; no command was sent to this port."
                )
                .into());
            }
            return Ok(Some(path.to_owned()));
        }
        return Ok(None);
    }

    let mut candidates: Vec<_> = ports
        .iter()
        .filter(|port| is_device(port, serial, pid))
        .map(|port| port.port_name.clone())
        .collect();
    candidates.sort();
    candidates.dedup();
    // macOS enumerates callout and dial-in aliases for the same interface.
    let callouts = candidates.clone();
    candidates.retain(|path| {
        !path
            .strip_prefix("/dev/tty.")
            .is_some_and(|suffix| callouts.contains(&format!("/dev/cu.{suffix}")))
    });
    match candidates.as_slice() {
        [] => Ok(None),
        [path] => Ok(Some(path.clone())),
        _ => Err(format!(
            "Multiple matching display interfaces found: {}. Set {setting} explicitly.",
            candidates.join(", ")
        )
        .into()),
    }
}

fn prepare(
    serial: &str,
    download_override: Option<&str>,
    runtime_override: Option<&str>,
    backend: &mut impl Backend,
) -> Result<String, Error> {
    let ports = backend.ports()?;
    if let Some(path) = select(
        &ports,
        serial,
        DOWNLOAD_PID,
        download_override,
        "DISPLAY_PORT",
    )? {
        return Ok(path);
    }
    let runtime = select(
        &ports,
        serial,
        RUNTIME_PID,
        runtime_override,
        "DISPLAY_MONITOR_PORT",
    )?
    .ok_or_else(|| {
        format!(
            "Display download and runtime ports are unavailable for serial {serial}. {MANUAL} Use DISPLAY_PORT for its download port and DISPLAY_MONITOR_PORT for its runtime CDC port."
        )
    })?;

    eprintln!("Display: requesting USB download interface through {runtime}...");
    backend.request(&runtime, REQUEST).map_err(|error| {
        format!(
            "Cannot request display bootloader through {runtime}: {error}. Close cargo display monitor and cargo display volume, then retry. {MANUAL}"
        )
    })?;
    for _ in 0..POLLS {
        backend.wait();
        if let Some(path) = select(
            &backend.ports()?,
            serial,
            DOWNLOAD_PID,
            download_override,
            "DISPLAY_PORT",
        )? {
            return Ok(path);
        }
    }
    let override_hint = download_override
        .map(|path| format!(" Check DISPLAY_PORT={path}; it must identify the download interface."))
        .unwrap_or_default();
    Err(format!(
        "Display download interface did not appear within 10 seconds.{override_hint} {MANUAL}"
    )
    .into())
}

pub fn prepare_flash(serial: &str) -> Result<String, Error> {
    prepare(
        serial,
        env::var("DISPLAY_PORT").ok().as_deref(),
        // DISPLAY_PORT may refer to an absent downloader. Never send a runtime
        // command there, even though monitor retains its historical fallback.
        env::var("DISPLAY_MONITOR_PORT").ok().as_deref(),
        &mut System,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serialport::UsbPortInfo;
    use std::collections::VecDeque;

    const DISPLAY: &str = "80:B5:4E:C7:F3:DC";
    const KNOB: &str = "40:4C:CA:5C:39:30";

    fn port(path: &str, pid: u16, serial: &str) -> SerialPortInfo {
        SerialPortInfo {
            port_name: path.into(),
            port_type: SerialPortType::UsbPort(UsbPortInfo {
                vid: VID,
                pid,
                serial_number: Some(serial.into()),
                manufacturer: None,
                product: None,
            }),
        }
    }

    #[derive(Default)]
    struct Fake {
        snapshots: VecDeque<Vec<SerialPortInfo>>,
        requests: Vec<(String, Vec<u8>)>,
        waits: usize,
        busy: bool,
    }

    impl Backend for Fake {
        fn ports(&mut self) -> Result<Vec<SerialPortInfo>, Error> {
            if self.snapshots.len() > 1 {
                Ok(self.snapshots.pop_front().unwrap())
            } else {
                Ok(self.snapshots.front().cloned().unwrap_or_default())
            }
        }

        fn request(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error> {
            if self.busy {
                return Err("port is busy".into());
            }
            self.requests.push((path.into(), bytes.into()));
            Ok(())
        }

        fn wait(&mut self) {
            self.waits += 1;
        }
    }

    #[test]
    fn existing_download_port_needs_no_runtime_command() {
        let mut backend = Fake {
            snapshots: VecDeque::from([vec![
                port("COM2", DOWNLOAD_PID, KNOB),
                port("COM5", DOWNLOAD_PID, DISPLAY),
            ]]),
            ..Fake::default()
        };
        assert_eq!(prepare(DISPLAY, None, None, &mut backend).unwrap(), "COM5");
        assert!(backend.requests.is_empty());
        assert_eq!(backend.waits, 0);
    }

    #[test]
    fn handoff_waits_for_same_board_after_runtime_disappears() {
        let knob = port("COM2", DOWNLOAD_PID, KNOB);
        let mut backend = Fake {
            snapshots: VecDeque::from([
                vec![knob.clone(), port("COM4", RUNTIME_PID, "80b54ec7f3dc")],
                vec![knob.clone()],
                vec![knob, port("COM7", DOWNLOAD_PID, DISPLAY)],
            ]),
            ..Fake::default()
        };
        assert_eq!(
            prepare(DISPLAY, Some("com7"), None, &mut backend).unwrap(),
            "com7"
        );
        assert_eq!(backend.requests, [("COM4".into(), REQUEST.to_vec())]);
        assert_eq!(backend.waits, 2);
    }

    #[test]
    fn override_for_wrong_board_or_wrong_interface_sends_nothing() {
        for (download, runtime) in [(Some("COM2"), None), (None, Some("COM2"))] {
            let mut backend = Fake {
                snapshots: VecDeque::from([vec![
                    port("COM2", DOWNLOAD_PID, KNOB),
                    port("COM4", RUNTIME_PID, DISPLAY),
                ]]),
                ..Fake::default()
            };
            assert!(prepare(DISPLAY, download, runtime, &mut backend).is_err());
            assert!(backend.requests.is_empty());
        }
        let ports = [port("COM4", RUNTIME_PID, DISPLAY)];
        assert!(select(&ports, DISPLAY, DOWNLOAD_PID, Some("COM4"), "DISPLAY_PORT").is_err());
    }

    #[test]
    fn old_firmware_or_wrong_board_times_out_with_manual_recovery() {
        let mut backend = Fake {
            snapshots: VecDeque::from([
                vec![port("COM4", RUNTIME_PID, DISPLAY)],
                vec![port("COM2", DOWNLOAD_PID, KNOB)],
            ]),
            ..Fake::default()
        };
        let error = prepare(DISPLAY, None, None, &mut backend)
            .unwrap_err()
            .to_string();
        assert!(error.contains("within 10 seconds"));
        assert!(error.contains("Older firmware"));
        assert_eq!(backend.requests.len(), 1);
        assert_eq!(backend.waits, POLLS);
    }

    #[test]
    fn busy_runtime_and_missing_device_do_not_wait_or_target_another_board() {
        let mut backend = Fake {
            snapshots: VecDeque::from([vec![port("COM4", RUNTIME_PID, DISPLAY)]]),
            busy: true,
            ..Fake::default()
        };
        let error = prepare(DISPLAY, None, None, &mut backend)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Close cargo display monitor and cargo display volume"));
        assert_eq!(backend.waits, 0);
        backend.snapshots = VecDeque::from([vec![port("COM2", DOWNLOAD_PID, KNOB)]]);
        assert!(prepare(DISPLAY, None, None, &mut backend).is_err());
        assert!(backend.requests.is_empty());
    }

    #[test]
    fn identity_is_exact_and_macos_callout_alias_is_preferred() {
        for invalid in [
            "",
            "80B54EC7F3D",
            "80B54EC7F3DC00",
            "80B54EC7F3DG",
            "80:B54E:C7:F3:DC",
        ] {
            assert!(!is_device(
                &port("COM4", RUNTIME_PID, invalid),
                DISPLAY,
                RUNTIME_PID
            ));
        }
        let mut other_vendor = port("COM4", RUNTIME_PID, DISPLAY);
        if let SerialPortType::UsbPort(info) = &mut other_vendor.port_type {
            info.vid = 0x1234;
        }
        assert!(!is_device(&other_vendor, DISPLAY, RUNTIME_PID));
        let ports = [
            port("/dev/tty.usbmodem1", DOWNLOAD_PID, DISPLAY),
            port("/dev/cu.usbmodem1", DOWNLOAD_PID, DISPLAY),
        ];
        assert_eq!(
            select(&ports, DISPLAY, DOWNLOAD_PID, None, "DISPLAY_PORT").unwrap(),
            Some("/dev/cu.usbmodem1".into())
        );
        assert!(same_path(r"\\.\COM7", "com7"));
    }
}
