# T-Keyboard — Rust firmware

Two independent Rust programs, built with Cargo and ESP-IDF:

- **T-Knob / ESP32-C6:** MT6701 encoder, TMC6300 motor, 24 haptic detents.
- **T-Display-Bar / ESP32-S3:** USB HID volume controls, ESP-NOW telemetry,
  LCD sleep/wake and backlight controls.

The application, board adapters, tests, and build helper are Rust. ESP-IDF is
Espressif's native SDK, linked through Rust bindings; there is no Arduino
framework or dependency on either vendor reference checkout.

## Build, upload, and monitor

From this directory:

```sh
cargo test
cargo knob check
cargo display check
cargo knob build
cargo display build
cargo knob flash
cargo display flash
cargo knob monitor
cargo display monitor
```

Run monitors in separate terminals; close a board's monitor before flashing it.
`flash` builds and uploads, then resets the board. Add `--monitor` to open its
serial monitor immediately afterward, for example `cargo knob flash --monitor`.
Uploading replaces that board's firmware. Let the knob align untouched for
about one second after a reset.

Once the display runs HID firmware, its normal USB port is a composite HID + CDC
serial device. To flash it again, hold the display's BOOT button, tap RESET (or
reconnect its USB cable while holding BOOT), then release BOOT. This exposes the
ROM download port used by `cargo display flash`. The helper uses the runtime CDC
port for `cargo display monitor` and switches to it after `cargo display flash
--monitor`. Display commands in this plain serial terminal require Enter; Ctrl+C
exits. `--non-interactive` disables keyboard input. The terminal does not reset
the display or perform a ROM bootloader handshake.

The two aliases invoke a small host-side Rust helper (`tools/xtask`). It selects
chip architecture, compiler, flash size, SDK settings and a persistent Linux
USB path. These ROM download IDs identify the boards even when their `ttyACM`
numbers change:

| Board | Target | Flash | USB serial ID |
| --- | --- | --- | --- |
| Knob | `riscv32imac-esp-espidf` | 4 MiB | `40:4C:CA:5C:39:30` |
| Display | `xtensa-esp32s3-espidf` | 16 MiB | `80:B5:4E:C7:F3:DC` |

The display's runtime CDC interface is named `T-Keyboard Volume Display`, with
its chip MAC as serial number (`80B54EC7F3DC` on this board). Its default Linux
path is `/dev/serial/by-id/usb-T-Keyboard_Volume_Display_80B54EC7F3DC-if00`.

Override a port when needed, including on Windows/macOS:

```sh
KNOB_PORT=/dev/ttyACM2 cargo knob flash
DISPLAY_PORT=/dev/ttyACM1 cargo display monitor
DISPLAY_MONITOR_PORT=/dev/ttyACM1 cargo display flash --monitor
cargo espflash list-ports
```

`DISPLAY_PORT` selects a port for the current action. `DISPLAY_MONITOR_PORT`
overrides just the runtime monitor port, including after an upload; ROM and
runtime devices can receive different COM/tty names. The corresponding
`KNOB_PORT` / `KNOB_MONITOR_PORT` overrides also work. Build/upload arguments are
forwarded to Cargo or cargo-espflash. The display's raw monitor accepts only
`--non-interactive`; it does not support espflash's ROM monitor options.
`check` checks the firmware without producing a linked executable or uploading.

`xtask` is a community convention for a project's own Rust development helper;
it is an ordinary binary package. For example, `cargo knob build` expands to
`cargo run --quiet -p xtask -- knob build`. The root `Cargo.toml` groups the
packages in a workspace; each member's `Cargo.toml` declares that package's
name, dependencies and features. Workspace dependency entries provide shared
versions that members can explicitly inherit with `workspace = true`.

## Install on a new development computer

