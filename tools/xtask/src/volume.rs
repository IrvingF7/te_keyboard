//! Optional, read-only PipeWire feedback over the display's runtime CDC port.
//! HID remains responsible for changing volume; this helper only observes it.
use serialport::SerialPort;
use std::{
    io::{self, Read, Write},
    process::{Child, Command, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

type Error = Box<dyn std::error::Error>;
const POLL: Duration = Duration::from_millis(100);
const HEARTBEAT: Duration = Duration::from_secs(1);
const QUERY_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_QUERY_BYTES: u64 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Reading {
    percent: u16,
    muted: bool,
}

fn parse_reading(text: &str) -> Result<Reading, &'static str> {
    let mut words = text.split_whitespace();
    if words.next() != Some("Volume:") {
        return Err("unexpected wpctl volume response");
    }
    let level = words
        .next()
        .and_then(|value| value.parse::<f64>().ok())
        .ok_or("invalid wpctl volume level")?;
    let percent = level * 100.0;
    if !percent.is_finite() || !(0.0..=999.0).contains(&percent) {
        return Err("wpctl volume must be between 0% and 999%");
    }
    let muted = match words.next() {
        None => false,
        Some("[MUTED]") if words.next().is_none() => true,
        _ => return Err("unexpected wpctl mute state"),
    };
    Ok(Reading {
        percent: percent.round() as u16,
        muted,
    })
}

fn wait_output(child: &mut Child, timeout: Duration) -> io::Result<Vec<u8>> {
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < timeout => sleep(Duration::from_millis(5)),
            result => {
                // Reap even a timed-out process. A stopped audio service must
                // not leave an unbounded query or an orphan wpctl behind.
                let _ = child.kill();
                let _ = child.wait();
                return Err(match result {
                    Err(error) => error,
                    _ => io::Error::new(io::ErrorKind::TimedOut, "wpctl query timed out"),
                });
            }
        }
    };
    if !status.success() {
        return Err(io::Error::other(format!("wpctl exited with {status}")));
    }
    let mut bytes = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        stdout.take(MAX_QUERY_BYTES + 1).read_to_end(&mut bytes)?;
    }
    if bytes.len() as u64 > MAX_QUERY_BYTES {
        return Err(io::Error::other("wpctl response was too long"));
    }
    Ok(bytes)
}

fn query() -> Result<Reading, Error> {
    let mut child = Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let bytes = wait_output(&mut child, QUERY_TIMEOUT)?;
    Ok(parse_reading(std::str::from_utf8(&bytes)?)?)
}

fn snapshot(reading: Option<Reading>) -> String {
    match reading {
        Some(reading) => format!("VOL {} {}\n", reading.percent, u8::from(reading.muted)),
        None => "VOL ?\n".to_owned(),
    }
}

pub fn once(args: &[String]) -> Result<bool, Error> {
    match args {
        [] => Ok(false),
        [arg] if arg == "--once" => Ok(true),
        _ => Err("display volume accepts only --once; Ctrl+C exits the continuous bridge".into()),
    }
}

fn open(path: &str) -> Result<Box<dyn SerialPort>, Error> {
    // serialport opens exclusively by default on Linux. Do not reset the MCU
    // or invoke the ROM download handshake on its running CDC interface.
    let mut port = serialport::new(path, 115_200)
        .timeout(Duration::from_millis(20))
        .open()?;
    port.write_data_terminal_ready(true)?;
    Ok(port)
}

