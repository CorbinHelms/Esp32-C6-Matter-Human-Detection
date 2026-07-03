# Vendored dependency patches (M2 bring-up)

`Cargo.toml` `[patch.crates-io]` overrides `openthread` 0.2.0,
`openthread-sys` 0.2.1 and `esp-radio` 0.18.0 with local copies in `vendor/`
(not committed — run `scripts/vendor.sh` to regenerate them from the cargo
registry cache plus these diffs).

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

## esp-radio-154-diag.patch

ISR-safe atomic diagnostic counters for the 802.15.4 driver
(`esp_radio::ieee802154::diag::snapshot()`): SFD/RxDone/queue events, ACK
branch decisions, TX outcomes, RX/TX abort reasons, RX re-arms. Never logs
from the ISR (ISR logging over the blocking USB-Serial-JTAG logger stalls
the executor — the original `esp_radio=off` lesson). `srp-probe` prints
deltas every 10s. Purely diagnostic; keep or drop.

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
