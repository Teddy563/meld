"""
coords.py — the single coordinate convention for light-meld.

THE RULE (light-docs/02-coordinates-grid.md):
    A longitude/latitude maps to a Minecraft block using a metres-per-degree
    constant anchored at the project ORIGIN latitude — never the point's own
    latitude or an average.

        mpd_lat      = 111320                      (constant)
        mpd_lon      = 111320 * cos(origin_lat)    (one project-wide constant)
        block_x(lon) = floor((lon - origin_lon) * mpd_lon * scale)
        block_z(lat) = floor((origin_lat - lat) * mpd_lat * scale)   # +Z = south

This file, the merge strip, the JS grid math, and the Arnis fork's
transform_point ALL obey this rule. When they agree, cell content fills its
canonical region rectangle exactly and the region-granular merge keeps complete
regions — no seam strips. (The historical bug was the fork using avg_lat; see
light-docs/03.)

Minecraft axes: +X = East, +Z = South. Region r.RX.RZ.mca covers blocks
[RX*512, RX*512+512) x [RZ*512, RZ*512+512); region = floor(block/512).
"""

from __future__ import annotations

import math

from .constants import REGION_BLOCKS, CHUNK_BLOCKS, METERS_PER_DEG_LAT


def mpd_lon(origin_lat: float) -> float:
    """Metres per degree of longitude, anchored at the origin latitude."""
    return METERS_PER_DEG_LAT * math.cos(math.radians(origin_lat))


# ── point → block ──────────────────────────────────────────────────────────

def block_x(lon: float, origin_lat: float, origin_lon: float, scale: float) -> int:
    return math.floor((lon - origin_lon) * mpd_lon(origin_lat) * scale)


def block_z(lat: float, origin_lat: float, scale: float) -> int:
    return math.floor((origin_lat - lat) * METERS_PER_DEG_LAT * scale)


# ── cell ↔ region offset ───────────────────────────────────────────────────

def cell_key_to_offset(cell_key: str) -> tuple[int, int] | None:
    """
    Exact region SW-corner offset for a grid cell key "rx,rz,size".
    +rz in the UI grid is north, but Minecraft +Z is south, so rz is negated.
    Returns (region_rx, region_rz) or None if the key is malformed.
    """
    parts = cell_key.split(",") if cell_key else []
    if len(parts) == 3:
        try:
            rx_idx, rz_idx, size = int(parts[0]), int(parts[1]), int(parts[2])
            return rx_idx * size, -rz_idx * size
        except ValueError:
            pass
    return None


def cell_size_from_key(cell_key: str) -> int | None:
    parts = cell_key.split(",") if cell_key else []
    if len(parts) == 3:
        try:
            return int(parts[2])
        except ValueError:
            return None
    return None


def canonical_region_bounds(cell_key: str) -> tuple[int, int, int, int] | None:
    """
    The inclusive region rectangle a cell OWNS: (rx_min, rx_max, rz_min, rz_max).
    Cells tile this with no gap/overlap at region granularity (light-docs/02).
    """
    off = cell_key_to_offset(cell_key)
    size = cell_size_from_key(cell_key)
    if off is None or size is None:
        return None
    rx_off, rz_off = off
    rx_min = rx_off
    rx_max = rx_off + size - 1
    rz_min = rz_off - size          # rz_off is the SOUTH boundary (exclusive); canonical is north of it
    rz_max = rz_off - 1
    return rx_min, rx_max, rz_min, rz_max


_REGION_EPS = 1e-6   # absorbs IEEE-754 floor jitter at exact region boundaries


def latlon_to_region_offset(sw_lat: float, sw_lon: float,
                            origin_lat: float, origin_lon: float,
                            scale: float) -> tuple[int, int]:
    """
    Region SW-corner offset for a bbox's SW corner. Origin-anchored mpd_lon.

    A small epsilon is added before the floor so a corner that sits exactly on a
    region line (e.g. block 20480 computed as 20479.99999999) resolves to the
    canonical region (40) rather than the previous one (39). Without it this
    disagrees with cell_key_to_offset by one region for ~70% of cells — a latent
    footgun if anything ever uses this for canonical bounds. (The merge itself
    uses cell_key_to_offset, which is exact integer math, so the merge was never
    affected; this just makes the two agree.)
    """
    mpl = mpd_lon(origin_lat)
    bx = math.floor((sw_lon - origin_lon) * mpl * scale + _REGION_EPS)
    bz = math.floor((origin_lat - sw_lat) * METERS_PER_DEG_LAT * scale + _REGION_EPS)
    return bx // REGION_BLOCKS, bz // REGION_BLOCKS


# ── cell grid geometry ─────────────────────────────────────────────────────

