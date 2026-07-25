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

## Status (on-hardware bring-up — updated 2026-07-03 evening: **PAIRED & WORKING in Google Home**)

| Stage | Code | Built | Hardware-verified |
|------|------|-------|-------------------|
| M0 scaffold + blink/log | `src/bin/main.rs`, `src/board.rs` | ✅ | ✅ |
| M1 LD2410 sensor + debounce | `src/sensor.rs`, `src/presence.rs` | ✅ | ✅ full presence cycle → occupancy attr |
| M2 Matter Occupancy over Thread | `src/matter/{mod,occupancy}.rs` | ✅ | ✅ commissioned into Google Home 2026-07-03 18:55 (three root causes fixed — see below) |
| M3 persistence + factory reset | `src/matter/mod.rs`, `partitions.csv` | ✅ | ✅ reboot → "already commissioned", rejoins as the same SRP host in ~4s; factory reset exercised repeatedly during re-pairing cycles |

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

### SOLVED #2: the FCF offset off-by-one (commit 298ddc3) — MAC retries never ran

The 2026-07-03 overnight soak (313 cycles) measured **25% SRP round-trip success**
(4,982 sends → 1,254 responses) with `ackrx` pinned at 0, and disproved the OtMdns-churn
hypothesis (churn phase D *out-performed* plain phase C, 88% vs 82%). The cause:
`vendor/esp-radio/.../frame.rs` kept the C driver's length-prefixed buffer offsets while
every Rust call site passes the PSDU with the length byte stripped — so `frame_is_ack_required`
read the frame-version bit instead of the AR bit (false for every 2006 frame we send) and
`frame_get_version` read the sequence number. **The driver never armed the ACK-wait, OpenThread
was told every TX succeeded, and 802.15.4 MAC retransmission never ran at all** — every
fragment of every message had to land first-shot. Fix: `FRAME_AR_OFFSET` 1→0,
`FRAME_VERSION_OFFSET` 2→1 (see `patches/esp-radio-154-fixes.patch`).

Post-fix hardware verification: `ackrx` counts hardware-confirmed ACKs (first time ever), the
`rxab`/`txab` abort lockstep is gone, and **all probe phases (incl. full Matter payload and
churn) register in 0.2–1.4s** — from 8–21s averages with 2–18% timeouts. 50-min validation
soak at 90s deadlines (`.bringup/offset-fix-soak.log`): **1,119 registrations, 0 failures,
median 462ms, p99 741ms; 1,676/1,677 updates answered (99.94% vs 25% baseline).**

Superseded (do not implement unless a new failure mode appears): OtMdns churn debounce,
SRP retry densification, two-tier interrupt executors.

Notes for pairing attempts: Google's pre-flow probe always arms the fail-safe 120s →
re-arms 1s → disarms → reconnects for the real flow (not a failure). After a *failed* attempt
the stack stays in the Thread phase (no BLE re-advertising) with the PASE window eventually
expiring — reboot the device before any new pairing attempt. During attempts 2–3 Google
established PASE **over the operational Thread network** (session ID 3 in the logs), so
operational IP reachability was already proven pre-fix.

### SOLVED #3: Google's CASE arrived after its own fail-safe (commit d7c4509) — M2 COMPLETE

With the radio fixed, re-pairing still failed 3/3 with a Matter-layer signature
(`.bringup/attempt-{console,consoletest,quiet}.log`): fabric added + Thread joined + SRP
registered ~10s into Google's 120s fail-safe, then Google's CASE Sigma1 landed at **expiry
±1s** — moments after the fabric rollback — dying with "Fabric Index mismatch"
(NoSharedTrustRoots). Best-evidence cause: controller-side stale DNS-SD cache — the host name
is stable across attempts (f77f983/d6e0bf5) but the SLAAC address rotates per boot, and Google
re-resolved only at record-TTL expiry (≈120s).

Fix (both dev-only aids; rs-matter 0.2.0 is now the 4th vendored crate,
`patches/rs-matter-failsafe-grace.patch`):
1. **One-shot 180s fail-safe grace** when the timeout hits with `AddNOC` processed under it
   (a real commissioning in flight; Google's pre-flow 120s/1s probe has no NOC and expires
   normally) — a late CASE + CommissioningComplete land on a live fabric.
2. **`Transport::notify_mdns_changed()` made pub** + the firmware re-announces SRP/mDNS every
   15s × 64 from boot (≈ the pairing window), so controller caches converge early.

Outcome: paired 2026-07-03 18:55 in ~15s flat (AddNOC → CommissioningComplete; the grace never
fired — clean caches + re-announces made Google fast). Reboot test 18:58: "already
commissioned", same SRP host re-registered in ~4s, Google controllers talking to the new
address within a minute.

**Pairing protocol (updated):** reboot the device (fresh 15-min window), remove any
stale/offline entry of the device from Google Home first, and **don't cancel the app before
~5 min** — the grace period means slow attempts complete late rather than fail.

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

