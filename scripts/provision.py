#!/usr/bin/env python3
"""Provision a new sensor board end-to-end.

For each unit this tool:
  1. generates a unique, spec-valid Matter passcode + discriminator
  2. builds the firmware with them baked in (MATTER_PASSCODE /
     MATTER_DISCRIMINATOR env vars, see src/matter/mod.rs)
  3. flashes the board over USB (espflash)
  4. derives the Matter onboarding QR payload and 11-digit manual pairing
     code (verified against the spec's canonical test vectors at startup)
  5. fills the fold-in-half pamphlet template with the unit's QR + code and
     renders a print-ready PDF
  6. records everything in hardware/units/registry.json

Usage:
  scripts/provision.py                      # next unit number, build+flash+pamphlet
  scripts/provision.py --port /dev/ttyACM1
  scripts/provision.py --no-flash           # dry run (no board attached)
  scripts/provision.py --regen 3            # re-render unit 3's pamphlet only

Requires: cargo + espflash (already used by this repo), qrencode,
google-chrome or chromium (for the PDF render).
"""

import argparse
import datetime
import json
import os
import re
import secrets
import shutil
import subprocess
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
UNITS_DIR = os.path.join(REPO, "hardware", "units")
REGISTRY = os.path.join(UNITS_DIR, "registry.json")
TEMPLATE = os.path.join(REPO, "hardware", "mounts", "pamphlet-template.html")
ELF = os.path.join(
    REPO, "target", "riscv32imac-unknown-none-elf", "release",
    "esp32c6-matter-human-detection")

VID, PID = 0xFFF1, 0x8001          # test IDs baked into the firmware
DISCOVERY_CAPS = 0x02              # BLE commissioning
BASE38 = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ-."

INVALID_PASSCODES = {
    0, 11111111, 22222222, 33333333, 44444444, 55555555,
    66666666, 77777777, 88888888, 99999999, 12345678, 87654321,
}

# ---------------------------------------------------------------- Matter math

VERHOEFF_D = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9], [1, 2, 3, 4, 0, 6, 7, 8, 9, 5],
    [2, 3, 4, 0, 1, 7, 8, 9, 5, 6], [3, 4, 0, 1, 2, 8, 9, 5, 6, 7],
    [4, 0, 1, 2, 3, 9, 5, 6, 7, 8], [5, 9, 8, 7, 6, 0, 4, 3, 2, 1],
    [6, 5, 9, 8, 7, 1, 0, 4, 3, 2], [7, 6, 5, 9, 8, 2, 1, 0, 4, 3],
    [8, 7, 6, 5, 9, 3, 2, 1, 0, 4], [9, 8, 7, 6, 5, 4, 3, 2, 1, 0],
]
VERHOEFF_P = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9], [1, 5, 7, 6, 2, 8, 3, 0, 9, 4],
    [5, 8, 0, 3, 7, 9, 6, 1, 4, 2], [8, 9, 1, 6, 0, 4, 3, 5, 2, 7],
    [9, 4, 5, 3, 1, 2, 6, 8, 7, 0], [4, 2, 8, 6, 5, 7, 3, 9, 0, 1],
    [2, 7, 9, 3, 8, 0, 6, 4, 1, 5], [7, 0, 4, 6, 9, 1, 3, 2, 5, 8],
]
VERHOEFF_INV = [0, 4, 3, 2, 1, 5, 6, 7, 8, 9]


def verhoeff_check_digit(digits: str) -> str:
    c = 0
    for i, ch in enumerate(reversed(digits), start=1):
        c = VERHOEFF_D[c][VERHOEFF_P[i % 8][int(ch)]]
    return str(VERHOEFF_INV[c])


def manual_pairing_code(passcode: int, discriminator: int) -> str:
    """11-digit manual code, VID/PID not included (spec 5.1.4)."""
    d1 = (discriminator >> 10) & 0x3
    d2_6 = ((discriminator & 0x300) << 6) | (passcode & 0x3FFF)
    d7_10 = (passcode >> 14) & 0x1FFF
    body = "%d%05d%04d" % (d1, d2_6, d7_10)
    return body + verhoeff_check_digit(body)


def group_manual_code(code: str) -> str:
    return "%s-%s-%s" % (code[0:4], code[4:7], code[7:11])


