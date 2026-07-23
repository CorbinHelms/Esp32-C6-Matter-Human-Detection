# Vendored dependency patches (M2 bring-up)

`Cargo.toml` `[patch.crates-io]` overrides `openthread` 0.2.0,
`openthread-sys` 0.2.1, `esp-radio` 0.18.0 and `rs-matter` 0.2.0 with local
copies in `vendor/` (not committed — run `scripts/vendor.sh` to regenerate
them from the cargo registry cache plus these diffs).

## openthread-esp-radio-fixes.patch (the M2 fix)

Three changes to `EspRadio` (openthread crate `src/esp.rs`), all validated on
hardware with `src/bin/srp-probe.rs` against the real Nest border router:

1. **`enhance_ack_tx: false`** (was `true`) — **the root-cause fix.**
   esp-radio's enhanced-ACK support is explicitly incomplete ("TODO: full
   enh-ack support"); with it enabled the C6 radio never captured the
   parent's imm-ACKs (every acked TX aborted with `RX_ACK_TIMEOUT`) and
   outgoing auto-ACKs were stopped mid-flight (`TX_ACK_STOP`). Net effect:
   ~90% loss of prompt unicast responses (SRP replies, and CASE Sigmas would
   be next), multi-round MLE attach, `OT_ERROR_RESPONSE_TIMEOUT` loop —
   the M2 blocker. Thread on a non-CSL MED only needs 2006 imm-ACKs.
2. **10ms bounded wait in `receive()`** — esp-radio has a known silent
   RX-stall condition (`ensure_receive_enabled`'s FIXME) whose self-heal
   only runs when the driver API is polled; a bare signal wait never polls
   again. The bounded wait re-arms a stalled receiver within one MAC-retry
   window.
3. **RX queue depth 50 → 200 frames** (~26KB heap) — executor stalls
   (mbedtls ECDSA, blocking flash writes) let bursts overflow a 50-deep
   queue (`Receive queue full` drops). Mirrors esp-rs/openthread#92 /
   sysgrok/rs-matter-embassy PR#62.

Probe measurements (full Matter-sized 2-service SRP registration): before —
1 success in 15+ attempts over ~20 min; after — registers in 0.6–37s,
typically <15s, across repeated cycles.

## esp-radio-154-fixes.patch

1. **`frame.rs` FCF offset off-by-one — the second root-cause fix.**
   `frame_is_ack_required` / `frame_get_version` kept the C driver's
   length-prefixed buffer offsets (`[len][FCF0][FCF1]…`), but every Rust
   call site passes the PSDU with the length byte stripped. "ACK required"
   therefore read FCF byte 1 bit 0x20 — the frame-version-high bit — which
   is false for every 2006 frame the device transmits, so the driver never
   armed the ACK-wait and OpenThread believed every TX succeeded:
   **802.15.4 MAC retransmission never ran at all.** Measured impact: 25%
   SRP round-trip success (4,982 sends → 1,254 responses overnight);
   multi-fragment updates only survived if every fragment landed first-shot.
   ("Frame version" likewise read the sequence number, causing the garbage
   RX ack-branch decisions that made enhanced-ACK mode catastrophic.)
   Fix: offsets 1→0 and 2→1 (PSDU-relative). Verified on hardware: `ackrx`
   counts hardware-confirmed ACKs (previously always 0), the RX/TX abort
   lockstep is gone, and SRP registrations complete in ~1.4s first-try.

2. ISR-safe atomic diagnostic counters
   (`esp_radio::ieee802154::diag::snapshot()`): SFD/RxDone/queue events, ACK
   branch decisions, TX outcomes, RX/TX abort reasons, RX re-arms. Never
   logs from the ISR (ISR logging over the blocking USB-Serial-JTAG logger
   stalls the executor — the original `esp_radio=off` lesson). `srp-probe`
   prints deltas every 10s. Purely diagnostic.

## rs-matter-failsafe-grace.patch

Two commissioning-window aids for `rs-matter` 0.2.0, both dev-only (drop once
Google's controller reliably completes inside its own window):

1. **One-shot fail-safe grace extension** (`failsafe.rs`): if the fail-safe
   times out while `AddNOC` was processed under it (a real commissioning in
   flight — not Google's pre-flow 120s/1s arm-disarm probe), re-arm once for
   180s instead of expiring. Observed on hardware (3/3 attempts,
   `.bringup/attempt-{console,consoletest,quiet}.log`): Thread join + SRP
   registration complete ~10s into Google's 120s fail-safe, but the
   controller's CASE Sigma1 arrives at expiry ±1s — moments after the fabric
   rollback — and dies with NoSharedTrustRoots ("Fabric Index mismatch").
   The grace keeps the staged fabric alive so the late CASE +
   CommissioningComplete can land. Spec-wise the fail-safe must expire at
   the requested time; this trades that for pairing reliability on a
   test-VID device.

2. **`Transport::notify_mdns_changed()` made `pub`** (`transport.rs`) so the
   firmware can re-trigger SRP/mDNS re-announcement every 15s during the
   pairing window. The expiry-±1s CASE timing matches a controller resolving
   a stale cached host AAAA (the host name is stable across attempts but the
   Thread SLAAC address rotates per boot) until record-TTL expiry (120s);
   re-announcing pushes fresh records so controller caches converge early.

## openthread-sys-log-level.patch

Compiles the OpenThread C library with `OT_LOG_LEVEL=INFO` (was `NOTE`) so
the SRP client / MLE / MAC internals are observable during bring-up. The
crate's `otPlatLog` shim maps OT INFO onto `log::debug!`, so visibility is
further gated by `ESP_LOG="...,openthread=debug"` in `.cargo/config.toml`.
Revert to `NOTE` (or drop the ESP_LOG clause) once M2 is verified — OT
formats INFO strings unconditionally, which costs some CPU.

The script also deletes `vendor/openthread-sys/libs/` and the pre-generated
riscv32imac bindings, forcing the on-the-fly OpenThread C build (needs
`cmake` + `clang`) — otherwise the prebuilt archives would silently ignore
the `OT_LOG_LEVEL` change.

## openthread-become-router.patch

Adds `OpenThread::become_router()` (gated on the `ftd` feature) wrapping
`otThreadBecomeRouter`, so the repeater firmware can solicit a router ID
immediately instead of waiting on the REED self-upgrade's randomized jitter —
observed taking 10+ minutes (or stalling) against the Nest-led network.
