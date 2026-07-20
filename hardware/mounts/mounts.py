# Parametric wall mounts for the XIAO ESP32-C6 + 24GHz mmWave sensor stack.
#
# Run inside FreeCAD (GUI or freecadcmd):
#   OUT_DIR = "<this directory>"; exec(open(OUT_DIR + "/mounts.py").read())
#
# Produces two styling variants of each mount:
#   wall-mount-45.stl        flat-wall wedge, sensor tilted 45 deg down
#   corner-mount-45.stl      corner mount, aim = corner bisector tilted 45 deg down
#   wall-mount-45-pro.stl    same geometry, production trim (rounded + chamfered)
#   corner-mount-45-pro.stl  same geometry, production trim
#   mounts.FCStd             all four solids for inspection
#
# Sensor stack facts (Seeed datasheet/wiki):
#   mmWave board (front, radar faces out): 18 x 22 mm
#   XIAO ESP32-C6 (rear): 21 x 17.8 mm, USB-C protrudes from one short edge
#   Stack depth (front face -> back), measured on the real assembly:
#   ~8.77 mm to the USB-C shell (the deepest point), ~7.61 mm excluding it.
#   The pocket registers the FRONT board: a rigid lip on the top edge + two
#   snap tabs on the bottom edge hold its face; one ~1 mm layer of foam tape
#   on the floor preloads the stack forward (see README).

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
STACK_D = 8.8                  # measured stack depth incl. USB-C shell (8.77)
CLR = 0.3                      # per-side clearance around the front board
WALL = 2.5
FLOOR = 3.0
LIP_W, LIP_OVER = 12.0, 1.5    # rigid lip, top edge
TAB_W, TAB_OVER = 3.0, 1.0     # snap tabs, bottom edge
LIP_T = TAB_T = 2.0            # lip/tab height; both must stay equal — they
                               # jointly hold the board face TAB_T below the rim
# The lip/tabs hold the front face TAB_T below the rim; rotating the stack in
# sweeps its top-back corner to TAB_T + STACK_D deep, so the cavity needs that
# plus working clearance. The leftover axial play is taken up by one layer of
# foam tape on the floor.
CAV_D = STACK_D + TAB_T + 0.7  # cavity depth, floor -> front rim (11.5)
CAV_W = BOARD_W + 2 * CLR      # 18.6
CAV_L = BOARD_L + 2 * CLR      # 22.6
POC_W = CAV_W + 2 * WALL       # 23.6
POC_L = CAV_L + 2 * WALL       # 27.6
POC_H = FLOOR + CAV_D          # 14.5
SLOT_W = 12.0                  # USB-C cable slot in the bottom (short) wall

# Imperfect-wall forgiveness: everything is recessed RECESS off the wall
# planes except small contact lands (rails / pads), so texture high spots and
# corner-bead buildup can't rock the mount. Both mounts print upside down on
# their flat top face, which keeps every recess face vertical (no bridges).
RECESS = 1.2                   # stand-off of the non-contact back surfaces

# ---- production ("pro") trim: same geometry, softened cosmetics ----
POCKET_R = 3.0                 # pocket body corner rounding
RIM_CH = 1.0                   # chamfer around the front rim
PLATE_R = 5.0                  # wall-mount plate corner rounding
WING_R = 6.0                   # corner-mount wing outer-end rounding
LIP_W_PRO = 16.0               # wider lip = cleaner front frame

# ---- flat-wall wedge parameters (global: wall = y=0 plane, +y out, +z up) --
PLATE_W, PLATE_T = 40.0, 3.0
WEDGE_STANDOFF = 9.0           # wall clearance at the pocket's lower back edge
WEDGE_LIFT = 14.0              # height (z) of the pocket's lower back edge
PLATE_H = WEDGE_LIFT + POC_L * S2   # top coplanar with fill/pocket -> flat bed face
RAIL_W = 9.0                   # tape rails at the plate's left/right edges

# ---- corner mount parameters (walls = x=0 and y=0 planes, corner on z axis) -
WING_LEN, WING_T = 55.0, 3.0
CORNER_OFFSET = 15.0           # back lower-edge center, along the bisector
CORNER_LIFT = 16.0             # height (z) of that edge
WING_H = CORNER_LIFT + POC_L * S2  # wings flush with the fill's flat top
CABLE_GAP = 12.0               # clear opening between the wing edges at the apex
                               # (the USB-C cable routes vertically in the corner)
CORNER_RELIEF = 2 * WING_T + CABLE_GAP * S2  # apex cut-back (~14.5): no
                               # material where x + y < this
PAD_LEN = 20.0                 # wall-contact pads at the outer end of each wing


