# 3D-printable mounts for the sensor (XIAO ESP32-C6 + 24GHz mmWave stack)

Two tape-on mounts that aim the presence sensor 45° downward:

| File | Where it goes | Aim |
|------|---------------|-----|
| `wall-mount-45.stl` | flat wall, mounted high | straight out, tilted 45° down |
| `corner-mount-45.stl` | 90° wall corner, near the ceiling | along the corner bisector (45° to each wall), tilted 45° down |
| `wall-mount-45-pro.stl` | same as wall mount | production trim — print this one to give away |
| `corner-mount-45-pro.stl` | same as corner mount | production trim — print this one to give away |

The `-pro` variants are geometrically identical (same pocket, retention,
recesses, pads, print orientation) with consumer-product cosmetics: rounded
plate/wing corners, radiused pocket body, a chamfer around the front rim, and
a wider retention lip for a cleaner front frame.

Both share the same snap-in pocket: the sensor stack drops in radar-face-out,
the **top edge slides under the rigid lip first**, then the bottom edge clicks
past two chamfered tabs. The bottom wall has a 12 mm slot for the USB-C cable —
install with the slot facing **down** so the cable hangs toward the floor.

**Before snapping the stack in, put one layer of the foam mounting tape
(~1 mm) on the pocket floor.** The cavity is sized to the measured stack
depth (8.77 mm to the USB-C shell, the deepest point on the back) plus
~0.7 mm of insertion clearance; the tape layer takes up that clearance and
preloads the front (radar) board's face against the lip/tabs so nothing
rattles.

## Designed for imperfect walls

The backs are **not** flat — only small raised lands touch the wall, so
texture high spots and corner-mud buildup can't rock the mount:

- **Wall mount:** two full-height 9 mm tape **rails** at the left/right edges;
  the middle of the back is recessed 1.2 mm.
- **Corner mount:** the apex is **cut back** — nothing comes within ~10 mm of
  the corner line, clearing corner bead radius and joint-compound buildup.
  The opening between the wing edges is a **12 mm cable channel**: the USB-C
  cable can route vertically along the corner, tucked behind the mount.
  Each 55 mm wing contacts its wall only through a 20 mm **pad at the outer
  end** (past the typical mud flare); the wing span between pad and body is
  recessed and thinned, so it flexes a degree or two and both pads still seat
  on an out-of-square corner. Press near both pads when sticking it up.

Put the tape **on the rails / pads only** — that's the only part that reaches
the wall. On textured walls use ≥1 mm foam tape (VHB or similar); the foam
absorbs what the recesses don't.

## Printing

- PLA or PETG, 0.2 mm layers, 2–3 perimeters, any infill ≥ 15 %. No supports.
- **Print both mounts upside down: flat top face on the bed.** (The recessed
  backs mean the old back-on-bed orientation would need bridging; printed
  top-down, every rail/pad/recess face is vertical and the worst overhang is
  the usual 45°.)

## Mounting

- Double-sided foam tape (3M VHB or similar): wall mount — one strip per rail;
  corner mount — one piece per wing pad.
- The assembly weighs ~20 g; clean the wall with isopropyl first and press for
  30 s (corner mount: press near the pads, not the apex).
- Sensor placement guidance (datasheet): wall mount ≈ 1.5–2 m high aims the
  45° wedge well when mounted higher (~2.2–2.7 m); the corner mount is meant
  for the ceiling corner of the room. The radar's ±60° beam covers most rooms.

## Provisioning a unit (flash + unique codes + pamphlet)

Each board that goes to someone else gets its own Matter passcode and
discriminator, flashed in and printed on a personalized fold-in-half
pamphlet (`pamphlet-template.html` is the template):

```sh
scripts/provision.py                 # plug the new board in first
scripts/provision_gui.py             # same thing with buttons
```

The GUI wraps the CLI: pick the port, hit **Provision new unit**, watch the
log; on success it shows the unit's code + QR with an *Open pamphlet PDF*
button, and lists previous units (open / re-render).

The **Flash repeater** button instead flashes the board as a Thread range
extender (`scripts/flash_repeater.py`, needs `.bringup/dataset.hex`): no
codes or pamphlet — plug it into USB power halfway to an out-of-range
sensor and its LED goes solid once it's routing.

The **External antenna (U.FL)** checkbox (both flows; CLI flag
`--external-antenna`) builds firmware that switches the XIAO's RF switch to
the U.FL connector at boot (GPIO3 low + GPIO14 high, see `src/board.rs`).
Tick it **only for boards that physically have a 2.4 GHz U.FL antenna
snapped on** — selecting the external port with nothing attached makes
range far worse, not better.

A desktop launcher ("Sensor Provisioning", teal radar icon) is installed at
`~/.local/share/applications/sensor-provisioning.desktop` pointing at
`scripts/provision_gui.py` with `scripts/provision-icon.png`.

That generates spec-valid random codes, builds the firmware with them baked
in (`MATTER_PASSCODE` / `MATTER_DISCRIMINATOR` env vars, see
`src/matter/mod.rs`), flashes over `/dev/ttyACM0` (`--port` to change),
derives the QR + 11-digit manual pairing code (self-tested against the
Matter spec's canonical vectors), and writes
`hardware/units/unit-NNN/{pamphlet.html,pamphlet.pdf,codes.json}` plus a
line in `hardware/units/registry.json`. Print the PDF on US Letter,
landscape, double-sided **flip on short edge**, fold in half.

`--no-flash` for a dry run; `--regen N` re-renders an existing unit's
pamphlet (e.g. after a template edit). After flashing, power-cycle and check
the boot log prints the same manual code as the pamphlet.

Note: units still use the Matter **test VID/PID** (`0xFFF1`/`0x8001`), so
every recipient's Google account must be added to your Developer Console
project — actually selling them requires CSA certification, per-device
attestation certs (DACs), and real vendor IDs.

## Regenerating / tweaking

`mounts.py` is a parametric FreeCAD script (all dimensions are named constants
at the top — clearance, wall thickness, tab overhang, plate/wing sizes, and
the forgiveness knobs `RECESS`, `CORNER_RELIEF`, `PAD_LEN`, `RAIL_W`). Run it
inside FreeCAD (or `freecadcmd`):

```python
OUT_DIR = "/path/to/hardware/mounts"
exec(open(OUT_DIR + "/mounts.py").read())
```

It rebuilds both solids, runs geometry assertions (valid solid, pocket rim on
the exact 45° aim plane, flat printable top, watertight mesh), writes both
STLs and `mounts.FCStd`.
