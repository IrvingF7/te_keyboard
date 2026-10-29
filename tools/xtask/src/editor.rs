//! rust-analyzer needs one target/toolchain at a time. Switch its board view
//! without changing how either board builds, or overwriting unrelated settings.
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs,
    path::Path,
    process::{Command, ExitStatus, Stdio},
};

pub fn check(
    command: &mut Command,
    root: &Path,
    target: &str,
) -> Result<ExitStatus, Box<dyn std::error::Error>> {
    let metadata = Command::new("cargo")
        .args(["+stable", "metadata", "--format-version=1", "--locked"])
        .args(["--filter-platform", target])
        .current_dir(root)
        .stderr(Stdio::inherit())
        .output()?;
    if !metadata.status.success() {
        return Err("could not load Cargo metadata for the editor".into());
    }
    let metadata: Value = serde_json::from_slice(&metadata.stdout)?;
    let packages: HashSet<&str> = metadata["packages"]
        .as_array()
        .ok_or("Cargo metadata has no packages")?
        .iter()
        .filter_map(|package| package["id"].as_str())
        .collect();
    let output = command.stderr(Stdio::inherit()).output()?;
    for line in std::str::from_utf8(&output.stdout)?.lines() {
        let message: Value = serde_json::from_str(line)?;
        // build-std emits artifact/build-script records for compiler-internal
        // crates that are absent from the application's Cargo metadata. Some
        // host-only dependencies are also absent from target-filtered metadata. Feeding
        // those to rust-analyzer causes "unknown package" log errors. Keep all
        // actual compiler diagnostics, including errors in std itself, and keep
        // the real command exit status. This does not hide compilation failures.
        let build_record = matches!(
            message["reason"].as_str(),
            Some("compiler-artifact" | "build-script-executed")
        );
        if build_record
            && !message["package_id"]
                .as_str()
                .is_some_and(|id| packages.contains(id))
        {
            continue;
        }
        println!("{line}");
    }
    Ok(output.status)
}

pub fn configure(
    root: &Path,
    board: &str,
    target: &str,
    toolchain: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = root.join(".vscode/settings.json");
    let mut settings: Value = match fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(error) => return Err(error.into()),
    };
    let settings = settings
        .as_object_mut()
        .ok_or("VS Code settings must be a JSON object")?;
    // The helper runs on the PC's stable compiler; it selects the firmware's
    // compiler internally. Both initial build-data loading and check-on-save
    // must use it: redirecting only check-on-save leaves startup broken.
    let command = json!([
        "cargo",
        "+stable",
        "run",
        "--quiet",
        "-p",
        "xtask",
        "--",
        board,
        "ide-check"
    ]);
    let extra_env = settings
        .entry("rust-analyzer.cargo.extraEnv")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("rust-analyzer.cargo.extraEnv must be a JSON object")?;
    // The esp toolchain's Cargo lacks --lockfile-path, which this rust-analyzer
    // sends to nightly Cargo. Use stable Cargo for S3 metadata queries while
    // selecting the actual esp standard library/proc-macro server explicitly.
    // Firmware checks still use the esp compiler through the command above.
    extra_env.insert(
        "RUSTUP_TOOLCHAIN".into(),
        json!(if board == "display" {
            "stable"
        } else {
            toolchain
        }),
    );
    let sysroot = if board == "display" {
        let output = Command::new("rustup")
            .args(["run", toolchain, "rustc", "--print", "sysroot"])
            .output()?;
        if !output.status.success() {
            return Err("could not locate the esp compiler's standard library".into());
        }
        String::from_utf8(output.stdout)?.trim().to_owned()
    } else {
        "discover".to_owned()
    };
    settings.insert("rust-analyzer.cargo.sysroot".into(), json!(sysroot));
    settings.insert(
        "rust-analyzer.procMacro.server".into(),
        if board == "display" {
            json!(Path::new(&sysroot).join("libexec/rust-analyzer-proc-macro-srv"))
        } else {
            Value::Null
        },
    );
    let excluded = settings
        .entry("rust-analyzer.files.exclude")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or("rust-analyzer.files.exclude must be a JSON array")?;
    excluded.retain(|entry| entry != "firmware/knob" && entry != "firmware/display");
    excluded.push(json!(if board == "knob" {
        "firmware/display"
    } else {
        "firmware/knob"
    }));
    for (key, value) in [
        ("rust-analyzer.cargo.target", json!(target)),
        ("rust-analyzer.cargo.allTargets", json!(false)),
        ("rust-analyzer.cargo.buildScripts.enable", json!(true)),
        (
            "rust-analyzer.cargo.buildScripts.useRustcWrapper",
            json!(false),
        ),
        (
            "rust-analyzer.cargo.buildScripts.overrideCommand",
            command.clone(),
        ),
        ("rust-analyzer.check.overrideCommand", command),
        ("rust-analyzer.procMacro.enable", json!(true)),
        ("rust-analyzer.cfg.setTest", json!(false)),
        ("rust-analyzer.linkedProjects", json!(["Cargo.toml"])),
        ("rust-analyzer.restartServerOnConfigChange", json!(true)),
    ] {
        settings.insert(key.into(), value);
    }
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(settings)?),
    )?;
    println!("rust-analyzer now targets {board} ({target}).");
    println!("If VS Code does not reload automatically, run 'rust-analyzer: Restart server'.");
    Ok(())
}