def edges_along(shape, axis, where=None):
    """Straight edges of `shape` parallel to `axis`, optionally filtered by a
    predicate on the edge's center of mass."""
    out = []
    for e in shape.Edges:
        if e.Curve.TypeId != "Part::GeomLine":
            continue
        t = e.tangentAt(e.FirstParameter)
        if abs(abs(t.dot(axis)) - 1) > 1e-9:
            continue
        if where is None or where(e.CenterOfMass):
            out.append(e)
    return out


def yz_profile_solid(pts, x0, width):
    """Extrude a closed y-z polygon (list of (y, z)) from x0 along +x."""
    poly = [App.Vector(x0, y, z) for (y, z) in pts]
    poly.append(poly[0])
    face = Part.Face(Part.makePolygon(poly))
    return face.extrude(App.Vector(width, 0, 0))


def make_pocket(pro=False):
    """Snap-in pocket solid in local coordinates."""
    outer = Part.makeBox(POC_W, POC_L, POC_H)
    if pro:
        outer = outer.makeFillet(POCKET_R, edges_along(outer, App.Vector(0, 0, 1)))
        rim = [f for f in outer.Faces
               if isinstance(f.Surface, Part.Plane)
               and abs(f.CenterOfMass.z - POC_H) < 1e-9]
        outer = outer.makeChamfer(RIM_CH, rim[0].Edges)
    cavity = Part.makeBox(CAV_W, CAV_L, CAV_D + 1,
                          App.Vector(WALL, WALL, FLOOR))
    solid = outer.cut(cavity)

    # Cable slot through the bottom wall, floor to rim.
    slot = Part.makeBox(SLOT_W, WALL + 2, CAV_D + 2,
                        App.Vector((POC_W - SLOT_W) / 2, -1, FLOOR))
    solid = solid.cut(slot)

    # Rigid lip over the top edge of the board (insert this edge first).
    lip_w = LIP_W_PRO if pro else LIP_W
    lip_y = WALL + CAV_L                     # top wall inner face
    lip = yz_profile_solid(
        [(lip_y, POC_H),
         (lip_y, POC_H - LIP_T),
         (lip_y - LIP_OVER + 0.5, POC_H - LIP_T),
         (lip_y - LIP_OVER, POC_H - LIP_T + 0.5),
         (lip_y - LIP_OVER, POC_H)],
        (POC_W - lip_w) / 2, lip_w)
    solid = solid.fuse(lip)

    # Two chamfered snap tabs on the bottom edge, flush with the slot edges.
    tab_pts = [(WALL, POC_H),
               (WALL, POC_H - TAB_T),
               (WALL + TAB_OVER, POC_H - TAB_T),
               (WALL + TAB_OVER, POC_H - TAB_T + 0.4),
               (WALL + 0.2, POC_H)]
    for x0 in ((POC_W - SLOT_W) / 2 - TAB_W, (POC_W + SLOT_W) / 2):
        solid = solid.fuse(yz_profile_solid(tab_pts, x0, TAB_W))

    return solid.removeSplitter()


def back_face(pro=False):
    """The pocket's outer back rectangle (local z=0), as a face."""
    if pro:
        slab = Part.makeBox(POC_W, POC_L, 1)
        slab = slab.makeFillet(POCKET_R, edges_along(slab, App.Vector(0, 0, 1)))
        return [f for f in slab.Faces if abs(f.CenterOfMass.z) < 1e-9][0]
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


def make_wall_mount(pro=False):
    # local x -> -X, local y -> up-out, local z (aim) -> out-down at 45 deg
    cols = (App.Vector(-1, 0, 0),
            App.Vector(0, S2, S2),
            App.Vector(0, S2, -S2))
    origin = App.Vector((PLATE_W + POC_W) / 2, WEDGE_STANDOFF, WEDGE_LIFT)

    pock = placed(make_pocket(pro), cols, origin)
    back = placed(back_face(pro), cols, origin)

    # Fill between the tilted back face and the wall plane.
    fill = back.extrude(App.Vector(0, -(WEDGE_STANDOFF + POC_L * S2 + 1), 0))
    fill = fill.common(Part.makeBox(200, 200, 200, App.Vector(-50, 0, -50)))

    plate = Part.makeBox(PLATE_W, PLATE_T, PLATE_H)
    if pro:
        # round the plate's silhouette corners (edges running along y)
        plate = plate.makeFillet(PLATE_R, edges_along(plate, App.Vector(0, 1, 0)))
    solid = plate.fuse(fill).fuse(pock)

    # Recess the wall side between the two tape rails so a texture high spot
    # under the middle can't rock the mount off the rails.
    solid = solid.cut(Part.makeBox(PLATE_W - 2 * RAIL_W, RECESS + 1,
                                   PLATE_H + 2,
                                   App.Vector(RAIL_W, -1, -1)))
    return solid.removeSplitter(), App.Vector(0, S2, -S2)


