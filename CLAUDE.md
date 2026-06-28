# Project context & hardware bring-up (read me first)

This file is the handoff for continuing on a machine **with the physical hardware attached**.
The firmware was written and compile-verified in a cloud container with **no hardware**, so every
milestone is "builds clean for the target" but **not yet run on metal**. Your job here is the
on-hardware bring-up. Work top-to-bottom; each stage has a clear success check and fallbacks.

## What this is

Baremetal (`no_std`) Rust firmware for a **Seeed XIAO ESP32-C6** + a **24GHz mmWave Human Static
Presence sensor (HLK-LD2410)**, exposing presence to **Google Home** as a **Matter Occupancy
Sensor over Thread**, for automations. Architecture and rationale are in [`README.md`](README.md);
the design record is the approved plan referenced in the git history.

## Status (all built green, none hardware-verified)

| Stage | Code | Built | Hardware-verified |
|------|------|-------|-------------------|
| M0 scaffold + blink/log | `src/bin/main.rs`, `src/board.rs` | ✅ | ⬜ you |
| M1 LD2410 sensor + debounce | `src/sensor.rs`, `src/presence.rs` | ✅ | ⬜ you |
| M2 Matter Occupancy over Thread | `src/matter/{mod,occupancy}.rs` | ✅ | ⬜ you |
| M3 persistence + factory reset | `src/matter/mod.rs`, `partitions.csv` | ✅ | ⬜ you |

The Matter subscription path (does occupancy reach Google Home?) was **adversarially review-verified
against the rs-matter source** — it is correct by construction — but still needs a live confirmation.

## Toolchain (on your PC — no special proxy/env needed)

The ESP32-C6 is RISC-V, so this builds on **stable** Rust — no `espup`, no nightly.

```bash
rustup toolchain install stable --component rust-src
rustup target add riscv32imac-unknown-none-elf
cargo install espflash
```

> Note: the cloud build needed `CARGO_NET_GIT_FETCH_WITH_CLI=1` + git-config overrides to fetch the
> git-pinned `rs-matter-embassy` through a scoped proxy. **On your PC none of that applies** — a
> plain `cargo build --release` fetches it normally. First build compiles the OpenThread C stack and
> takes a few minutes.

## Build / flash / monitor

```bash
cargo build --release        # verify it still builds on your machine
cargo run --release          # flash (with partitions.csv) + serial monitor
```

`cargo run`'s runner is `espflash flash --monitor --chip esp32c6 --partition-table partitions.csv`
(see `.cargo/config.toml`). Logs come out over **USB-Serial-JTAG** (the same USB-C port), so no
extra adapter is needed.

## On-hardware bring-up plan

### Stage M0 — first flash (no sensor needed)
1. Connect the XIAO ESP32-C6 by USB-C, `cargo run --release`.
2. **Success:** builtin LED (GPIO15) blinks ~1 Hz; monitor prints
   `esp32-c6 human-detection firmware booted ...`.
   - The M2/M3 firmware also immediately tries to bring up Thread/BLE and prints a QR/pairing code —
     that's expected even before the sensor is wired.
3. If nothing prints: confirm the port, and that the board enumerated as USB-Serial-JTAG. `ESP_LOG`
   defaults to `info` (set in `.cargo/config.toml`).

### Stage M1 — confirm the sensor (the most important hardware check)
**Wiring** — the XIAO silkscreen labels are NOT the GPIO numbers (single source of truth:
`src/board.rs`). The 24GHz-mmWave-for-XIAO board mounts in the XIAO footprint; serial is on D2/D3:
- sensor **TX → D2 (GPIO2)** = ESP RX
- sensor **RX ← D3 (GPIO21)** = ESP TX
- 8N1 @ **256000 baud**; sensor power per the Seeed wiki (5V board in, 3.3V logic).

1. With the sensor wired, `cargo run --release` and watch the monitor.
2. **Success:** walking in/out *and sitting still* logs `presence -> OCCUPIED` / `presence -> vacant`
   with stationary/moving distance + energy.
3. **If you see repeated `sensor frame error: ... (resyncing)`:** the bytes don't match the LD2410
   framing. Two likely causes:
   - **Wrong baud / wiring** — double-check D2/D3 and 256000.
   - **Different radar IC** — a few board revisions ship an **Iclegend S3KM1110** (incompatible
     protocol). Confirm your IC (BLE name `HLK-LD2410_xxxx` ⇒ LD2410). If it's the Iclegend part,
     switch to the **digital presence GPIO fallback**: the LD2410 driver's parser won't match, so
     wire the module's OUT pad to `board::SENSOR_PRESENCE_GPIO` (D1/GPIO1) and drive presence from
     an `Input` edge instead of the UART. (Ask Claude Code to add the GPIO-presence path in
     `src/sensor.rs`; the hook/const is already there.)
