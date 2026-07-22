#!/usr/bin/env python3
"""Build + flash a Thread repeater (range extender) onto a fresh ESP32-C6.

Usage: plug the new board in, then

    scripts/flash_repeater.py [--port /dev/ttyACM0]

The repeater firmware (src/bin/repeater.rs) joins the home Thread network as
an always-on router and forwards traffic for out-of-range sensors. It needs
the network's active dataset TLVs at build time; those are read from
.bringup/dataset.hex (captured from a commissioning-attempt log line
"Connecting to Thread network, dataset: ...").

After flashing, the board usually parks in the ROM bootloader — unplug it and
plug it into USB power where you want it; a real power cycle is what boots
the app. LED: fast blink = joining, slow blink = joined as child,
solid with a short dip every 3 s = promoted to Router and repeating.
"""

import argparse
import os
import re
import subprocess
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATASET = os.path.join(REPO, ".bringup", "dataset.hex")
ELF = os.path.join(
    REPO, "target", "riscv32imac-unknown-none-elf", "release", "repeater")


def read_dataset(path):
    try:
        with open(path) as f:
            hexstr = f.read().strip()
    except OSError as e:
        sys.exit(f"error: cannot read Thread dataset {path}: {e}\n"
                 "Capture it from a sensor's boot log line "
                 '"Connecting to Thread network, dataset: ..." and save the '
                 "hex there.")
    if not re.fullmatch(r"[0-9a-fA-F]+", hexstr) or len(hexstr) % 2:
        sys.exit(f"error: {path} is not an even-length hex string")
    return hexstr


def build(dataset_hex):
    env = dict(os.environ, THREAD_DATASET_HEX=dataset_hex)
    subprocess.run(
        ["cargo", "build", "--release", "--features", "ftd",
         "--bin", "repeater"],
        cwd=REPO, env=env, check=True)


def flash(port):
    subprocess.run(
        ["espflash", "flash", "--chip", "esp32c6", "--port", port,
         "--partition-table", os.path.join(REPO, "partitions.csv"), ELF],
        cwd=REPO, check=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--dataset", default=DATASET,
                    help="Thread active dataset TLV hex file "
                         "(default: .bringup/dataset.hex)")
    ap.add_argument("--no-flash", action="store_true",
                    help="build only, skip espflash")
    args = ap.parse_args()

    dataset_hex = read_dataset(args.dataset)
    print(f"==> building repeater (dataset: {args.dataset}, "
          f"{len(dataset_hex) // 2} bytes)")
    build(dataset_hex)

    if args.no_flash:
        print("==> dry run: skipped flashing; ELF at", ELF)
        return

    print(f"==> flashing {args.port}")
    flash(args.port)
    print("""
==> repeater flashed.

Now UNPLUG the board and plug it into a USB power adapter roughly halfway
between the border router and the out-of-range sensor. (The board stays in
the bootloader until a real power cycle.)

LED on the board:
  fast blink              joining the Thread network
  slow blink (1 Hz)       joined as child, waiting for router promotion
  solid, short dip / 3 s  Router — repeating traffic (can take ~10 min)

Then power-cycle the far sensor so it re-attaches through the repeater.""")


if __name__ == "__main__":
    main()
