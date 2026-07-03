# Vendored dependency patches (M2 bring-up)

`Cargo.toml` `[patch.crates-io]` overrides `openthread` 0.2.0 and
`openthread-sys` 0.2.1 with local copies in `vendor/` (not committed — run
`scripts/vendor.sh` to regenerate them from the cargo registry cache plus
these diffs).

## openthread-rx-queue.patch

Deepens the ESP 802.15.4 RX queue from 50 to 200 frames (~26KB heap).
On the single-core ESP32-C6, long executor stalls (mbedtls ECDSA for the SRP
client signature / CASE, blocking `esp-storage` flash writes) let normal
Thread traffic bursts overflow a 50-deep queue; the radio ISR then drops
frames (`Receive queue full`), which loses SRP responses / CASE Sigma frames
and breaks commissioning with an `OT_ERROR_RESPONSE_TIMEOUT` loop.
Mirrors the upstream fix validated on hardware in esp-rs/openthread#92 /
sysgrok/rs-matter-embassy PR#62 (not yet in a released crate).

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