4. **Tuning:** `HOLD_MS` in `src/sensor.rs` (default 2000 ms) is the absence debounce on top of the
   LD2410's own delay. Increase it if presence flickers; the LD2410 gate sensitivity / max distance
   can also be set over UART (config-frame commands) — not yet implemented, add if needed.

### Stage M2 — commission into Google Home
**Prerequisite:** a **Thread Border Router** on your network (Nest Hub 2nd gen, Nest Hub Max, Nest
Wifi Pro, or Google TV Streamer 4K).

1. In the [Google Home Developer Console](https://console.home.google.com): create a project, add a
   **Matter** integration, and register the **test VID `0xFFF1` / PID `0x8001`** (these are baked in
   via `TEST_DEV_*` in `src/matter/mod.rs`).
2. Flash + monitor; copy the **QR code / 11-digit manual pairing code** the stack prints at boot
   (default test passcode `20202021`, discriminator `3840`).
3. Google Home app → **Add → Matter-enabled device** → scan QR (or enter the manual code) → choose
   your Thread network. Commissioning runs over BLE, then the device joins Thread.
4. **Success:** the device shows up as an **Occupancy Sensor**; presence toggles it; it's available
   as an automation starter (`device.state.OccupancySensing`, `is: OCCUPIED`/`UNOCCUPIED`).
5. **Independent cross-check** with the Matter SDK controller:
   `chip-tool occupancysensing read occupancy <node-id> 1` and `subscribe` to watch live changes —
   this isolates "firmware exposes occupancy correctly" from any Google Home UI quirk.

### Stage M3 — persistence, factory reset, hardening
- The fabric is persisted to the NVS flash partition (`partitions.csv`), so the device **stays
  commissioned across reboots**. Verify: power-cycle after pairing; it should reconnect without
  re-commissioning.
- **Factory reset:** hold the **BOOT button (GPIO9)** low for 3+ seconds → clears the fabric and
  reboots. Use this before re-pairing.
- **SRAM headroom:** the C6 has 512 KB SRAM; the Matter stack is memory-tight. If the stack panics
  during init for lack of memory, raise `HEAP_SIZE` (`src/bin/main.rs`) or `BUMP_SIZE`
  (`src/matter/mod.rs`). Soak-test under sustained radio load.

## Key facts / knobs (so you don't have to re-derive them)

- Target: `riscv32imac-unknown-none-elf`, stable Rust. Flashed image ≈ 1.7 MB.
- Pins (`src/board.rs`): LED `GPIO15`; sensor UART1 RX `GPIO2`/TX `GPIO21` @ 256000 8N1; presence
  fallback `GPIO1`; BOOT/factory-reset `GPIO9`.
- Matter identity (`src/matter/mod.rs`): VID `0xFFF1`, PID `0x8001`, discriminator `3840`, passcode
  `20202021`. **Dev-only** (no OTA, not shippable).
- Tuning knobs: `HOLD_MS` (`src/sensor.rs`), `HEAP_SIZE` (`src/bin/main.rs`), `BUMP_SIZE`
  (`src/matter/mod.rs`), `partitions.csv` (5 MB app + 256 KB NVS).
- `rs-matter-embassy` is git-pinned in `Cargo.toml` to rev `efef8b70b64178a8f8d1460d02ebb6fa146d2d95`
  (no crates.io release; this whole Matter/Thread Rust stack is pre-1.0 — expect churn if you bump it).

## File map

```
src/bin/main.rs     runtime + heap setup, spawns LED + sensor tasks, runs Matter
src/board.rs        XIAO ESP32-C6 pin map (the non-linear D-label table)
src/presence.rs     hold-time debouncer (pure logic, has unit tests)
src/sensor.rs       async LD2410 UART driver + shared PresenceState
src/matter/mod.rs   EmbassyThreadMatterStack assembly, node, run loop, persistence, factory reset
src/matter/occupancy.rs  OccupancySensing (0x0406) handler + Occupancy Sensor (0x0107) device type
partitions.csv      flash layout (5 MB app, 256 KB NVS)
```

## Things most likely to bite, and the fix

- **No logs at all** → wrong serial port / not USB-Serial-JTAG; or `ESP_LOG` unset.
- **`sensor frame error` spam** → wrong baud/wiring, or the Iclegend IC (use the GPIO fallback).
- **Panic `no NVS partition found`** → flashed without `partitions.csv` (use `cargo run`, not a bare
  `espflash` without `--partition-table`).
- **Stack OOM panic at init** → raise `HEAP_SIZE` / `BUMP_SIZE`.
- **Can't commission** → no Thread Border Router on the LAN, VID/PID not registered in the Dev
  Console, or a stale prior fabric (factory-reset with BOOT, then re-pair).
- **Occupancy not updating in Google Home** → verify with `chip-tool` first; if `chip-tool` sees it
  but Google Home doesn't, it's an ecosystem/automation-config issue, not the firmware.

## Conventions

- Develop on branch `claude/matter-thread-human-detection-orr0n1` (or a new branch off it); commit
  per working change; keep `cargo build --release` and `cargo clippy --release` green before pushing.