fn drain_logs(port: &mut dyn SerialPort) -> Result<(), Error> {
    // The firmware logs periodically. Drain a bounded amount without flooding
    // the terminal, and never wait for a line that may not be complete yet.
    let mut bytes = [0u8; 512];
    for _ in 0..4 {
        let count = (port.bytes_to_read()? as usize).min(bytes.len());
        if count == 0 {
            break;
        }
        match port.read(&mut bytes[..count]) {
            Ok(0) => return Err("display CDC disconnected".into()),
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::TimedOut => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub fn run(path: &str, once: bool) -> Result<(), Error> {
    if !cfg!(target_os = "linux") {
        return Err("display volume currently supports Linux with PipeWire/WirePlumber".into());
    }
    eprintln!(
        "Volume feedback: {path}. Reads the default PipeWire output; close cargo display monitor before connecting. Ctrl+C exits."
    );
    if once {
        let mut port = open(path).map_err(|error| {
            format!("Cannot open display CDC: {error}. Close cargo display monitor and retry.")
        })?;
        let reading = query();
        // A newline also recovers from a previous process's partial write.
        port.write_all(format!("\n{}", snapshot(reading.as_ref().ok().copied())).as_bytes())?;
        let reading = reading?;
        eprintln!(
            "Sent {}%{}.",
            reading.percent,
            if reading.muted { " (muted)" } else { "" }
        );
        return Ok(());
    }

    let mut port: Option<Box<dyn SerialPort>> = None;
    let mut next_open = Instant::now();
    let mut last_sent: Option<Option<Reading>> = None;
    let mut last_write = Instant::now();
    let mut audio_error = None;
    let mut port_error = None;
    loop {
        let cycle = Instant::now();
        let reading = match query() {
            Ok(reading) => {
                if audio_error.take().is_some() {
                    eprintln!("PipeWire volume feedback restored.");
                }
                Some(reading)
            }
            Err(error) => {
                let error = error.to_string();
                if audio_error.as_ref() != Some(&error) {
                    eprintln!("Volume unknown: {error}. Retrying automatically.");
                    audio_error = Some(error);
                }
                None
            }
        };

        if port.is_none() && Instant::now() >= next_open {
            match open(path) {
                Ok(connected) => {
                    port = Some(connected);
                    last_sent = None;
                    port_error = None;
                    eprintln!("Display connected; forwarding volume feedback.");
                }
                Err(error) => {
                    let error = error.to_string();
                    if port_error.as_ref() != Some(&error) {
                        eprintln!("Waiting for display: {error}. Close any display monitor; retrying automatically.");
                        port_error = Some(error);
                    }
                    next_open = Instant::now() + HEARTBEAT;
                }
            }
        }
        if let Some(connected) = port.as_mut() {
            let transfer = (|| -> Result<(), Error> {
                drain_logs(connected.as_mut())?;
                if last_sent != Some(reading) || last_write.elapsed() >= HEARTBEAT {
                    if last_sent.is_none() {
                        connected.write_all(b"\n")?;
                    }
                    connected.write_all(snapshot(reading).as_bytes())?;
                    last_sent = Some(reading);
                    last_write = Instant::now();
                }
                Ok(())
            })();
            if let Err(error) = transfer {
                eprintln!("Display disconnected: {error}. Reconnecting automatically.");
                port = None;
                next_open = Instant::now() + HEARTBEAT;
            }
        }
        sleep(POLL.saturating_sub(cycle.elapsed()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_level_mute_zero_and_amplified_volume() {
        for (response, percent, muted) in [
            ("Volume: 0.18\n", 18, false),
            ("Volume: 0.00 [MUTED]\n", 0, true),
            ("Volume: 0.00", 0, false),
            ("Volume: 0.185", 19, false),
            ("Volume: 1.50 [MUTED]", 150, true),
            ("Volume: 9.99", 999, false),
        ] {
            assert_eq!(parse_reading(response), Ok(Reading { percent, muted }));
        }
    }

    #[test]
    fn rejects_invalid_or_unrepresentable_audio_feedback() {
        for response in [
            "",
            "0.18",
            "Volume:",
            "Volume: nan",
            "Volume: inf",
            "Volume: -0.1",
            "Volume: 10",
            "Volume: 9.991",
            "Volume: 0.2 [UNKNOWN]",
            "Volume: 0.2 extra",
            "Volume: 0.2 [MUTED] extra",
            "Volume: 0,18",
            "Volume: 1e999",
        ] {
            assert!(parse_reading(response).is_err(), "accepted {response:?}");
        }
    }

    #[test]
    fn snapshots_distinguish_unknown_zero_and_mute() {
        assert_eq!(snapshot(None), "VOL ?\n");
        assert_eq!(
            snapshot(Some(Reading {
                percent: 0,
                muted: false
            })),
            "VOL 0 0\n"
        );
        assert_eq!(
            snapshot(Some(Reading {
                percent: 150,
                muted: true
            })),
            "VOL 150 1\n"
        );
    }

    #[test]
    fn volume_arguments_do_not_accept_monitor_or_flash_flags() {
        assert!(!once(&[]).unwrap());
        assert!(once(&["--once".into()]).unwrap());
        assert!(once(&["--non-interactive".into()]).is_err());
        assert!(once(&["--once".into(), "extra".into()]).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn hung_queries_are_killed_and_reaped() {
        let mut child = Command::new("sleep")
            .arg("5")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let start = Instant::now();
        let error = wait_output(&mut child, Duration::from_millis(30)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(child.try_wait().unwrap().is_some());
    }
}