Install Rust using rustup, plus the system prerequisites in the
[Rust ESP-IDF template](https://github.com/esp-rs/esp-idf-template). On Linux,
these include a C compiler, CMake/Ninja, Python with venv support, pkg-config,
and the normal development headers required by the tools below.

The tested tool versions are:

```sh
cargo install espup --version 0.19.0 --locked
cargo install cargo-espflash --version 4.6.0 --locked
cargo install ldproxy --version 0.3.5 --locked
rustup toolchain install nightly-2026-04-15 --component rust-src --profile minimal
espup install --targets esp32s3 --toolchain-version 1.95.0.0
```

The first firmware build downloads ESP-IDF **v5.3.2** and its native tools.
Rust dependencies are recorded in `Cargo.lock`; board commands use `--locked`.
The display also downloads Espressif's `esp_tinyusb` 1.7.6 component. Native USB
dependency versions and hashes are recorded in `components_esp32s3.lock`.
The host tools require Rust 1.95 or newer. C6 uses the pinned RISC-V nightly;
S3 uses the `esp` Xtensa compiler installed above. ESP-IDF supplies the runtime
and FreeRTOS underneath Rust's standard library. No desktop OS runs on the MCU.

`espup` writes `~/export-esp.sh`. Our helper reads its `LIBCLANG_PATH` value for
binding generation. If using a custom export location, source that file before
building. The helper generates board-specific SDK defaults beneath `target/`,
including an absolute path to `partitions.csv`. Common defaults are in
`sdkconfig.defaults`, with USB settings in each firmware's `sdkconfig.defaults`.
A 3 MiB application partition fits either board; the bar's
additional flash and PSRAM are not needed by this firmware.

## Controls

| Board / button | Action | Serial shortcut |
| --- | --- | --- |
| Knob BOOT, GPIO 9 | Toggle haptics | `m` |
| Bar BOOT, GPIO 0 | LCD sleep/wake | `d` |
| Bar Button1, GPIO 38 | Backlight off/on | `b` |
| Bar serial | Report panel and backlight state | `?` |

Press BOOT after startup. Holding it during reset selects download mode.
Buttons require 30 ms of stable input and trigger once per press. Defaults are
haptics, LCD and backlight ON; these preferences are kept in RAM only.

Backlight off leaves the LCD controller running but removes the light that
makes its image visible. LCD sleep also stops drawing and sends display-off /
sleep-in commands. The MCU, radio, USB and buttons remain awake. Wake waits
120 ms without blocking the loop, redraws, then restores the saved backlight
preference. Toggling backlight while asleep changes that preference only.

These controls let us compare power modes; they do not measure USB current.
Brightness dimming and automatic idle sleep have not been added.

## How the Rust code is organized

```text
Cargo.toml / Cargo.lock      workspace and dependency versions
.cargo/config.toml          board aliases and compiler/linker configuration
crates/keyboard-core/       portable logic and PC unit tests
  src/controls.rs           debouncing and LCD power state machine
  src/motor.rs              detents, encoder CRC, alignment math
  src/protocol.rs           explicit packet encoding and link state
  src/volume.rs             step tracking and HID press/release sequencing
  src/usb.rs                composite USB and HID report descriptors
crates/board-support/       USB, SPI and ESP-NOW adapters for ESP-IDF
  src/usb.rs                S3 HID/CDC adapter and TinyUSB callbacks
firmware/knob/src/          encoder, motor driver, haptic task, main loop
firmware/display/src/       Rust framebuffer/panel driver and main loop
tools/xtask/                PC-side Rust build/upload command helper
T-Knob/                    ignored vendor hardware reference
T-Display-Bar/             ignored vendor hardware reference
backups/                   ignored recovery images and hardware logs
```

Start learning in `keyboard-core`, which uses no ESP-IDF APIs. An `enum` models
mutually exclusive states; `match` handles each state explicitly; `Option`
represents a reading/packet that may be absent. `Result` propagates hardware
errors with `?`. Comments explain why choices are made, not just syntax.

The knob's motor task owns the encoder and PWM driver. `Arc` shares a command
flag and telemetry storage with the UI; `AtomicBool` carries on/off requests.
The motor uses `try_lock` to publish a complete snapshot, so it never waits for
a slow USB logger. Only the UI formats text and sends radio packets.

ESP-IDF APIs that Rust cannot verify use small `unsafe` sections. In particular,
SPI transaction memory must remain alive until completion, and the motor timer
interrupt shares state through an ISR-safe critical section. Rust does not
make incorrect pin maps, electrical settings or motor tuning safe automatically.

The editor recommendation is rust-analyzer. Select the board you are editing:

```sh
cargo knob ide
# Or switch to the display:
cargo display ide
```

These commands update `.vscode/settings.json`, preserving unrelated settings.
The committed default is the knob. VS Code should restart rust-analyzer when
the configuration changes; if it does not, run **rust-analyzer: Restart server**
from the Command Palette. Open this repository folder directly in VS Code so
its folder settings apply.

One board is analyzed at a time: shared libraries remain included, while the
other board's firmware folder is excluded until you switch. This keeps C6 and
S3 hardware definitions from being interpreted using the wrong chip settings.
The editor loads build-script outputs and checks the selected firmware on save
using the same helper as the terminal. Run `cargo test` for host unit tests and
`cargo clippy -p keyboard-core -p xtask -- -D warnings` for host-tool checking.

Both the initial build-data command and the check-on-save command must select
the correct board. Configuring only check-on-save can leave rust-analyzer trying
to build ESP-IDF for `x86_64-unknown-linux-gnu`. The helper also removes Cargo
artifact bookkeeping for crates outside the editor's target metadata, while
preserving all compiler diagnostics and failed exit statuses. Standard-library
rebuilding is scoped to firmware commands rather than every Cargo invocation.

The display's older `esp` Cargo lacks a metadata flag used by this rust-analyzer
version. Its editor configuration uses stable Cargo for metadata, with the
actual `esp` standard library and macro server selected explicitly. Firmware
compilation continues to use the `esp` compiler. Re-run `cargo display ide`
after moving or reinstalling that toolchain to refresh the generated paths.

## Haptics and timing

The MT6701 returns a 14-bit angle plus status and CRC in a 24-bit SPI frame.
Both bad checksums and field/tracking errors are rejected. SPI waits are bounded;
a timed-out transaction is never reused or freed while hardware might own it.

The control task runs approximately every 1 ms. At startup a fixed magnetic
field ramps to a 1 V command over 250 ms. After a full 200 ms settling window
and at least one second of alignment, that angle becomes detent zero. Slow
continuous drift does not count as settling. Failure to settle within two
seconds latches a fault.

There are seven motor pole pairs and 24 detents per revolution. A proportional
spring plus velocity damping pulls toward the current target. Crossing 55% of
the 15-degree spacing selects the next target; this hysteresis prevents chatter.
Normal commands are limited to ±0.8 V. This is voltage mode, not measured torque,
a winding-current limit or a USB power cap. Near a stationary detent, the motor
coasts when no correction is needed.

Hardware MCPWM generates 20 kHz complementary pulses with 0.5 us dead time.
An independent GPTimer interrupt checks every 1 ms and forces all six physical
motor outputs LOW if an energized command becomes 20 ms old. Timeout is latched
until reset; a subsequent task update cannot re-enable the motor. Encoder,
driver, timing and alignment failures also latch faults.

The watchdog is software running in an interrupt. Interrupt masking or flash
cache operations can delay it; it is not an external hardware interlock. The
application performs no flash writes during motor operation. Driver hardware
protections remain in place. Serial logs report fault reason, encoder errors,
maximum task interval and watchdog cutoff age.

A deliberate 50 ms task-stall test is available as a separate build:

```sh
cargo knob flash --features watchdog-self-test
cargo knob monitor
# After inspecting cutoff_us and the latched fault, restore normal firmware:
cargo knob flash
```

The normal build does not enable that feature.

## ESP-NOW and display

Both devices send four heartbeats per second on channel 1. Knob step/flag changes
also trigger packets, limited to one every 20 ms. No router is needed. The fixed
peer addresses are in `protocol.rs`. USB supplies power and logs, and the display
sends host volume keys; haptics remain local even if the display disconnects.

The 32-byte v1 packet is explicitly encoded little-endian, with a version,
role, random boot session, sequence number, echoed session/sequence, angle,
steps and flags. Rust never transmits its struct memory layout. Incoming
packets must match the peer MAC, length, version and role. This is an
unencrypted bench link; a MAC filter is not authentication.

`CONNECTED` requires recent incoming packets AND a fresh echo of our own
packets, proving a round trip. Connection expires after about two seconds.
The last peer values remain visible when disconnected. Wi-Fi callbacks only
validate/copy into a bounded inbox; UI and drawing happen in the main loop.

The panel driver preserves the vendor register sequence and landscape RAM
offset (18,82) for the 284×76 image. `embedded-graphics` renders text into a
RGB565 framebuffer, then Rust sends it over SPI. It does not compile or link
the vendor graphics library.

The SPI bus uses the vendor's **mode 3**: clock idle HIGH, data sampled on the
second edge. GPIO initialization also explicitly selects the GPIO pin function,
not just input/output direction. Pins have multiple possible connections inside
the chip; for example, LCD reset GPIO40 can instead serve JTAG MTDO. The Rust
adapter uses `gpio_config()` to select GPIO routing before driving those pins.
A successful SPI write confirms transmission, not a visible image on the LCD.

## Plug-and-play volume control

Connect the display's USB port to the computer and power the knob. After encoder
alignment and radio connection, positive knob steps generate **Volume Up** and
negative steps generate **Volume Down**, one key press/release per step. These
are standard USB HID Consumer Control events; no project-specific PC program is
needed. Windows and the standard Ubuntu desktop handle them as multimedia
keyboard volume keys. The desktop determines the volume increment and output
device. Physical clockwise/counterclockwise direction still needs bench
confirmation against the encoder's sign.

The display remains a CDC serial device for `d`, `b`, `?`, and periodic status
logs. Logs are best-effort and nonblocking: a closed or slow terminal cannot
stall volume control. BOOT, LCD sleep, and backlight controls retain their
previous functions. The C6 continues to use its existing Serial/JTAG port.

```text
knob cumulative steps -> ESP-NOW -> display -> VolumeSteps -> VolumeKeys
                                                        -> HID press/release
```

`VolumeSteps` establishes a baseline on the first reading or a new boot session.
`VolumeKeys` pairs each press with a release and retries when USB is busy. The
display discards pending steps and waits for a fresh radio baseline after USB
disconnect/suspend, radio loss, invalid encoder data, or a motor fault. A release
still takes priority when input disappears. Pending movement is bounded to 32
net steps and expires after 500 ms without new movement, so a slow host cannot
replay an arbitrarily large, stale burst. Radio input older than 500 ms is ignored.

HID reports are submitted from TinyUSB's task, which also handles connection
changes. A one-report mailbox provides backpressure until each transfer finishes.
If the display loop exits on a hardware error, USB disconnects to clear any held
media key at the host.

The USB descriptors are in `crates/keyboard-core/src/usb.rs`: one CDC function and
one non-boot Consumer Control HID interface. The S3 uses Espressif's development
VID and TinyUSB's CDC+HID PID. `firmware/display/usb_bindings.h` provides an opaque,
size-preserving binding for an unused CDC telephony type that otherwise triggers
Rust E0588 with the pinned ESP-IDF bindings generator. Application logic stays in
Rust; the native USB stack is Espressif/TinyUSB.

Current-volume feedback and animated pixel art remain future features. The LCD
continues to show link and knob telemetry; it does not estimate volume percentage.

After flashing, start with a low host volume and turn a few detents in each
direction. Confirm desktop volume changes, then test unplug/replug, host
suspend/resume, and knob restart for unexpected changes. Verify CDC commands
and LCD sleep while turning. Repeat on Windows before treating cross-platform
behavior as tested. Builds and portable checks do not replace these bench tests.

## Validation and recovery

Run the portable checks with:

```sh
cargo test
cargo test -p xtask
cargo clippy -p keyboard-core -p xtask -- -D warnings
cargo fmt --all -- --check
cargo knob build
cargo display build
```

Tests cover bounce, held buttons, timer wrap, rapid sleep/wake changes,
backlight preferences, encoder CRC/status, angle wrap, both rotation directions,
hysteresis, voltage limits, commutation direction, slow alignment drift, exact
packet bytes, malformed packets, round trips, peer reboot and sequence wrap.
Volume tests also cover baseline resets, count rollover, press/release pairing,
USB backpressure, stale backlog, reversal, composite descriptors, and prompt
rate-limited knob updates. Helper tests cover ROM/runtime port selection and
runtime-monitor argument handling.

On 2026-10-29, the HID integration passed 20 core tests and 2 helper tests,
portable/helper Clippy with warnings denied, formatting, and both release
firmware builds. The required code review finished with no remaining findings.
Both boards were then flashed. Ubuntu enumerated the display as `303a:4005`
(`T-Keyboard Volume Display`), with CDC and a Consumer Control interface bound
to `hid-generic`. Runtime CDC logs reported `USB READY LINK CONNECTED`, zero
radio errors, and motor/panel/backlight ON. The flashing helper now passes
`-Z build-std=std,panic_abort` explicitly because cargo-espflash's Xtensa
preflight does not inspect the build-std environment variable.
Physical volume changes, rotation direction, reconnect/suspend behavior, and
Windows operation still await bench testing. The older results below predate HID.

Both release firmware builds and all 10 host tests passed on 2026-10-27.
Clippy passed with warnings denied for the portable core, host helper and both
MCU targets. A follow-up code review identified incomplete GPIO initialization;
the correction was reviewed with no remaining actionable findings.

Both normal Rust firmwares were uploaded. Serial validation passed for the
bidirectional link, haptics off/on, backlight off/on, LCD sleep/wake, changing
backlight preference while asleep and restoring a dark preference on wake.
During that run the largest motor-task interval was **1,506 us**, with zero
encoder/send errors and no motor fault/cutoff. The separate 50 ms stall test
recorded a **20,025 us** independent watchdog cutoff and latched the motor off.
Normal firmware was restored afterward. Each board also detected the other
being held in download mode and automatically reconnected after its restart.
The final boot logs show no NVS initialization error. Logs:

- `backups/rust-controls-validation-20261027.txt`
- `backups/rust-watchdog-20261027.txt`
- `backups/rust-reconnect-validation-20261027.txt`
- `backups/rust-manual-observation-20261027.txt`
- `backups/rust-gpio-controls-validation-20261027.txt`

A subsequent read-only observation captured knob rotation and haptic button
toggles without faults; its maximum task interval was 1,510 us.
The user confirmed the knob works, but initially reported a white display.
Correcting SPI mode alone did not restore the image. After switching the GPIO
initialization to `gpio_config()`, the user confirmed visible, readable text.
Both boards were reflashed with that shared GPIO correction. USB current still
requires a meter; no oscilloscope measurement of the six motor outputs was
performed.

After the GPIO correction, the serial control sequence passed again with zero
encoder/send errors, no motor fault, and a maximum motor-task interval of
1,509 us. LCD sleep/wake and backlight preferences were exercised, and the
test finished with haptics, LCD and backlight ON.

Full pre-migration recovery images are saved for the exact connected boards:

- `backups/knob-before-rust-20261027.bin` — 4 MiB.
- `backups/display-before-rust-20261027.bin` — 16 MiB.
- `backups/rust-recovery-manifest-20261027.json` — sizes and SHA-256 checksums.

To restore a full image, close monitors and use the matching board's port:

```sh
cargo espflash write-bin --port YOUR_KNOB_PORT 0 backups/knob-before-rust-20261027.bin
cargo espflash write-bin --port YOUR_DISPLAY_PORT 0 backups/display-before-rust-20261027.bin
```

Restoring writes the complete flash, including its previous partition table and
stored data. Recovery backups are ignored by Git; preserve `backups/` separately.

If a board does not enter download mode automatically, hold BOOT, tap RESET,
release BOOT and retry. Flashing and monitoring use the same data cable.

## References

- [LILYGO T-Knob](https://github.com/Xinyuan-LilyGO/T-Knob)
- [LILYGO T-Display-Bar](https://github.com/Xinyuan-LilyGO/T-Display-Bar)
- [Rust ESP-IDF bindings](https://github.com/esp-rs/esp-idf)
- [cargo-espflash](https://github.com/esp-rs/espflash/tree/main/cargo-espflash)
- [Rust on ESP book](https://docs.espressif.com/projects/rust/book/)
