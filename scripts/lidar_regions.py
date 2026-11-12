"""Assign bulk source files to geographic folders using the project's Natural Earth outlines."""
from functools import lru_cache
import json
import math
from pathlib import Path

import lidar_catalog as catalog


@lru_cache(maxsize=1)
def boundaries():
    path = Path(__file__).resolve().parents[1] / "crates/cache_builder/assets/ne_110m_countries.json"
    result = []
    for f in json.loads(path.read_bytes())["features"]:
        props = f["properties"]
        region = (props["CONTINENT"], props["NAME_LONG"])
        for rings in catalog.polygons(f["geometry"]):
            result.append((region, catalog.bounds({"type": "Polygon", "coordinates": rings}), rings))
    return result


def inside(ring, x, y):
    hit = False
    for (x0, y0), (x1, y1) in zip(ring, ring[1:]):
        if (y0 > y) != (y1 > y) and x < x0 + (x1-x0)*(y-y0)/(y1-y0):
            hit = not hit
    return hit


def region(survey, box):
    # The national catalogs include islands and offshore measurements beyond the coast outline.
    for prefix, home in (("LINZ:", ("Oceania", "New Zealand")), ("INFOMAR:", ("Europe", "Ireland")),
                         ("swisssurface3d:", ("Europe", "Switzerland")), ("eba-archives", ("South America", "Brazil"))):
        if survey.startswith(prefix):
            return home
    w, s, e, n = box
    x, y = (w + ((e-w) % 360)/2 + 180) % 360 - 180, (s+n)/2
    near = []
    scale = max(.05, math.cos(math.radians(y)))
    for name, (l, b, r, t), rings in boundaries():
        for lon in (x, x-360, x+360):
            if l <= lon <= r and b <= y <= t and inside(rings[0], lon, y) and not any(inside(h, lon, y) for h in rings[1:]):
                return name
            # Airborne coastal tiles can lie just offshore. Use distance to the shoreline.
            if l-1/scale <= lon <= r+1/scale and b-1 <= y <= t+1:
                for (a, c), (d, f) in zip(rings[0], rings[0][1:]):
                    dx, dy = (d-a)*scale, f-c
                    u = max(0., min(1., ((lon-a)*scale*dx+(y-c)*dy)/(dx*dx+dy*dy or 1)))
                    near.append((((lon-a)*scale-u*dx)**2+(y-c-u*dy)**2, name))
    if near:
        distance, name = min(near)
        if distance < 1:
            return name
    if survey.startswith("NOAAID:"):
        return ("Oceania" if x > 135 or x < -150 and y < 30 else "North America", "United States")
    return ("Ocean", "Offshore")
