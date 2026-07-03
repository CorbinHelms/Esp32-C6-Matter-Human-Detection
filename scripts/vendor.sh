#!/usr/bin/env bash
# Recreate the vendor/ overrides used by [patch.crates-io] in Cargo.toml.
#
# Copies the pristine `openthread` / `openthread-sys` sources out of the local
# cargo registry cache and applies the small diffs kept in patches/. Run once
# after a fresh clone (a prior `cargo fetch` / failed build populates the
# cache). vendor/ itself is .gitignore'd — patches/ is the source of truth.
set -euo pipefail
cd "$(dirname "$0")/.."

REG=(~/.cargo/registry/src/index.crates.io-*)
OT="${REG[0]}/openthread-0.2.0"
OT_SYS="${REG[0]}/openthread-sys-0.2.1"
ESP_RADIO="${REG[0]}/esp-radio-0.18.0"

for d in "$OT" "$OT_SYS" "$ESP_RADIO"; do
    [[ -d "$d" ]] || {
        echo "error: $d not found in the cargo cache — run 'cargo fetch' first" >&2
        exit 1
    }
done

rm -rf vendor
mkdir -p vendor
cp -r "$OT" vendor/openthread
cp -r "$OT_SYS" vendor/openthread-sys
cp -r "$ESP_RADIO" vendor/esp-radio

# Force the on-the-fly OpenThread C build (so the OT_LOG_LEVEL patch takes
# effect): drop the prebuilt archives and the pre-generated bindings for our
# target. Also drop cargo cache markers.
rm -rf vendor/openthread-sys/libs
rm -f vendor/openthread-sys/src/include/riscv32imac-unknown-none-elf.rs
rm -f vendor/openthread/.cargo-ok vendor/openthread-sys/.cargo-ok vendor/esp-radio/.cargo-ok

for p in patches/*.patch; do
    patch -p1 -d vendor <"$p"
done

echo "vendor/ regenerated; patches applied."
