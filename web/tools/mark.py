"""Generate the sloop mark and write it to web/public/favicon.svg.

Three pebbles, stacked and overlapping. Each outline is a closed Catmull-Rom
curve through eight noised points on an ellipse, so every one is an organic
shape rather than a rounded rectangle, and no two are alike. All three are the
same nominal size; the outline noise and the tilt are what differ.

The seam
--------
The pebbles overlap properly — they are not three shapes parked next to each
other. Each lower pebble is drawn through a mask that subtracts the pebble above
it *dilated by the seam width*, so exactly one thin transparent line is left
along each contact. Stroking every outline instead would leave two lines per
contact, one for each boundary crossing the overlap.

The seam is transparent rather than a background colour, so the mark sits on any
surface in either theme.
"""

import math
import os

BOX = 64.0
OVERLAP = 5.0   # how far consecutive pebbles nest into each other
SEAM = 1.7      # visible transparent line; the mask dilates by this, centred
PAD = 2.0
BRAND = "#D97757"


def pebble_points(rx, ry, fx, fy, angle):
    n = len(fx)
    a = math.radians(angle)
    pts = []
    for i in range(n):
        th = 2.0 * math.pi * i / n
        x = rx * math.cos(th) * fx[i]
        y = ry * math.sin(th) * fy[i]
        pts.append((x * math.cos(a) - y * math.sin(a), x * math.sin(a) + y * math.cos(a)))
    return pts


def catmull_to_bezier(pts, tension=1.0):
    n = len(pts)
    d = "M%.2f %.2f" % pts[0]
    for i in range(n):
        p0, p1, p2, p3 = pts[(i - 1) % n], pts[i], pts[(i + 1) % n], pts[(i + 2) % n]
        c1 = (p1[0] + (p2[0] - p0[0]) / 6.0 * tension, p1[1] + (p2[1] - p0[1]) / 6.0 * tension)
        c2 = (p2[0] - (p3[0] - p1[0]) / 6.0 * tension, p2[1] - (p3[1] - p1[1]) / 6.0 * tension)
        d += "c%.2f %.2f %.2f %.2f %.2f %.2f" % (
            c1[0] - p1[0], c1[1] - p1[1], c2[0] - p1[0], c2[1] - p1[1],
            p2[0] - p1[0], p2[1] - p1[1])
    return d + "z"


def bounds(pts):
    xs, ys = [p[0] for p in pts], [p[1] for p in pts]
    return min(xs), min(ys), max(xs), max(ys)


# Eight points, gentle noise: enough irregularity to read as a stone, not so much
# that the ends pinch into a lens. Same nominal size for all three.
SPECS = [
    dict(rx=25.0, ry=10.0, angle=-6.0,
         fx=[1.00, 0.96, 1.00, 1.05, 1.00, 1.06, 1.00, 0.94],
         fy=[1.00, 1.07, 1.05, 0.95, 1.00, 0.93, 0.96, 1.06]),
    dict(rx=25.0, ry=10.0, angle=5.0,
         fx=[1.05, 1.00, 0.95, 1.00, 1.03, 1.00, 1.06, 1.00],
         fy=[1.00, 0.94, 1.06, 1.04, 1.00, 1.07, 0.94, 0.96]),
    dict(rx=25.0, ry=10.0, angle=-2.0,
         fx=[0.96, 1.05, 1.00, 0.95, 1.04, 1.00, 1.00, 1.06],
         fy=[1.00, 1.05, 0.94, 1.06, 1.00, 0.95, 1.07, 1.00]),
]

placed, cursor = [], 0.0
for s in SPECS:
    pts = pebble_points(s["rx"], s["ry"], s["fx"], s["fy"], s["angle"])
    x0, y0, x1, y1 = bounds(pts)
    pts = [(x, y + cursor - y0) for x, y in pts]
    placed.append(pts)
    cursor = cursor - y0 + y1 - OVERLAP

allpts = [p for g in placed for p in g]
x0, y0, x1, y1 = bounds(allpts)
scale = min((BOX - 2 * PAD) / (x1 - x0), (BOX - 2 * PAD) / (y1 - y0))
ox = (BOX - (x1 - x0) * scale) / 2.0 - x0 * scale
oy = (BOX - (y1 - y0) * scale) / 2.0 - y0 * scale
placed = [[(x * scale + ox, y * scale + oy) for x, y in g] for g in placed]
paths = [catmull_to_bezier(g) for g in placed]

for i, g in enumerate(placed):
    b = bounds(g)
    print("pebble %d: x %6.2f..%6.2f  y %6.2f..%6.2f" % (i + 1, b[0], b[2], b[1], b[3]))
b = bounds([p for g in placed for p in g])
print("silhouette: x %.2f..%.2f  y %.2f..%.2f" % (b[0], b[2], b[1], b[3]))

P1, P2, P3 = paths


def svg(fill, mask_prefix, indent="  "):
    """Lower pebbles are drawn first and cut by the one that sits on them."""
    return f"""{indent}<defs>
{indent}  <mask id="{mask_prefix}-a" maskUnits="userSpaceOnUse" x="0" y="0" width="64" height="64">
{indent}    <rect width="64" height="64" fill="#fff" />
{indent}    <path d="{P2}" fill="#000" stroke="#000" stroke-width="{SEAM * 2}" />
{indent}  </mask>
{indent}  <mask id="{mask_prefix}-b" maskUnits="userSpaceOnUse" x="0" y="0" width="64" height="64">
{indent}    <rect width="64" height="64" fill="#fff" />
{indent}    <path d="{P1}" fill="#000" stroke="#000" stroke-width="{SEAM * 2}" />
{indent}  </mask>
{indent}</defs>
{indent}<g fill="{fill}">
{indent}  <path d="{P3}" mask="url(#{mask_prefix}-a)" />
{indent}  <path d="{P2}" mask="url(#{mask_prefix}-b)" />
{indent}  <path d="{P1}" />
{indent}</g>"""


here = os.path.dirname(os.path.abspath(__file__))
out = os.path.join(here, os.pardir, "public", "favicon.svg")
with open(out, "w", encoding="utf-8", newline="\n") as fh:
    fh.write(f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" role="img" aria-label="sloop">
  <!--
    Three data layers, stacked and touching. No container: the mark is the
    shapes themselves, in Claude Code orange.

    Each outline is a closed Catmull-Rom curve through eight noised points on an
    ellipse, so every layer is an organic pebble rather than a rounded rectangle
    and no two are the same. All three are the same nominal size; what differs is
    the shape and the tilt -- -6, +5 and -2 degrees -- so they rest on each other
    the way real things do instead of lining up.

    They overlap by {OVERLAP:g} units. Each lower pebble is drawn through a mask that
    subtracts the one above it dilated by the seam, which leaves exactly one
    {SEAM:g}-unit transparent line along each contact. Stroking every outline instead
    would leave two lines per contact, one per boundary crossing the overlap.

    The seam is transparent, not a background colour, so the mark sits on any
    surface in either theme.

    Generated by web/tools/mark.py. The noise factors are fixed constants, not random
    at build time, so this file is reproducible -- edit the script, not this.
  -->
{svg(BRAND, "seam")}
</svg>
""")
print("\nwrote", out)

# The inline copy for the Angular template: same geometry, token colour.
inline = os.path.join(here, "mark-inline.html")
with open(inline, "w", encoding="utf-8", newline="\n") as fh:
    fh.write(svg("currentColor", "mark", indent="        "))
print("wrote", inline)
