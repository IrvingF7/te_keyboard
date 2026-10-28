//! A tiny PC-side Rust program gives us `cargo knob build/flash/monitor`.
//! It selects the right CPU/toolchain and keeps USB identities out of firmware.
use std::{
    env, fs,
    path::PathBuf,
    process::{Command, ExitCode},
};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    let board = args
        .first()
        .ok_or("usage: cargo <knob|display> <build|flash|monitor> [extra args]")?;
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
    if !["build", "flash", "monitor"].contains(&action) {
        return Err("action must be build, flash or monitor".into());
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let generated = root.join("target").join(format!("{board}-config"));
    fs::create_dir_all(&generated)?;
    let defaults = generated.join("sdkconfig.defaults");
    let config = format!("{}\nCONFIG_ESPTOOLPY_FLASHSIZE_{size}=y\nCONFIG_ESPTOOLPY_FLASHSIZE=\"{size}\"\nCONFIG_PARTITION_TABLE_CUSTOM_FILENAME=\"{}\"\n",
        fs::read_to_string(root.join("sdkconfig.defaults"))?, root.join("partitions.csv").display());
    // Only touch the defaults when they change, preserving incremental builds.
    if fs::read_to_string(&defaults).ok().as_deref() != Some(&config) {
        fs::write(&defaults, config)?;
    }
    let port = env::var(format!("{}_PORT", board.to_uppercase())).unwrap_or_else(|_| {
        format!("/dev/serial/by-id/usb-Espressif_USB_JTAG_serial_debug_unit_{serial}-if00")
    });
    let mut cmd = Command::new("cargo");
    if action == "monitor" {
        cmd.args(["espflash", "monitor", "--port", &port]);
    } else {
        cmd.arg(format!("+{toolchain}"));
        if action == "flash" {
            cmd.args(["espflash", "flash", "--port", &port]);
        } else {
            cmd.arg("build");
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
    cmd.args(args.iter().skip(2));
    cmd.current_dir(&root)
        .env("ESP_IDF_SYS_ROOT_CRATE", format!("keyboard-{board}"))
        .env("ESP_IDF_SDKCONFIG_DEFAULTS", &defaults)
        .env("ESP_IDF_SDKCONFIG", generated.join("sdkconfig"));
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
    println!("{board}: {action} ({target})");
    let status = cmd.status()?;
    if !status.success() {
        return Err(format!("{action} failed: {status}").into());
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
