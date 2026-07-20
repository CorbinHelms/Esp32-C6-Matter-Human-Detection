# 3D-printable mounts for the sensor (XIAO ESP32-C6 + 24GHz mmWave stack)

Two tape-on mounts that aim the presence sensor 45° downward:

| File | Where it goes | Aim |
|------|---------------|-----|
| `wall-mount-45.stl` | flat wall, mounted high | straight out, tilted 45° down |
| `corner-mount-45.stl` | 90° wall corner, near the ceiling | along the corner bisector (45° to each wall), tilted 45° down |

Both share the same snap-in pocket: the sensor stack drops in radar-face-out,
the **top edge slides under the rigid lip first**, then the bottom edge clicks
past two chamfered tabs. The bottom wall has a 10 mm slot for the USB-C cable —
install with the slot facing **down** so the cable hangs toward the floor.

**Before snapping the stack in, put a ~3–5 mm foam pad (or a few stacked layers
of the mounting tape) on the pocket floor.** The pocket registers only the
front (radar) board's face; the pad preloads the stack forward against the
lip/tabs regardless of your header height, and swallows the USB-C connector
bump on the back of the XIAO.

## Designed for imperfect walls

The backs are **not** flat — only small raised lands touch the wall, so
texture high spots and corner-mud buildup can't rock the mount:

- **Wall mount:** two full-height 9 mm tape **rails** at the left/right edges;
  the middle of the back is recessed 1.2 mm.
- **Corner mount:** the apex is **cut back** — nothing comes within ~7 mm of
  the corner line, clearing corner bead radius and joint-compound buildup.
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
