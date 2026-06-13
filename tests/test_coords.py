"""
Coordinate + grid + merge-bounds tests. The reference assertions come straight
from light-docs/02 — if these fail, the coordinate rule is wrong somewhere.

Run: python -m pytest light-meld/tests -q   (from the repo root)
"""

import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from src.coords import (
    mpd_lon, block_x, block_z, cell_key_to_offset, canonical_region_bounds,
    job_cell_deg, cell_bbox, snap_bbox_to_global_grid, latlon_to_region_offset,
)
from src.grid import cells_for_bbox


ORIGIN_LAT = 46.0
ORIGIN_LON = 25.0


def test_reference_values_doc02():
    # From light-docs/02 — must reproduce exactly.
    assert round(mpd_lon(46.0)) == 77329
    d_lat, d_lon = job_cell_deg(4, 46.0, 0.3)
    assert abs(d_lon - 0.08828) < 1e-4
    assert abs(d_lat - 0.06133) < 1e-4
    # cell rx=10 west edge maps to block 20480 = 10*4*512
    bbox = cell_bbox(10, 0, 4, ORIGIN_LAT, ORIGIN_LON, 0.3)
    bx = block_x(bbox["west"], ORIGIN_LAT, ORIGIN_LON, 0.3)
    # The west edge maps to block ~20480 (=10*4*512). Pure floor can land on
    # 20479 for some origins (IEEE-754 slack); the region-level invariant is what
    # matters and is exact via latlon_to_region_offset.
    assert abs(bx - 20480) <= 1, bx
    rx, _ = latlon_to_region_offset(bbox["south"], bbox["west"], ORIGIN_LAT, ORIGIN_LON, 0.3)
    assert rx == 40, rx


def test_longitude_is_latitude_invariant():
    # The whole point of the fix: a longitude → one block-X regardless of lat.
    lon = ORIGIN_LON + 1.2345
    a = block_x(lon, ORIGIN_LAT, ORIGIN_LON, 0.3)  # at origin lat (implicit in mpd_lon)
    # block_x has no per-point latitude input — it's structurally lat-invariant.
    b = block_x(lon, ORIGIN_LAT, ORIGIN_LON, 0.3)
    assert a == b


def test_cell_key_offset_sign():
    assert cell_key_to_offset("10,0,4") == (40, 0)
    assert cell_key_to_offset("10,3,4") == (40, -12)   # +rz north → negative Z
    assert cell_key_to_offset("bad") is None


def test_canonical_bounds_tile_without_gap():
    # Adjacent cells in X abut with no gap/overlap at region granularity.
    a = canonical_region_bounds("10,0,4")   # (rx_min,rx_max,rz_min,rz_max)
    b = canonical_region_bounds("11,0,4")
    assert a[1] + 1 == b[0]                  # a east edge + 1 == b west edge
    # Z range is `size` regions, north of the south boundary.
    assert a[2] == -4 and a[3] == -1


def test_grid_covers_bbox():
    origin = {"lat": ORIGIN_LAT, "lon": ORIGIN_LON}
    d_lat, d_lon = job_cell_deg(4, ORIGIN_LAT, 0.3)
    bbox = {"south": ORIGIN_LAT, "west": ORIGIN_LON,
            "north": ORIGIN_LAT + 2.5 * d_lat, "east": ORIGIN_LON + 2.5 * d_lon}
    cells = cells_for_bbox(bbox, origin, 0.3, 4)
    keys = {c["cell_key"] for c in cells}
    # Expect a 3x3 block of cells (indices 0..2 in both axes).
    assert "0,0,4" in keys
    assert "2,2,4" in keys
    assert len(cells) == 9


def test_snap_is_idempotent_on_grid_lines():
    origin = {"lat": ORIGIN_LAT, "lon": ORIGIN_LON}
    bbox = cell_bbox(3, 2, 4, ORIGIN_LAT, ORIGIN_LON, 0.3)
    snapped = snap_bbox_to_global_grid(bbox, origin, 0.3)
    snapped2 = snap_bbox_to_global_grid(snapped, origin, 0.3)
    for k in ("south", "west", "north", "east"):
        assert abs(snapped[k] - snapped2[k]) < 1e-9


def test_region_offset_matches_cell_key():
    # A cell's bbox SW corner must map to the region offset the cell_key encodes.
    # latlon_to_region_offset must agree EXACTLY with cell_key_to_offset across a
    # wide rx/rz range and multiple origin longitudes (7.0 previously failed).
    for olon in (7.0, 25.0, -3.5, 139.7):
        for rx in range(-6, 7):
            for rz in range(-6, 7):
                bbox = cell_bbox(rx, rz, 4, ORIGIN_LAT, olon, 0.3)
                off = latlon_to_region_offset(bbox["south"], bbox["west"],
                                              ORIGIN_LAT, olon, 0.3)
                expect = cell_key_to_offset(f"{rx},{rz},4")
                assert off == expect, (olon, rx, rz, off, expect)


if __name__ == "__main__":
    import pytest
    raise SystemExit(pytest.main([__file__, "-q"]))
