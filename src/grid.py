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
