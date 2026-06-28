# ESP32-C6 mmWave Human Detection → Google Home (Matter over Thread)

Baremetal (`no_std`) Rust firmware for a **Seeed Studio XIAO ESP32-C6** driving a
**24GHz mmWave Human Static Presence sensor**, exposing presence to **Google Home** as a
**Matter Occupancy Sensor over Thread** so it can drive automations directly in the Home app.

> Status: **M0 complete** — project scaffold builds for the RISC-V target and blinks +
> logs on hardware. Sensor (M1) and Matter/Thread (M2) layers are in progress. See
> [Roadmap](#roadmap).

## Hardware

| Part | Detail |
|------|--------|
| MCU board | Seeed Studio XIAO ESP32-C6 (RISC-V @160MHz, 8MB flash, 512KB SRAM, Wi-Fi 6 / BLE 5 / **802.15.4 Thread**) |
| Sensor | Seeed "24GHz mmWave Sensor for XIAO – Human Static Presence" (SKU 101010001), radar IC **Hi-Link HLK-LD2410** |
| Sensor link | UART, **8N1 @ 256000 baud**, 3.3V logic |
| Border router | A Thread Border Router on your network (Nest Hub 2nd gen / Hub Max / Nest Wifi Pro / Google TV Streamer) |

### Wiring

The XIAO silkscreen `D`-labels are **not** the GPIO numbers. Verified mapping (see
[`src/board.rs`](src/board.rs) for the full table). The mmWave board mounts in the XIAO
form factor and uses D2/D3 for serial:

| Signal | XIAO pin | GPIO |
|--------|----------|------|
| sensor TX → ESP RX | D2 | GPIO2 |
| ESP TX → sensor RX | D3 | GPIO21 |
| (optional) presence OUT | D1 | GPIO1 |
| builtin LED | — | GPIO15 |

Logs are emitted over **USB-Serial-JTAG** (shares the USB-C cable, consumes no GPIO), so the
UART pins stay free for the sensor.

> ⚠️ A few board revisions may ship an Iclegend S3KM1110 instead of the LD2410 (incompatible
> protocol). The sensor driver confirms the IC at bring-up and can fall back to the digital
> presence pin.

## Toolchain

The ESP32-C6 is RISC-V, so this builds on **stable** Rust — no `espup`, no nightly.

```bash
rustup toolchain install stable --component rust-src
rustup target add riscv32imac-unknown-none-elf
cargo install espflash   # for flashing/monitoring over USB
```

## Build, flash, monitor

```bash
cargo build --release                       # compile for riscv32imac-unknown-none-elf
cargo run --release                         # flash + monitor (uses espflash as the runner)
# or explicitly:
espflash flash --monitor --chip esp32c6 \
  target/riscv32imac-unknown-none-elf/release/esp32c6-matter-human-detection
```

Expected on M0: the builtin LED blinks at ~1 Hz and the monitor prints a boot line plus a
heartbeat every 5 seconds.

## Project layout

```
src/
  bin/main.rs   # entry point: esp-rtos/embassy runtime, task spawn
  lib.rs        # library root / module map
  board.rs      # XIAO ESP32-C6 pin map (the non-linear D-label table)
.cargo/config.toml  # target, espflash runner, linker args
build.rs            # linker script wiring (linkall.x)
rust-toolchain.toml # stable + rust-src + riscv32imac target
```

## Roadmap

- **M0 — Scaffold + bring-up** ✅ builds for target; blink + heartbeat log.
- **M1 — Sensor** — LD2410 over async UART → debounced presence `Signal`.
- **M2 — Matter Occupancy Sensor over Thread** — `rs-matter` + `rs-matter-embassy`,
  hand-built OccupancySensing (0x0406) endpoint (device type 0x0107), BLE commissioning,
  Thread transport; commission into Google Home (test VID `0xFFF1`).
- **M3 — Persistence + hardening** — flash-backed fabric persistence, occupancy-flicker
  tuning, SRAM budgeting.

## License

MIT (see `Cargo.toml`).
