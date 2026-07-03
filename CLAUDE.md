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

## Status (on-hardware bring-up — updated 2026-07-03, late session)

| Stage | Code | Built | Hardware-verified |
|------|------|-------|-------------------|
| M0 scaffold + blink/log | `src/bin/main.rs`, `src/board.rs` | ✅ | ✅ |
| M1 LD2410 sensor + debounce | `src/sensor.rs`, `src/presence.rs` | ✅ | ✅ full presence cycle → occupancy attr |
| M2 Matter Occupancy over Thread | `src/matter/{mod,occupancy}.rs` | ✅ | 🟡 radio root-cause FIXED & probe-verified; commissioning reaches PASE-over-Thread but still fails at in-context SRP — see below |
| M3 persistence + factory reset | `src/matter/mod.rs`, `partitions.csv` | ✅ | ⬜ blocked by M2 |

## M2 state of play (READ THIS — supersedes all earlier M2 analysis)

### SOLVED: the radio-level root cause (commit e80a8d0)

The original "SRP `OT_ERROR_RESPONSE_TIMEOUT` forever" blocker was **esp-radio 0.18's
incomplete enhanced-ACK support**: with `EspRadio`'s `enhance_ack_tx: true` the C6 (a) never
captured the parent's imm-ACKs (every acked TX aborted `RX_ACK_TIMEOUT` after a 200ms deaf
window) and (b) had its own outgoing auto-ACKs stopped mid-flight (`TX_ACK_STOP`) — the peer's
MAC saw hardware ACKs so it never retransmitted, while frames died before software. ~90% loss
of prompt unicast responses, 5-round MLE attaches, 1-in-15 SRP registrations.

Fixes live in `vendor/` (regenerate with `scripts/vendor.sh`; diffs in `patches/`, full story
in `patches/README.md`): `enhance_ack_tx: false` + a 10ms bounded wait in `receive()` (esp-radio
has silent RX stalls whose self-heal only runs when polled) + RX queue 200. **Verified**: the
standalone probe registers full Matter-sized 2-service SRP in seconds (was: almost never).

Disproven along the way (do not re-chase): BLE/coex (BLE is fully deinited before the Thread
phase — `BleConnector::drop` → `ble_deinit`), rx-on-when-idle, short-address filter, RX-queue
depth alone, flash-write stalls during SRP.

### The probe — iterate WITHOUT a phone

`src/bin/srp-probe.rs` joins the real Nest network using the dataset TLVs captured from a
commissioning attempt (they print at info level: "Connecting to Thread network, dataset:")
and measures SRP registration reliability/latency for Matter-shaped payloads, plus replicates
failure hypotheses (phase D = OtMdns churn). Radio-level x-ray via ISR-safe counters
(`esp_radio::ieee802154::diag`, vendored). Build/flash:

```sh
THREAD_DATASET_HEX=$(cat .bringup/dataset.hex) cargo build --release --bin srp-probe
espflash flash --chip esp32c6 --port /dev/ttyACM0 --partition-table partitions.csv \
    target/riscv32imac-unknown-none-elf/release/srp-probe
```

`.bringup/` (gitignored) holds the dataset hex + all attempt logs. The device was left running
the probe overnight, logging to `.bringup/overnight-probe.log` — **check those stats first.**

### OPEN: commissioning still fails at SRP-in-main-firmware

Three Google Home attempts (2026-07-02/03). Attempts 2–3 (fixed radio): full BLE interview ✓,
AddNOC ✓, dataset delivered ✓, clean BLE→Thread handover ✓, 1-round attach ✓, **Google even
establishes PASE over the operational Thread network** (operational reachability works!) — but
the device's SRP registration times out through the ~120s fail-safe (code=28 per retry), Google
never CASEs, fail-safe expires, fabric rolls back. Meanwhile the probe on the same radio
registers fine — the remaining delta is above the radio. Leads, in order:

1. **OtMdns re-registration churn (prime suspect, probe phase D tests exactly this).**
   rs-matter-embassy's `OtMdns::run_register` does an *immediate* `srp_remove_all(true)` +
   re-register on every `wait_mdns()` notification — observed 2–3× within seconds of attach,
   each clearing the in-flight SRP transaction. If phase D shows churn wedges/starves the
   client → fix = debounce/dedupe in a vendored rs-matter-embassy (only re-register when the
   service set actually changed).
2. **Executor starvation** (partially addressed): PHY now runs on a Priority2 InterruptExecutor
   (`ProxyRadio`/`PhyRadioRunner`, commit 8e62236), but OT's tasklets/alarms still share the
   main executor with rs-matter's mbedtls (PASE/SPAKE2p grinds seconds per handshake; SRP
   client timers+response processing freeze meanwhile). Escalation if needed: second
   InterruptExecutor tier — PHY@P3(SWI2), whole `ot.run()`@P2(SWI1), rs-matter on thread.
   (Careful: OT tasklets include SRP ECDSA signing — measure before/after.)
3. **Ambient RF/server variance is real**: late-night probe runs needed 7–37s registrations
   (vs 0.2–13s earlier) with several retries. The fail-safe gives ~90s post-attach; SRP retry
   ladder only fits ~7 attempts. If 1+2 don't fully close it, densify retries (vendored
   openthread-sys: `OPENTHREAD_CONFIG_SRP_CLIENT_MIN_RETRY_WAIT_INTERVAL` 1800→500ms).

Also observed (don't be confused by it): Google's pre-flow probe always arms the fail-safe
120s → re-arms 1s → disarms → reconnects for the real flow. And after a failed attempt the
stack stays in the Thread phase (no BLE re-advertising) with the PASE window eventually
expiring — reboot the device before any new pairing attempt.

### Session/tooling notes (2026-07-03)

- OT C library now compiles at `OT_LOG_LEVEL=INFO` (vendored openthread-sys). Runtime
  visibility gated by ESP_LOG: `openthread=info` (NOTE-level, default now) vs
  `openthread=debug` (full SrpClient/MLE/MeshForwarder internals — very chatty; each line is
  a blocking USB write on the main executor, measurably harmful in hot paths).
- Never log from the radio ISR (the original `esp_radio=off` lesson) — use the diag counters.
- `pkill -f "espflash monitor"` from a compound command self-matches and kills your own shell
  (exit 144) — kill and relaunch in separate commands.
- espflash monitor is kept running via nohup writing to a log; grep that file rather than
  attaching interactively.

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
