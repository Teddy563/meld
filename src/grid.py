"""
grid.py — turn a selection bbox into a uniform grid of region-aligned cells.

v1 is a uniform grid (no adaptive recursive subdivision). Every cell edge falls
on an integer multiple of the cell degree-step from the origin, so cells tile
the world with no gaps at region granularity (light-docs/02).
"""

from __future__ import annotations

import math

from .coords import job_cell_deg, cell_bbox


def cells_for_bbox(bbox: dict, origin: dict, scale: float, size: int) -> list[dict]:
    """
    Return a list of cells covering `bbox`:
        [{ "cell_key": "rx,rz,size", "bbox": {south,west,north,east} }, ...]

    Indices are integer steps from the origin; a cell is included when it
    overlaps the selection bbox at all.
    """
    olat, olon = float(origin["lat"]), float(origin["lon"])
    d_lat, d_lon = job_cell_deg(size, olat, scale)
    if d_lat <= 0 or d_lon <= 0:
        return []

    south, north = float(bbox["south"]), float(bbox["north"])
    west, east = float(bbox["west"]), float(bbox["east"])

    rz_start = math.floor((south - olat) / d_lat)
    rz_end = math.ceil((north - olat) / d_lat)
    rx_start = math.floor((west - olon) / d_lon)
    rx_end = math.ceil((east - olon) / d_lon)

    cells: list[dict] = []
    for rz in range(rz_start, rz_end):
        cell_south = olat + rz * d_lat
        cell_north = cell_south + d_lat
        if cell_north <= south or cell_south >= north:
            continue
        for rx in range(rx_start, rx_end):
            cell_west = olon + rx * d_lon
            cell_east = cell_west + d_lon
            if cell_east <= west or cell_west >= east:
                continue
            cells.append({
                "cell_key": f"{rx},{rz},{size}",
                "bbox": cell_bbox(rx, rz, size, olat, olon, scale),
            })
    return cells


def _point_in_poly(lat: float, lon: float, poly: list[tuple[float, float]]) -> bool:
    """Ray-casting point-in-polygon. `poly` is a list of (lat, lon) vertices."""
    inside = False
    n = len(poly)
    j = n - 1
    for i in range(n):
        yi, xi = poly[i]
        yj, xj = poly[j]
        if ((yi > lat) != (yj > lat)) and \
           (lon < (xj - xi) * (lat - yi) / (yj - yi + 1e-18) + xi):
            inside = not inside
        j = i
    return inside


def _perp_dist(p, a, b) -> float:
    """Perpendicular distance from point p to segment a-b (planar, small-area ok).
    Points are (lat, lon)."""
    py, px = p
    ay, ax = a
    by, bx = b
    dx, dy = bx - ax, by - ay
    if dx == 0 and dy == 0:
        return math.hypot(px - ax, py - ay)
    t = ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)
    t = max(0.0, min(1.0, t))
    cx, cy = ax + t * dx, ay + t * dy
    return math.hypot(px - cx, py - cy)


def _simplify_ring(points, eps: float):
    """Iterative Douglas-Peucker. Drops vertices closer than `eps` (degrees) to the
    line they sit on, so a high-detail country border becomes a low-resolution outline
    with far fewer angles. Iterative (no recursion limit) for huge OSM rings."""
    n = len(points)
    if n < 4 or eps <= 0:
        return points
    keep = [False] * n
    keep[0] = keep[-1] = True
    stack = [(0, n - 1)]
    while stack:
        s, e = stack.pop()
        a, b = points[s], points[e]
        dmax, idx = 0.0, -1
        for i in range(s + 1, e):
            d = _perp_dist(points[i], a, b)
            if d > dmax:
                dmax, idx = d, i
        if idx != -1 and dmax > eps:
            keep[idx] = True
            stack.append((s, idx))
            stack.append((idx, e))
    out = [points[i] for i in range(n) if keep[i]]
    return out if len(out) >= 3 else points


def cells_for_polygons(rings, origin: dict, scale: float, size: int) -> list[dict]:
    """Tile the bounding box of one or more polygon rings, then keep only cells whose
    CENTER falls inside ANY ring. Lets the selection follow a region/country shape
    (incl. multi-polygon island nations), not just a square. Each ring is a list of
    [lat, lon] vertices.

    Country outlines from OSM are very high-detail, which makes a jagged tile edge, so
    each ring is first SIMPLIFIED to roughly cell resolution (border detail finer than a
    cell is invisible once tiled anyway)."""
    norm: list[list[tuple[float, float]]] = []
    for r in rings or []:
        pts = [(float(p[0]), float(p[1])) for p in r]
        if len(pts) >= 3:
            norm.append(pts)
    if not norm:
        return []
    # Simplify each ring to ~0.7 of a cell so the boundary has far fewer angles.
    mid_lat = sum(p[0] for r in norm for p in r) / sum(len(r) for r in norm)
    d_lat, d_lon = job_cell_deg(size, mid_lat, scale)
    eps = max(d_lat, d_lon) * 0.7
    norm = [_simplify_ring(r, eps) for r in norm]
    lats = [p[0] for r in norm for p in r]
    lons = [p[1] for r in norm for p in r]
    bbox = {"south": min(lats), "north": max(lats), "west": min(lons), "east": max(lons)}
    out: list[dict] = []
    for c in cells_for_bbox(bbox, origin, scale, size):
        b = c["bbox"]
        clat = (b["south"] + b["north"]) / 2.0
        clon = (b["west"] + b["east"]) / 2.0
        if any(_point_in_poly(clat, clon, r) for r in norm):
            out.append(c)
    return out


def cells_for_polygon(poly, origin: dict, scale: float, size: int) -> list[dict]:
    """Single-ring convenience wrapper around cells_for_polygons."""
    return cells_for_polygons([poly], origin, scale, size)
