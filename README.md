# ESP32-C6 mmWave Human Detection → Google Home (Matter over Thread)

Baremetal (`no_std`) Rust firmware for a **Seeed Studio XIAO ESP32-C6** driving a
**24GHz mmWave Human Static Presence sensor**, exposing presence to **Google Home** as a
**Matter Occupancy Sensor over Thread** so it can drive automations directly in the Home app.

> Status: **Feature-complete (builds)** — the full firmware (mmWave sensor → Matter Occupancy
> Sensor over Thread, with flash-backed fabric persistence and a BOOT-pin factory reset) compiles
> for the RISC-V target. On-hardware bring-up (flashing, sensor validation, Google Home
> commissioning) is the user's step — see [Verification](#verification) and the step-by-step
> [`CLAUDE.md`](CLAUDE.md) bring-up guide. See [Roadmap](#roadmap).

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
cargo build --release   # compile for riscv32imac-unknown-none-elf
cargo run --release     # flash (with partitions.csv) + monitor — uses espflash as the runner
```

The `cargo run` runner flashes with the project's [`partitions.csv`](partitions.csv): a 5 MB app
partition (the Matter firmware is ~1.7 MB — it does **not** fit the default 1 MB partition) plus a
256 KB NVS partition for the persisted Matter fabric.

On boot the LED blinks, the monitor prints the boot banner, the QR/pairing code, and — once the
sensor is wired — live `presence -> OCCUPIED/vacant` transitions.

## Project layout

```
src/
  bin/main.rs       # entry point: runtime, heap, task spawn, runs Matter
  lib.rs            # library root / module map
  board.rs          # XIAO ESP32-C6 pin map (the non-linear D-label table)
  presence.rs       # hold-time debouncer (pure logic)
  sensor.rs         # async LD2410 driver + shared PresenceState
  matter/
    mod.rs          # stack assembly, node, run loop, flash persistence, factory reset
    occupancy.rs    # OccupancySensing (0x0406) handler + Occupancy Sensor (0x0107)
partitions.csv      # flash layout: 5 MB app + 256 KB NVS (Matter fabric)
.cargo/config.toml  # target, espflash runner (+partition table), linker args, build-std
build.rs            # linker script wiring (linkall.x)
rust-toolchain.toml # stable + rust-src + riscv32imac target
```

The Matter stack (`rs-matter` + `rs-matter-embassy` + `openthread`) is a pre-1.0 ecosystem;
`rs-matter-embassy` has no crates.io release, so `Cargo.toml` pins it to an exact git commit.
A normal `cargo build` fetches it automatically.

## Commissioning into Google Home

This uses **test** Device Attestation (VID `0xFFF1`, PID `0x8001`), which is dev-only (no OTA,
not for shipping products) but is accepted by Google Home for development.

1. **Thread Border Router:** ensure a Google Thread Border Router is on your network
   (Nest Hub 2nd gen, Nest Hub Max, Nest Wifi Pro, or Google TV Streamer 4K).
2. **Developer Console:** in the [Google Home Developer Console](https://console.home.google.com),
   create a project, add a **Matter** integration, and register the test **VID `0xFFF1` / PID `0x8001`**.
3. **Flash & monitor** the firmware (`cargo run --release`). At startup the stack prints the
   **QR code and 11-digit manual pairing code** over the USB-Serial-JTAG log (default test
   passcode `20202021`, discriminator `3840`).
4. **Pair:** in the Google Home app → **Add → Matter-enabled device** → scan the QR code (or
   enter the manual code), and choose your Thread network when prompted. Commissioning runs over
   BLE, then the device joins Thread.
5. **Automate:** the device appears as an **Occupancy Sensor**. Use it as an automation starter
   (`device.state.OccupancySensing`, `is: OCCUPIED`/`UNOCCUPIED`) in the Home app or script editor.

> Cross-check the cluster directly with `chip-tool occupancysensing read occupancy <node> 1`.

The Matter fabric is **persisted to flash**, so the device stays commissioned across reboots. To
unpair and re-commission, **factory reset**: hold the **BOOT button (GPIO9)** low for 3+ seconds —
the firmware clears the stored fabric and reboots.

## Verification

The firmware is developed in a cloud container with no hardware attached, so "verified" here
means **builds clean for `riscv32imac-unknown-none-elf`**. On-hardware verification is the
user's step, per milestone:

- **M0/M1:** flash; the LED blinks and the monitor prints boot + heartbeat; with the sensor
  wired, walking in/out *and sitting still* logs `presence -> OCCUPIED/vacant`.
- **M2:** commission into Google Home (above); the Occupancy Sensor reflects presence and works
  as an automation starter; cross-check `0x0406/Occupancy` with `chip-tool`.

## Roadmap

- **M0 — Scaffold + bring-up** ✅ builds for target; blink + heartbeat log.
- **M1 — Sensor** ✅ LD2410 over async UART → debounced presence (`PresenceState`).
- **M2 — Matter Occupancy Sensor over Thread** ✅ (builds) `rs-matter` + `rs-matter-embassy`,
  OccupancySensing (0x0406) endpoint (device type 0x0107), BLE commissioning, Thread transport;
  commissions into Google Home (test VID `0xFFF1`).
- **M3 — Persistence + hardening** ✅ (builds) flash-backed fabric persistence
  (`SeqMapKvBlobStore` over an NVS partition), BOOT-pin factory reset, partition table.
  Occupancy-flicker tuning (LD2410 gate sensitivity + hold time) and SRAM-headroom soak testing
  are on-hardware follow-ups.

## License

MIT (see `Cargo.toml`).