## Thread repeater bring-up (2026-07-23 — RADIO ROOT CAUSES FIXED, field test pending)

A second C6 (MAC `58:e6:c5:13:4a:9c`, U.FL antenna attached) runs `src/bin/repeater.rs`
(`--features ftd`, see file header for build/flash + LED codes). The long "sensor 2 never
attaches through it / repeater-as-Router forwards nothing" saga ended 2026-07-23 evening with
**two stacked vendored-stack bugs that zeroed every link margin OpenThread ever computed**
(commit 861fc62, found via the telemetry v3 remote stream, commit 1194847):
1. EspRadio glue read per-frame **RSSI one byte past the frame** (fixed-size buffer, guard
   always passed) → garbage RSS (0/-38/-113). Real slot: first FCS byte, `data[1..][len-2]`.
2. `otPlatRadioGetReceiveSensitivity` was a **TODO returning 0 dBm** → margin = RSS − 0 < 0 →
   LinkQualityIn 0 for every neighbor. Now −104 dBm (C6 datasheet).
Either alone pins lq_in at 0: router↔router links sit at cost ∞ (`nbr{lq[i/o]:0/3 cost:16}`),
so the moment the repeater was promoted to Router, MeshForwarder dropped every off-link
unicast with `NoRoute` — a Router-shaped black hole. Children were unaffected (they only send
to their parent), which is why sensors worked as children and everything died at promotion.
Bench-verified post-fix: `lq[i/o]:3/3 cost:1` to the Nest, multi-hop routes appear, 63
sends/0 drops as Router, telemetry streams continuously.

Telemetry (v3, working): sinks are discovered at runtime — the Nest publishes its OWN NAT64
/96 (`fd6b:588a:7d6a:2::/96`, NOT well-known `64:ff9b::`), so the PC sink is that prefix +
PC IPv4; sinks not covered by a published route are disabled (send() reports Ok even for
frames forwarding later drops). All MeshForwarder/route-error lines are console-only (queue
feedback at ratio 1.0 melted v2); >300 batches/10 s trips a breaker. Listen with a UDP :9999
socket; heartbeat `tele hb sinks ok=,err=` every 10 s.

**OUTCOME (2026-07-25): system fully deployed and working.** Sensor 2 sits at the far
wall spot attached THROUGH the repeater (its first child, RLOC 0x9c01, LED double-dip) and
is active in Google Home end-to-end; sensor 1 unchanged. The 7/23 silent crash-loop did not
recur on the final firmware (4 h PC + wall soak, 343+ announce sweeps exercised) — believed
a storm-era artifact, NOT fully root-caused. If the repeater ever flaps again (double-dip
gone / sensor 2 offline): plug it into the PC and run `scripts/serial_capture.py` in a
respawn loop (plain stty/cat hangs forever in open() on this cdc-acm) to catch the next
boot's ROM `rst:0x...` reset reason. Google may ignore a freshly-commissioned device for
HOURS (observed 15 h) — device-side health shows as SRP re-registrations with zero inbound
CASE; opening the Google Home app can prod it. Board quirks: the repeater board often
(not always) parks in the ROM bootloader after espflash contact until a physical replug;
pairing attempts require power-cycling the sensor first (BLE doesn't re-advertise after a
failure). Full history: session memory `thread-repeater` + git log 7d99e09..1194847.

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
- Matter identity (`src/matter/mod.rs`): VID `0xFFF1`, PID `0x8001`; passcode/discriminator default
  to the test values (`20202021`/`3840`) but are overridable at build time via `MATTER_PASSCODE` /
  `MATTER_DISCRIMINATOR` env vars — `scripts/provision.py` uses that to flash per-unit codes and
  generate a per-unit pamphlet (`hardware/units/`). **Dev-only** (no OTA, not shippable).
- `EXTERNAL_ANTENNA=1` at build time switches the XIAO's RF switch to the U.FL connector at boot
  (`board::select_antenna`, GPIO3/GPIO14) — per-unit opt-in for boards with an antenna attached
  (`--external-antenna` on provision.py / flash_repeater.py, or the GUI checkbox).
- Tuning knobs: `HOLD_MS` (`src/sensor.rs`), `HEAP_SIZE` (`src/bin/main.rs`), `BUMP_SIZE`
  (`src/matter/mod.rs`), `partitions.csv` (5 MB app + 256 KB NVS).
- `rs-matter-embassy` is git-pinned in `Cargo.toml` to rev `efef8b70b64178a8f8d1460d02ebb6fa146d2d95`
  (no crates.io release; this whole Matter/Thread Rust stack is pre-1.0 — expect churn if you bump it).

## File map

```
src/bin/main.rs     runtime + heap setup, spawns LED + sensor tasks, runs Matter
src/bin/repeater.rs Thread range extender: joins as FTD → auto-promoted Router
                    (build with --features ftd + THREAD_DATASET_HEX; no Matter)
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
