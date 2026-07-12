# Parametric wall mounts for the XIAO ESP32-C6 + 24GHz mmWave sensor stack.
#
# Run inside FreeCAD (GUI or freecadcmd):
#   OUT_DIR = "<this directory>"; exec(open(OUT_DIR + "/mounts.py").read())
#
# Produces:
#   wall-mount-45.stl   flat-wall wedge, sensor tilted 45 deg down
#   corner-mount-45.stl corner mount, aim = corner bisector tilted 45 deg down
#   mounts.FCStd        both solids for inspection
#
# Sensor stack facts (Seeed datasheet/wiki):
#   mmWave board (front, radar faces out): 18 x 22 mm
#   XIAO ESP32-C6 (rear): 21 x 17.8 mm, USB-C protrudes from one short edge
#   Stack depth (front face -> back of XIAO incl. headers) is not published
#   (~12-16 mm depending on header height), so the pocket only registers the
#   FRONT board: a rigid lip on the top edge + two snap tabs on the bottom
#   edge hold its face; a foam pad on the pocket floor preloads the stack
#   forward (see README).

import FreeCAD as App
import Part
import Mesh  # noqa: F401  (registers mesh types)
import MeshPart
import math
import os

OUT = globals().get("OUT_DIR", os.getcwd())
S2 = math.sqrt(2) / 2

# ---- pocket parameters (local frame: x = board width, y = board length,
# ---- z = sensor aim axis, z=0 at the outer back face) ----
BOARD_W, BOARD_L = 18.0, 22.0
CLR = 0.3                      # per-side clearance around the front board
WALL = 2.5
FLOOR = 3.0
CAV_D = 13.5                   # cavity depth, floor -> front rim
CAV_W = BOARD_W + 2 * CLR      # 18.6
CAV_L = BOARD_L + 2 * CLR      # 22.6
POC_W = CAV_W + 2 * WALL       # 23.6
POC_L = CAV_L + 2 * WALL       # 27.6
POC_H = FLOOR + CAV_D          # 16.5
SLOT_W = 10.0                  # USB-C cable slot in the bottom (short) wall
LIP_W, LIP_OVER, LIP_T = 12.0, 1.5, 1.5   # rigid lip, top edge
TAB_W, TAB_OVER, TAB_T = 3.0, 1.0, 1.5    # snap tabs, bottom edge

# ---- flat-wall wedge parameters (global: wall = y=0 plane, +y out, +z up) --
PLATE_W, PLATE_H, PLATE_T = 40.0, 44.0, 3.0
WEDGE_STANDOFF = 9.0           # wall clearance at the pocket's lower back edge
WEDGE_LIFT = 14.0              # height (z) of the pocket's lower back edge

# ---- corner mount parameters (walls = x=0 and y=0 planes, corner on z axis) -
WING_LEN, WING_H, WING_T = 40.0, 46.0, 3.0
CORNER_OFFSET = 15.0           # back lower-edge center, along the bisector
CORNER_LIFT = 16.0             # height (z) of that edge
FILL_CAP_MARGIN = 4.0


def yz_profile_solid(pts, x0, width):
    """Extrude a closed y-z polygon (list of (y, z)) from x0 along +x."""
    poly = [App.Vector(x0, y, z) for (y, z) in pts]
    poly.append(poly[0])
    face = Part.Face(Part.makePolygon(poly))
    return face.extrude(App.Vector(width, 0, 0))


def make_pocket():
    """Snap-in pocket solid in local coordinates."""
    outer = Part.makeBox(POC_W, POC_L, POC_H)
    cavity = Part.makeBox(CAV_W, CAV_L, CAV_D + 1,
                          App.Vector(WALL, WALL, FLOOR))
    solid = outer.cut(cavity)

    # Cable slot through the bottom wall, floor to rim.
    slot = Part.makeBox(SLOT_W, WALL + 2, CAV_D + 2,
                        App.Vector((POC_W - SLOT_W) / 2, -1, FLOOR))
    solid = solid.cut(slot)

    # Rigid lip over the top edge of the board (insert this edge first).
    lip_y = WALL + CAV_L                     # top wall inner face
    lip = yz_profile_solid(
        [(lip_y, POC_H),
         (lip_y, POC_H - LIP_T),
         (lip_y - LIP_OVER + 0.5, POC_H - LIP_T),
         (lip_y - LIP_OVER, POC_H - LIP_T + 0.5),
         (lip_y - LIP_OVER, POC_H)],
        (POC_W - LIP_W) / 2, LIP_W)
    solid = solid.fuse(lip)

    # Two chamfered snap tabs on the bottom edge, flanking the slot.
    tab_pts = [(WALL, POC_H),
               (WALL, POC_H - TAB_T),
               (WALL + TAB_OVER, POC_H - TAB_T),
               (WALL + TAB_OVER, POC_H - TAB_T + 0.4),
               (WALL + 0.2, POC_H)]
    for x0 in (WALL + 0.5, POC_W - WALL - 0.5 - TAB_W):
        solid = solid.fuse(yz_profile_solid(tab_pts, x0, TAB_W))

    return solid.removeSplitter()


