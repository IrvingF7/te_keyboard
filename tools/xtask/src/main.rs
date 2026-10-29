//! A tiny PC-side Rust program gives us `cargo knob build/flash/monitor/check/ide`.
//! It selects the right CPU/toolchain and keeps USB identities out of firmware.
mod editor;
mod monitor;
use std::{
    env, fs,
    path::PathBuf,
    process::{Command, ExitCode},
    time::{Duration, Instant},
};

fn default_port(board: &str, serial: &str, monitor: bool) -> String {
    if board == "display" && monitor {
        format!(
            "/dev/serial/by-id/usb-T-Keyboard_Volume_Display_{}-if00",
            serial.replace(':', "")
        )
    } else {
        format!("/dev/serial/by-id/usb-Espressif_USB_JTAG_serial_debug_unit_{serial}-if00")
    }
}

fn port_for(board: &str, serial: &str, monitor: bool) -> String {
    let prefix = board.to_uppercase();
    if monitor {
        if let Ok(port) = env::var(format!("{prefix}_MONITOR_PORT")) {
            return port;
        }
    }
    env::var(format!("{prefix}_PORT")).unwrap_or_else(|_| default_port(board, serial, monitor))
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    let board = args
        .first()
        .ok_or("usage: cargo <knob|display> <build|flash|monitor|check|ide> [extra args]")?;
    let action = args.get(1).map(String::as_str).unwrap_or("build");
    let (target, toolchain, serial, size) = match board.as_str() {
        "knob" => (
            "riscv32imac-esp-espidf",
            "nightly-2026-04-15",
            "40:4C:CA:5C:39:30",
            "4MB",
        ),
        "display" => ("xtensa-esp32s3-espidf", "esp", "80:B5:4E:C7:F3:DC", "16MB"),
        _ => return Err("board must be knob or display".into()),
    };
    if !["build", "flash", "monitor", "check", "ide", "ide-check"].contains(&action) {
        return Err("action must be build, flash, monitor, check or ide".into());
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    if action == "ide" {
        return editor::configure(&root, board, target, toolchain);
    }
    if board == "display" && action == "monitor" {
        let interactive = monitor::interactive(&args[2..])?;
        return monitor::run(&port_for(board, serial, true), interactive);
    }
    let generated = root.join("target").join(format!("{board}-config"));
    fs::create_dir_all(&generated)?;
    let defaults = generated.join("sdkconfig.defaults");
    let config = format!("{}\n{}\nCONFIG_ESPTOOLPY_FLASHSIZE_{size}=y\nCONFIG_ESPTOOLPY_FLASHSIZE=\"{size}\"\nCONFIG_PARTITION_TABLE_CUSTOM_FILENAME=\"{}\"\n",
        fs::read_to_string(root.join("sdkconfig.defaults"))?,
        fs::read_to_string(root.join("firmware").join(board).join("sdkconfig.defaults"))?,
        root.join("partitions.csv").display());
    // Only touch the defaults when they change, preserving incremental builds.
    if fs::read_to_string(&defaults).ok().as_deref() != Some(&config) {
        fs::write(&defaults, config)?;
    }
    let port = port_for(board, serial, action == "monitor");
    // The S3's running HID firmware and ROM downloader enumerate separately.
    // Let espflash upload via ROM, then open our CDC interface for --monitor.
    let monitor_after_flash = board == "display"
        && action == "flash"
        && args
            .iter()
            .skip(2)
            .any(|arg| arg == "--monitor" || arg == "-M");
    if board == "display"
        && action == "flash"
        && port.starts_with("/dev/")
        && !std::path::Path::new(&port).exists()
    {
        return Err(format!(
            "Display ROM download port is unavailable: {port}. Hold display BOOT, tap RESET (or reconnect its USB cable), then release BOOT and retry. Use DISPLAY_PORT to override the download port."
        ).into());
    }
    let mut cmd = Command::new("cargo");
    if action == "monitor" {
        cmd.args(["espflash", "monitor", "--port", &port]);
    } else {
        cmd.arg(format!("+{toolchain}"));
        if action == "flash" {
            cmd.args(["espflash", "flash", "--port", &port]);
            // cargo-espflash's Xtensa preflight checks arguments/config only;
            // it does not read CARGO_UNSTABLE_BUILD_STD from the environment.
            cmd.args(["-Z", "build-std=std,panic_abort"]);
        } else if action == "ide-check" {
            cmd.args(["check", "--message-format=json"]);
        } else {
            cmd.arg(action);
        }
        cmd.args([
            "--release",
            "--locked",
            "--package",
            &format!("keyboard-{board}"),
            "--target",
            target,
        ]);
    }
    cmd.args(args.iter().skip(2).filter(|arg| {
        !monitor_after_flash || (arg.as_str() != "--monitor" && arg.as_str() != "-M")
    }));
    cmd.current_dir(&root)
        // Only firmware commands need to rebuild std for ESP32. Keep that out
        // of the workspace-wide settings used by host tools and editor probes.
        .env("CARGO_UNSTABLE_BUILD_STD", "std,panic_abort")
        .env("ESP_IDF_SYS_ROOT_CRATE", format!("keyboard-{board}"))
        .env("ESP_IDF_SDKCONFIG_DEFAULTS", &defaults)
        .env("ESP_IDF_SDKCONFIG", generated.join("sdkconfig"));
    if board == "display" {
        // TinyUSB's CMake target sets this define for C compilation. Bindgen
        // needs the same target definition before parsing the USB headers.
        let key = format!("BINDGEN_EXTRA_CLANG_ARGS_{target}");
        let existing = env::var(&key)
            .or_else(|_| env::var(key.replace('-', "_")))
            .or_else(|_| env::var("BINDGEN_EXTRA_CLANG_ARGS"))
            .unwrap_or_default();
        cmd.env(key, format!("{existing} -DCFG_TUSB_MCU=OPT_MCU_ESP32S3"));
    }
    // espup writes this one variable for bindgen. Reading its quoted value
    // avoids depending on shell startup files or executing arbitrary shell text.
    if env::var_os("LIBCLANG_PATH").is_none() {
        if let Some(home) = env::var_os("HOME") {
            if let Ok(exports) = fs::read_to_string(PathBuf::from(home).join("export-esp.sh")) {
                for line in exports.lines() {
                    if let Some(value) = line.strip_prefix("export LIBCLANG_PATH=") {
                        cmd.env("LIBCLANG_PATH", value.trim_matches('"').trim_matches('\''));
                    }
                }
            }
        }
    }
    // Leave stdout for Cargo's JSON diagnostics when invoked by rust-analyzer.
    eprintln!("{board}: {action} ({target})");
    let status = if action == "ide-check" {
        editor::check(&mut cmd, &root, target)?
    } else {
        cmd.status()?
    };
    if !status.success() {
        return Err(format!("{action} failed: {status}").into());
    }
    if monitor_after_flash {
        let monitor_port = port_for(board, serial, true);
        // Runtime USB appears after reset. Wait only for filesystem-style Linux
        // ports; explicit Windows COM names are opened directly by the terminal.
        let start = Instant::now();
        while monitor_port.starts_with("/dev/")
            && !std::path::Path::new(&monitor_port).exists()
            && start.elapsed() < Duration::from_secs(10)
        {
            std::thread::sleep(Duration::from_millis(100));
        }
        let interactive = !args.iter().any(|arg| arg == "--non-interactive");
        monitor::run(&monitor_port, interactive).map_err(|e| format!(
            "Upload succeeded, but runtime monitoring failed: {e}. Set DISPLAY_MONITOR_PORT to the display's CDC port and run cargo display monitor."
        ))?;
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_runtime_and_download_ports_are_distinct() {
        let serial = "80:B5:4E:C7:F3:DC";
        assert_eq!(
            default_port("display", serial, true),
            "/dev/serial/by-id/usb-T-Keyboard_Volume_Display_80B54EC7F3DC-if00"
        );
        let rom =
            "/dev/serial/by-id/usb-Espressif_USB_JTAG_serial_debug_unit_80:B5:4E:C7:F3:DC-if00";
        assert_eq!(default_port("display", serial, false), rom);
        assert_eq!(default_port("knob", serial, true), rom);
        assert_eq!(default_port("knob", serial, false), rom);
    }
}