def qr_payload(passcode: int, discriminator: int,
               vid: int = VID, pid: int = PID,
               caps: int = DISCOVERY_CAPS) -> str:
    """Onboarding payload "MT:..." (spec 5.1.3): 88-bit LSB-first pack,
    little-endian bytes, base38 in 3-byte groups."""
    bits = 0
    off = 0
    for value, width in ((0, 3), (vid, 16), (pid, 16), (0, 2), (caps, 8),
                         (discriminator, 12), (passcode, 27), (0, 4)):
        assert 0 <= value < (1 << width), (value, width)
        bits |= value << off
        off += width
    assert off == 88
    data = bits.to_bytes(11, "little")
    out = []
    for i in range(0, len(data), 3):
        group = data[i:i + 3]
        n = int.from_bytes(group, "little")
        for _ in range({3: 5, 2: 4, 1: 2}[len(group)]):
            out.append(BASE38[n % 38])
            n //= 38
    return "MT:" + "".join(out)


def qr_decode(payload: str):
    """Inverse of qr_payload, for the self-test."""
    s = payload.removeprefix("MT:")
    data = b""
    for i in range(0, len(s), 5):
        chunk = s[i:i + 5]
        n = 0
        for ch in reversed(chunk):
            n = n * 38 + BASE38.index(ch)
        data += n.to_bytes({5: 3, 4: 2, 2: 1}[len(chunk)], "little")
    bits = int.from_bytes(data, "little")
    out = {}
    off = 0
    for name, width in (("version", 3), ("vid", 16), ("pid", 16), ("flow", 2),
                        ("caps", 8), ("discriminator", 12), ("passcode", 27)):
        out[name] = (bits >> off) & ((1 << width) - 1)
        off += width
    return out


def self_test():
    # Canonical test vector: passcode 20202021, discriminator 3840.
    assert manual_pairing_code(20202021, 3840) == "34970112332", \
        "manual pairing code math broken"
    # Canonical onboarding payload (chip-tool test device, PID 0x8000).
    ref = qr_decode("MT:Y.K9042C00KA0648G00")
    assert (ref["vid"], ref["pid"], ref["discriminator"], ref["passcode"]) == \
        (0xFFF1, 0x8000, 3840, 20202021), \
        "QR payload math broken: decoded %r" % ref
    # Round-trip with our own encoder.
    own = qr_decode(qr_payload(48271936, 2741))
    assert (own["discriminator"], own["passcode"]) == (2741, 48271936)


# ------------------------------------------------------------------- registry

def load_registry():
    if os.path.exists(REGISTRY):
        with open(REGISTRY) as f:
            return json.load(f)
    return []


def save_registry(reg):
    os.makedirs(UNITS_DIR, exist_ok=True)
    with open(REGISTRY, "w") as f:
        json.dump(reg, f, indent=2)
        f.write("\n")


def generate_codes(reg):
    used_disc = {u["discriminator"] for u in reg}
    used_pass = {u["passcode"] for u in reg}
    while True:
        disc = secrets.randbelow(4096)
        if disc != 3840 and disc not in used_disc:   # never the test default
            break
    while True:
        passcode = secrets.randbelow(99999998) + 1
        if passcode not in INVALID_PASSCODES and passcode != 20202021 \
                and passcode not in used_pass:
            break
    return passcode, disc


# ------------------------------------------------------------------- pamphlet

def qr_svg(payload: str) -> str:
    out = subprocess.run(
        ["qrencode", "-t", "svg", "-l", "M", "-m", "2", "-o", "-", payload],
        check=True, capture_output=True).stdout.decode()
    m = re.search(r"<svg.*</svg>", out, re.S)
    if not m:
        sys.exit("could not parse qrencode SVG output")
    svg = m.group(0)
    if "viewBox" not in svg:
        wh = re.search(r'width="([\d.]+)\w*"\s+height="([\d.]+)\w*"', svg)
        svg = svg.replace(
            "<svg ",
            '<svg viewBox="0 0 %s %s" ' % (wh.group(1), wh.group(2)), 1)
    return svg