def back_face():
    """The pocket's outer back rectangle (local z=0), as a face."""
    pts = [App.Vector(0, 0, 0), App.Vector(POC_W, 0, 0),
           App.Vector(POC_W, POC_L, 0), App.Vector(0, POC_L, 0),
           App.Vector(0, 0, 0)]
    return Part.Face(Part.makePolygon(pts))


def placed(shape, cols, origin):
    """Rigid transform: local basis -> (col1, col2, col3), origin -> origin."""
    c1, c2, c3 = cols
    m = App.Matrix(c1.x, c2.x, c3.x, origin.x,
                   c1.y, c2.y, c3.y, origin.y,
                   c1.z, c2.z, c3.z, origin.z,
                   0, 0, 0, 1)
    s = shape.copy()
    s.transformShape(m)
    return s


def make_wall_mount(pocket):
    # local x -> -X, local y -> up-out, local z (aim) -> out-down at 45 deg
    cols = (App.Vector(-1, 0, 0),
            App.Vector(0, S2, S2),
            App.Vector(0, S2, -S2))
    origin = App.Vector((PLATE_W + POC_W) / 2, WEDGE_STANDOFF, WEDGE_LIFT)

    pock = placed(pocket, cols, origin)
    back = placed(back_face(), cols, origin)

    # Fill between the tilted back face and the wall plane.
    fill = back.extrude(App.Vector(0, -(WEDGE_STANDOFF + POC_L * S2 + 1), 0))
    fill = fill.common(Part.makeBox(200, 200, 200, App.Vector(-50, 0, -50)))

    plate = Part.makeBox(PLATE_W, PLATE_T, PLATE_H)
    return plate.fuse(fill).fuse(pock).removeSplitter(), App.Vector(0, S2, -S2)


def make_corner_mount(pocket):
    # aim = corner bisector tilted 45 deg down
    aim = App.Vector(0.5, 0.5, -S2)
    cols = (App.Vector(-S2, S2, 0),        # local x, horizontal
            App.Vector(0.5, 0.5, S2),      # local y, up along the bisector
            aim)
    bis = App.Vector(S2, S2, 0)
    center = bis * CORNER_OFFSET + App.Vector(0, 0, CORNER_LIFT)
    origin = center - cols[0] * (POC_W / 2)

    pock = placed(pocket, cols, origin)
    back = placed(back_face(), cols, origin)

    # Fill from the tilted back face horizontally into the corner,
    # trimmed to the two walls and capped on top.
    top = CORNER_LIFT + POC_L * S2 + FILL_CAP_MARGIN
    fill = back.extrude(bis * -(CORNER_OFFSET + POC_W + 5))
    fill = fill.common(Part.makeBox(100, 100, top))

    wing_x = Part.makeBox(WING_T, WING_LEN, WING_H)
    wing_y = Part.makeBox(WING_LEN, WING_T, WING_H)
    return wing_x.fuse(wing_y).fuse(fill).fuse(pock).removeSplitter(), aim


def check(name, solid, aim):
    assert solid.isValid(), name + ": invalid shape"
    assert solid.Volume > 8000, name + ": volume %.0f too small" % solid.Volume
    # the front rim must be a planar face whose outward normal is the aim
    front = [f for f in solid.Faces
             if isinstance(f.Surface, Part.Plane)
             and f.normalAt(0, 0).getAngle(aim) < 1e-6]
    assert front, name + ": no face normal to the aim direction"
    bb = solid.BoundBox
    print("%s ok: volume %.0f mm3, bbox %.1f x %.1f x %.1f mm"
          % (name, solid.Volume, bb.XLength, bb.YLength, bb.ZLength))


pocket = make_pocket()
wall_mount, wall_aim = make_wall_mount(pocket)
corner_mount, corner_aim = make_corner_mount(pocket)
check("wall-mount-45", wall_mount, wall_aim)
check("corner-mount-45", corner_mount, corner_aim)

doc = App.newDocument("mounts")
for label, solid in (("wall_mount_45", wall_mount),
                     ("corner_mount_45", corner_mount)):
    obj = doc.addObject("Part::Feature", label)
    obj.Shape = solid
doc.getObject("corner_mount_45").Placement.Base = App.Vector(70, 0, 0)
doc.recompute()
doc.saveAs(os.path.join(OUT, "mounts.FCStd"))

for label, solid in (("wall-mount-45", wall_mount),
                     ("corner-mount-45", corner_mount)):
    mesh = MeshPart.meshFromShape(Shape=solid, LinearDeflection=0.08,
                                  AngularDeflection=math.radians(20),
                                  Relative=False)
    assert mesh.isSolid() and not mesh.hasNonManifolds(), label + ": bad mesh"
    path = os.path.join(OUT, label + ".stl")
    mesh.write(path)
    print("exported %s (%d facets)" % (path, mesh.CountFacets))

if App.GuiUp:
    import FreeCADGui as Gui
    Gui.SendMsgToActiveView("ViewFit")
    Gui.activeDocument().activeView().viewIsometric()

print("done")