def make_corner_mount(pro=False):
    # aim = corner bisector tilted 45 deg down
    aim = App.Vector(0.5, 0.5, -S2)
    cols = (App.Vector(-S2, S2, 0),        # local x, horizontal
            App.Vector(0.5, 0.5, S2),      # local y, up along the bisector
            aim)
    bis = App.Vector(S2, S2, 0)
    center = bis * CORNER_OFFSET + App.Vector(0, 0, CORNER_LIFT)
    origin = center - cols[0] * (POC_W / 2)

    pock = placed(make_pocket(pro), cols, origin)
    back = placed(back_face(pro), cols, origin)

    # Fill from the tilted back face horizontally into the corner,
    # trimmed to the two walls (its flat top ends up at z = WING_H).
    fill = back.extrude(bis * -(CORNER_OFFSET + POC_W + 5))
    fill = fill.common(Part.makeBox(100, 100, WING_H + 1))

    wing_x = Part.makeBox(WING_T, WING_LEN, WING_H)
    wing_y = Part.makeBox(WING_LEN, WING_T, WING_H)
    if pro:
        # round each wing's outer-end corners (edges across the thickness)
        wing_x = wing_x.makeFillet(WING_R, edges_along(
            wing_x, App.Vector(1, 0, 0), lambda c: c.y > WING_LEN - 1))
        wing_y = wing_y.makeFillet(WING_R, edges_along(
            wing_y, App.Vector(0, 1, 0), lambda c: c.x > WING_LEN - 1))
    solid = wing_x.fuse(wing_y).fuse(fill).fuse(pock)

    # Apex relief: cut back everything within x + y < CORNER_RELIEF so the
    # mount never touches the corner itself (bead radius, mud buildup).
    tri = Part.Face(Part.makePolygon([
        App.Vector(-1, -1, -1),
        App.Vector(CORNER_RELIEF + 1, -1, -1),
        App.Vector(-1, CORNER_RELIEF + 1, -1),
        App.Vector(-1, -1, -1)]))
    solid = solid.cut(tri.extrude(App.Vector(0, 0, WING_H + 2)))

    # Recess each wing (and the fill) off its wall, sparing a PAD_LEN contact
    # pad at the outer end. Only the pads touch; the thinned wing span flexes
    # enough to seat both pads on an out-of-square corner.
    solid = solid.cut(Part.makeBox(RECESS + 1, WING_LEN - PAD_LEN + 1,
                                   WING_H + 2, App.Vector(-1, -1, -1)))
    solid = solid.cut(Part.makeBox(WING_LEN - PAD_LEN + 1, RECESS + 1,
                                   WING_H + 2, App.Vector(-1, -1, -1)))
    return solid.removeSplitter(), aim


def check(name, solid, aim):
    assert solid.isValid(), name + ": invalid shape"
    assert solid.Volume > 8000, name + ": volume %.0f too small" % solid.Volume
    # the front rim must be a planar face whose outward normal is the aim
    front = [f for f in solid.Faces
             if isinstance(f.Surface, Part.Plane)
             and f.normalAt(0, 0).getAngle(aim) < 1e-6]
    assert front, name + ": no face normal to the aim direction"
    bb = solid.BoundBox
    # flip-print bed face: the top must be flat with real area
    top = [f for f in solid.Faces
           if isinstance(f.Surface, Part.Plane)
           and f.normalAt(0, 0).getAngle(App.Vector(0, 0, 1)) < 1e-6
           and abs(f.CenterOfMass.z - bb.ZMax) < 1e-6]
    top_area = sum(f.Area for f in top)
    assert top_area > 300, name + ": top bed face only %.0f mm2" % top_area
    print("%s ok: volume %.0f mm3, bbox %.1f x %.1f x %.1f mm"
          % (name, solid.Volume, bb.XLength, bb.YLength, bb.ZLength))


solids = {}
for pro in (False, True):
    sfx = "-pro" if pro else ""
    for label, maker in (("wall-mount-45", make_wall_mount),
                         ("corner-mount-45", make_corner_mount)):
        solid, aim = maker(pro)
        check(label + sfx, solid, aim)
        solids[label + sfx] = solid

if "mounts" in App.listDocuments():
    App.closeDocument("mounts")
doc = App.newDocument("mounts")
OFFSETS = {"wall-mount-45": (0, 0), "corner-mount-45": (70, 0),
           "wall-mount-45-pro": (0, 90), "corner-mount-45-pro": (70, 90)}
for label, solid in solids.items():
    obj = doc.addObject("Part::Feature", label.replace("-", "_"))
    obj.Shape = solid
    dx, dy = OFFSETS[label]
    obj.Placement.Base = App.Vector(dx, dy, 0)
doc.recompute()
doc.saveAs(os.path.join(OUT, "mounts.FCStd"))

for label, solid in solids.items():
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
