# T-Keyboard

One PlatformIO project, two independently flashed devices. Open **this folder**
(`te_keyboard`) in VS Code with the PlatformIO IDE extension.

## ESP-NOW connection test

Both test targets exchange ESP-NOW heartbeats on **Wi-Fi channel 1**, four times
per second. No router, Wi-Fi password, or wiring between boards is needed.
Each board still needs power. Both USB cables connected on 2026-10-26 support
data; the knob was identified as ESP32-C6 and the bar as ESP32-S3 via esptool.

Build and flash the two boards separately (upload replaces their firmware):

```sh
pio run -e knob_test -e display_test
pio run -e knob_test -t upload --upload-port /dev/serial/by-id/usb-Espressif_USB_JTAG_serial_debug_unit_40:4C:CA:5C:39:30-if00
pio run -e display_test -t upload --upload-port /dev/serial/by-id/usb-Espressif_USB_JTAG_serial_debug_unit_80:B5:4E:C7:F3:DC-if00
```

These persistent Linux device paths identify the current boards even if their
`ttyACM` numbers change. On other systems use `pio device list` to find the ports.
If `pio` is unavailable, use `~/.platformio/penv/bin/pio`. Close serial monitors
before uploading.

The display's pre-upload first 1 MiB is saved as
`backups/t-display-before-espnow-first-1MiB-20261026.bin`. This partial backup
covers the flash regions overwritten by this test upload; it is not a full
16 MiB device backup. The knob's original full-flash backup remains available
as described below.

The display shows:

- `CONNECTED` in green after a fresh heartbeat makes a round trip between MCUs.
  TX counts submitted packets; increasing RX and `CONNECTED` confirm the link.
- The knob's angle, signed logical steps (24 per revolution), and BOOT button.
- The age of the last received packet. After roughly two seconds without a
  working round trip, the status changes to `DISCONNECTED`. Shown telemetry is
  the last received value while disconnected.

Turn the knob and watch the angle/steps change. Press its **BOOT board button**
after startup to see `BOOT:DOWN`; the shaft itself is not a click switch. Unplug
the knob's power to check disconnection, then reconnect it to check automatic
recovery. `WAITING FOR PEER` means no valid peer packets have arrived yet;
`RADIO ERROR` means initialization failed (see serial output).

Both USB serial logs print `LINK CONNECTED` with counters once per second:

```sh
pio device monitor -e knob_test --port /dev/serial/by-id/usb-Espressif_USB_JTAG_serial_debug_unit_40:4C:CA:5C:39:30-if00
pio device monitor -e display_test --port /dev/serial/by-id/usb-Espressif_USB_JTAG_serial_debug_unit_80:B5:4E:C7:F3:DC-if00
```

Use separate terminals for simultaneous monitoring. Once programmed, USB is
only needed for power/logging: a power-only cable can run the knob, but cannot
flash it. There are no PC media-key commands or motor actuation in this test.

The shared 32-byte versioned packet and link code live in
`include/esp_now_link.h`. Peers are fixed to the two boards' station MACs:
knob `40:4C:CA:5C:39:30`, display `80:B5:4E:C7:F3:DC`. Change those constants if
replacing a board. Packets are unencrypted for this bench test, with source MAC,
role, version, and length checks. Boot session IDs prevent stale echoes from
claiming a new connection. Wi-Fi callbacks only enqueue packets; the main loop
handles sends, telemetry, logging, and drawing.

Hardware validation on 2026-10-26 passed on both boards: bidirectional round
trips, disconnect detection while each peer was paused in download mode, and
automatic reconnection after either peer rebooted. Serial evidence is saved in
`backups/espnow-validation-20261026.txt`. Both PlatformIO builds passed, and the
code-reviewer subagent reported no actionable findings.

## Knob encoder diagnostics

Connect only the T-Knob using a USB data cable. In the PlatformIO terminal:

```sh
pio run -e knob_test
pio device list
pio run -e knob_test -t upload --upload-port /dev/ttyACM0
pio device monitor -e knob_test --port /dev/ttyACM0
```

Replace `/dev/ttyACM0` with the knob's actual port (Windows: `COMx`; macOS:
`/dev/cu.usbmodem...`). If `pio` is missing from your shell, use PlatformIO's
terminal or `~/.platformio/penv/bin/pio`. The first build downloads the toolchain.
In VS Code, the same operations are under **PlatformIO > Project Tasks >
knob_test > Build / Upload / Monitor**. Close the monitor before uploading again.

Uploading replaces the existing firmware. The checked-out vendor examples remain
available for rebuilding motor demos later.

The connected knob's original 4 MB flash was saved to
`backups/t-knob-original.bin` before loading this test. Keep that ignored backup
if you want to restore its previous firmware. From this directory, using the
PlatformIO Python environment with esptool installed:

```sh
~/.platformio/penv/bin/python -m esptool --chip esp32c6 --port /dev/ttyACM0 write_flash 0x0 backups/t-knob-original.bin
```

Use the same knob and its current port for restoration.

Turn the knob by hand. Output shows absolute angle (0–360 degrees), relative
movement since startup, signed logical steps (24 per revolution), and valid/error
read counts. A status line appears every second even at rest. Angle wraps at one
revolution; relative movement and steps continue across the boundary. The sign
follows the encoder; map it to volume up/down after checking your preferred
direction. Press/release **BOOT** after startup to see button events; this tests
the board button, not a promised shaft-click switch.

The test holds all motor driver inputs LOW. The knob should turn freely: this
tests the MT6701 encoder and USB, **not motor actuation or haptic detents**.
`steps` are software counts. A growing `ok` count with few/no `errors` is expected;
continuous errors or a fixed angle while turning require checking the hardware.
After a long gap in valid data, movement across that gap can be ambiguous; this
is a bench test, not a calibrated position controller.

If upload cannot connect, hold BOOT, tap RESET, release BOOT, then retry. If the
upload succeeds but output is absent, tap RESET and reopen the monitor at 115200.
For Linux permission errors, configure serial access for your user and log in
again; avoid running PlatformIO as root.

## Layout

```text
te_keyboard/
  platformio.ini             # Separate targets/toolchains for the C6 and S3
  firmware/
    knob/                   # Encoder + ESP-NOW link test; motor disabled
    display/                # ESP-NOW status + live knob telemetry on screen
  include/esp_now_link.h     # Shared packet, fixed peers, heartbeat/echo test
  T-Knob/                   # Existing LILYGO reference checkout
  T-Display-Bar/            # Existing LILYGO reference checkout + patched TFT
```

The reference checkouts remain in place. The display target uses the vendor's
custom board definition and local TFT_eSPI fork, including its 76 x 284 panel
offsets. Keep both checkouts next to `platformio.ini`. The knob uses a pinned
pioarduino 53.03.13 platform with Arduino 3.1.3 for ESP32-C6 support; the display stays on
the vendor's PlatformIO espressif32 6.5.0 / Arduino 2.0.14 stack for compatibility.
The generic C6 board profile is overridden to match the T-Knob's **4 MB flash**.

This directory is not currently a Git repository. For version control, make the
application the top-level repository and record the two vendor repositories as
submodules at their current commits, preserving any local changes first.

## Display upload by port number

Connect the bar, identify its port separately, then run:

```sh
pio run -e display_test
pio run -e display_test -t upload --upload-port YOUR_BAR_PORT
pio device monitor -e display_test --port YOUR_BAR_PORT
```

The screen shows the ESP-NOW link status and live knob readings described above.

## System and next steps

```text
T-Knob (ESP32-C6) <-- ESP-NOW --> T-Display-Bar (ESP32-S3) <-- USB --> PC
 encoder + haptics                display + USB media keys          optional helper
```

Use the bar as the PC-facing controller. The C6's USB is fixed-function
serial/JTAG and cannot implement a USB HID media keyboard. The S3 supports USB
device/HID, so it can send volume up/down/mute commands without a PC helper.
ESP-NOW is a practical first link between the two boards without extra wiring
or a router; both must use the same Wi-Fi channel. If you prefer a wired link,
UART is an alternative after verifying exposed, unused pins on both schematics.
The USB-C ports do not directly connect these two boards as peers.

The encoder, screen, and bidirectional ESP-NOW heartbeat test are implemented.
Build the remaining pieces in this order:

1. Extend the bench packet into an application protocol for controller state
   and discrete button events. Current heartbeats carry cumulative steps and
   button state; a brief press between packets can be missed.
2. Add USB consumer-control HID on the bar for volume/mute. This needs an
   application target with `ARDUINO_USB_MODE=0` (TinyUSB), rather than the hardware
   CDC/JTAG mode used by the current screen test.
3. Add a PC helper over USB CDC if the screen should show the **actual** system
   volume, active app, track title, etc. HID volume keys alone do not report the
   resulting OS volume back to the device. Choose the helper implementation for
   your PC's operating system.
4. Add motor control and haptic detents on the knob after the event path works.
   Keep the fast motor-control loop separate from logging, radio and rendering.

## References

- [LILYGO T-Knob](https://github.com/Xinyuan-LilyGO/T-Knob): pin map and MT6701 SPI/CRC reference.
- [LILYGO T-Display-Bar](https://github.com/Xinyuan-LilyGO/T-Display-Bar): board definition and patched display library.
- [ESP32-C6 USB limitations](https://docs.espressif.com/projects/esp-idf/en/stable/esp32c6/api-guides/usb-serial-jtag-console.html).
- [Pinned pioarduino release](https://github.com/pioarduino/platform-espressif32/releases/tag/53.03.13).