def snap_to_region_grid(lat: float, lon: float, scale: float,
                        anchor_lat: float = 0.0, anchor_lon: float = 0.0) -> tuple[float, float]:
    """
    Snap a lat/lon to the nearest region-grid corner for `scale`, measured from
    `anchor` (default the global 0,0 grid). Relock passes the EXISTING origin as
    the anchor so the origin only ever moves in whole-region steps along the same
    grid — the cell grid stays aligned across relocks instead of drifting.
    """
    rdeg_lat = REGION_BLOCKS / scale / METERS_PER_DEG_LAT
    slat = anchor_lat + round((lat - anchor_lat) / rdeg_lat) * rdeg_lat
    # Use the SNAPPED latitude for the lon quantum so the result is idempotent
    # (re-snapping an already-snapped origin returns the same value — needed for
    # pasting an origin from another project).
    ref_lat = anchor_lat if anchor_lat else slat
    rdeg_lon = REGION_BLOCKS / scale / mpd_lon(ref_lat)
    slon = anchor_lon + round((lon - anchor_lon) / rdeg_lon) * rdeg_lon
    return slat, slon


def job_cell_deg(size: int, origin_lat: float, scale: float) -> tuple[float, float]:
    """Degrees (lat, lon) spanned by one cell of `size` regions. Origin-anchored."""
    blocks_per_job = size * REGION_BLOCKS
    meters_per_job = blocks_per_job / scale
    d_lat = meters_per_job / METERS_PER_DEG_LAT
    d_lon = meters_per_job / mpd_lon(origin_lat)
    return d_lat, d_lon


def cell_bbox(rx_idx: int, rz_idx: int, size: int,
              origin_lat: float, origin_lon: float, scale: float) -> dict:
    """Canonical (un-expanded) bbox for a grid cell. Matches the JS grid math."""
    d_lat, d_lon = job_cell_deg(size, origin_lat, scale)
    south = origin_lat + rz_idx * d_lat
    west = origin_lon + rx_idx * d_lon
    return {"south": south, "west": west, "north": south + d_lat, "east": west + d_lon}


# ── snapping + seam expansion ──────────────────────────────────────────────

def snap_bbox_to_global_grid(bbox: dict, origin: dict, scale: float) -> dict:
    """
    Snap each bbox edge to the global BLOCK grid anchored at origin, so two
    independently-computed adjacent edges land on the identical lat/lon (which
    the fork then maps to the identical block). Returns a NEW dict; returns the
    input unchanged if origin/scale are invalid or the result is degenerate.
    """
    if not isinstance(bbox, dict) or not isinstance(origin, dict):
        return bbox
    olat, olon = origin.get("lat"), origin.get("lon")
    if olat is None or olon is None:
        return bbox
    try:
        scale_f = float(scale)
    except (TypeError, ValueError):
        return bbox
    if scale_f <= 0.0:
        return bbox

    mpd_lon_v = mpd_lon(float(olat))
    if mpd_lon_v <= 0.0:
        return bbox

    block_lat_deg = (1.0 / scale_f) / METERS_PER_DEG_LAT
    block_lon_deg = (1.0 / scale_f) / mpd_lon_v

    def snap_lat(v: float) -> float:
        return float(olat) + round((v - float(olat)) / block_lat_deg) * block_lat_deg

    def snap_lon(v: float) -> float:
        return float(olon) + round((v - float(olon)) / block_lon_deg) * block_lon_deg

    out = {
        "south": snap_lat(float(bbox["south"])),
        "north": snap_lat(float(bbox["north"])),
        "west": snap_lon(float(bbox["west"])),
        "east": snap_lon(float(bbox["east"])),
    }
    if out["south"] >= out["north"] or out["west"] >= out["east"]:
        return bbox
    return out


def expand_bbox_for_seam(bbox: dict, n_chunks: int, origin: dict, scale: float) -> dict:
    """
    Expand bbox by n_chunks (16 blocks each) on every side, then snap to the
    global grid. With n_chunks<=0 the bbox is still snapped so cell-to-cell
    edges align at fractional scale.
    """
    if n_chunks <= 0:
        return snap_bbox_to_global_grid(bbox, origin, scale)
    olat = (origin or {}).get("lat", (bbox["south"] + bbox["north"]) / 2.0)
    expand_lat = n_chunks * CHUNK_BLOCKS / scale / METERS_PER_DEG_LAT
    expand_lon = n_chunks * CHUNK_BLOCKS / scale / mpd_lon(float(olat))
    expanded = {
        "south": bbox["south"] - expand_lat,
        "west": bbox["west"] - expand_lon,
        "north": bbox["north"] + expand_lat,
        "east": bbox["east"] + expand_lon,
    }
    return snap_bbox_to_global_grid(expanded, origin, scale)
