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

## Printing

- PLA or PETG, 0.2 mm layers, 2–3 perimeters, any infill ≥ 15 %. No supports.
- `wall-mount-45`: print with the flat back plate on the bed.
- `corner-mount-45`: print with either wall wing flat on the bed (worst
  overhang is ~45°; add supports only if your slicer complains).

## Mounting

- Double-sided foam tape (3M VHB or similar) on the flat back plate — two
  vertical strips. The corner mount gets a strip on each wing.
- The assembly weighs ~20 g; clean the wall with isopropyl first and press for
  30 s.
- Sensor placement guidance (datasheet): wall mount ≈ 1.5–2 m high aims the
  45° wedge well when mounted higher (~2.2–2.7 m); the corner mount is meant
  for the ceiling corner of the room. The radar's ±60° beam covers most rooms.

## Regenerating / tweaking

`mounts.py` is a parametric FreeCAD script (all dimensions are named constants
at the top — clearance, wall thickness, tab overhang, plate sizes, angles are
derived). Run it inside FreeCAD:

```python
OUT_DIR = "/path/to/hardware/mounts"
exec(open(OUT_DIR + "/mounts.py").read())
```

It rebuilds both solids, runs geometry assertions (valid solid, pocket rim on
the exact 45° aim plane, watertight mesh), writes both STLs and `mounts.FCStd`.