def render_pamphlet(unit):
    with open(TEMPLATE) as f:
        html = f.read()
    subs = {
        "{{UNIT}}": "%03d" % unit["unit"],
        "{{DATE}}": unit["date"],
        "{{MANUAL_CODE}}": unit["manual_code"],
        "{{MANUAL_CODE_GROUPED}}": group_manual_code(unit["manual_code"]),
        "{{QR_TEXT}}": unit["qr"],
        "{{QR_SVG}}": qr_svg(unit["qr"]),
    }
    for k, v in subs.items():
        if k != "{{QR_SVG}}":
            assert k in html, "template is missing " + k
        html = html.replace(k, v)

    unit_dir = os.path.join(UNITS_DIR, "unit-%03d" % unit["unit"])
    os.makedirs(unit_dir, exist_ok=True)
    html_path = os.path.join(unit_dir, "pamphlet.html")
    with open(html_path, "w") as f:
        f.write(html)
    with open(os.path.join(unit_dir, "codes.json"), "w") as f:
        json.dump(unit, f, indent=2)
        f.write("\n")

    chrome = shutil.which("google-chrome") or shutil.which("chromium") \
        or shutil.which("chromium-browser")
    pdf_path = os.path.join(unit_dir, "pamphlet.pdf")
    if chrome:
        subprocess.run(
            [chrome, "--headless=new", "--disable-gpu",
             "--print-to-pdf=" + pdf_path, "--no-pdf-header-footer",
             "file://" + html_path],
            check=True, capture_output=True)
    else:
        print("! no chrome/chromium found - open pamphlet.html and print to "
              "PDF manually")
    return unit_dir


# ----------------------------------------------------------------- build/flash

def build(passcode, disc):
    env = dict(os.environ,
               MATTER_PASSCODE=str(passcode),
               MATTER_DISCRIMINATOR=str(disc))
    subprocess.run(["cargo", "build", "--release"], cwd=REPO, env=env,
                   check=True)


def flash(port):
    subprocess.run(
        ["espflash", "flash", "--chip", "esp32c6", "--port", port,
         "--partition-table", os.path.join(REPO, "partitions.csv"), ELF],
        cwd=REPO, check=True)


# ------------------------------------------------------------------------ main

def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--unit", type=int, help="unit number (default: next)")
    ap.add_argument("--no-flash", action="store_true",
                    help="build + pamphlet only, skip espflash")
    ap.add_argument("--no-build", action="store_true",
                    help="skip cargo build (use the existing ELF)")
    ap.add_argument("--regen", type=int, metavar="UNIT",
                    help="re-render an existing unit's pamphlet and exit")
    args = ap.parse_args()

    self_test()
    reg = load_registry()

    if args.regen is not None:
        matches = [u for u in reg if u["unit"] == args.regen]
        if not matches:
            sys.exit("unit %d not in %s" % (args.regen, REGISTRY))
        out = render_pamphlet(matches[0])
        print("re-rendered pamphlet in", out)
        return

    unit_no = args.unit if args.unit is not None else \
        (max((u["unit"] for u in reg), default=0) + 1)
    if any(u["unit"] == unit_no for u in reg):
        sys.exit("unit %d already exists (use --regen %d to re-render its "
                 "pamphlet)" % (unit_no, unit_no))

    passcode, disc = generate_codes(reg)
    unit = {
        "unit": unit_no,
        "date": datetime.date.today().isoformat(),
        "vid": VID, "pid": PID,
        "passcode": passcode,
        "discriminator": disc,
        "manual_code": manual_pairing_code(passcode, disc),
        "qr": qr_payload(passcode, disc),
    }

    print("== unit %03d: passcode %d, discriminator %d" %
          (unit_no, passcode, disc))
    if not args.no_build:
        print("== building firmware with baked codes ...")
        build(passcode, disc)
    if not args.no_flash:
        print("== flashing %s ..." % args.port)
        flash(args.port)

    reg.append(unit)
    save_registry(reg)
    out = render_pamphlet(unit)

    print()
    print("unit %03d provisioned:" % unit_no)
    print("  manual code : %s" % group_manual_code(unit["manual_code"]))
    print("  QR payload  : %s" % unit["qr"])
    print("  pamphlet    : %s/pamphlet.pdf" % out)
    if not args.no_flash:
        print("  sanity check: power-cycle the board and confirm the boot log")
        print("  prints the same manual pairing code.")


if __name__ == "__main__":
    main()
