//! A plain CDC terminal. espflash's monitor performs a ROM bootloader handshake,
//! which our running composite USB device intentionally does not implement.
use std::{
    io::{self, BufRead, Read, Write},
    time::Duration,
};

pub fn interactive(args: &[String]) -> Result<bool, Box<dyn std::error::Error>> {
    match args {
        [] => Ok(true),
        [arg] if arg == "--non-interactive" => Ok(false),
        _ => Err("display monitor accepts only --non-interactive; type commands followed by Enter, and use Ctrl+C to exit".into()),
    }
}

pub fn run(path: &str, interactive: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut port = serialport::new(path, 115_200)
        .timeout(Duration::from_millis(100))
        .open()?;
    port.write_data_terminal_ready(true)?;
    if interactive {
        let mut writer = port.try_clone()?;
        std::thread::spawn(move || {
            for line in io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                if writeln!(writer, "{line}").is_err() {
                    break;
                }
            }
        });
    }
    eprintln!("Display CDC: {path}. Commands: d / b / ? + Enter; Ctrl+C exits.");
    let mut buffer = [0u8; 1024];
    let mut output = io::stdout().lock();
    loop {
        match port.read(&mut buffer) {
            Ok(0) => return Err("display CDC disconnected".into()),
            Ok(count) => {
                output.write_all(&buffer[..count])?;
                output.flush()?;
            }
            Err(error) if error.kind() == io::ErrorKind::TimedOut => (),
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_rom_monitor_options_are_rejected() {
        assert!(interactive(&[]).unwrap());
        assert!(!interactive(&["--non-interactive".into()]).unwrap());
        assert!(interactive(&["--no-reset".into()]).is_err());
    }
}
