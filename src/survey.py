"""
survey.py — coarse elevation survey for the whole selection, producing one
global (min_m, max_m) to LOCK across every cell so terrain height is continuous
(light-docs/04). Uses Mapzen/AWS Terrarium tiles at a low zoom (min/max only,
detail not needed).

Pillow is used to decode the PNG tiles. If Pillow is unavailable the survey
returns ok=False with a reason so the UI can fall back to a manual range — the
pipeline still runs, just without the auto lock.

Terrarium decode: elevation_m = (R*256 + G + B/256) - 32768.
"""

from __future__ import annotations

import io
import math
from urllib.request import Request, urlopen

TERRARIUM_URL = "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png"
_OFFSET = 32768.0


def _lat_lng_to_tile(lat: float, lng: float, zoom: int) -> tuple[int, int]:
    lat_rad = math.radians(lat)
    n = 2 ** zoom
    x = int((lng + 180.0) / 360.0 * n)
    y = int((1.0 - math.asinh(math.tan(lat_rad)) / math.pi) / 2.0 * n)
    return max(0, min(n - 1, x)), max(0, min(n - 1, y))


def _decode_minmax(png_bytes: bytes):
    from PIL import Image  # imported lazily so the module loads without Pillow
    img = Image.open(io.BytesIO(png_bytes)).convert("RGB")
    lo, hi = float("inf"), float("-inf")
    # Sample every 8th pixel — min/max is robust to subsampling and ~64x faster.
    px = img.load()
    w, h = img.size
    for yy in range(0, h, 8):
        for xx in range(0, w, 8):
            r, g, b = px[xx, yy]
            elev = (r * 256 + g + b / 256.0) - _OFFSET
            # Skip AWS Terrarium no-data sentinels / out-of-band spikes before
            # they reach the global lock. A (0,0,0) pixel decodes to -32768 m
            # (unscanned / no-data, common at tile edges and over some water);
            # deep bathymetry below -500 m is never rendered (Arnis caps water
            # depth). Without this guard a single such pixel craters global_min,
            # the lock range balloons to tens of km, and Arnis — which consumes
            # --elevation-min/max DIRECTLY (no filtering on the override path) —
            # compresses real terrain into a few flat blocks, so the whole world
            # renders nearly flat. Band [-500, 9000] (Dead Sea floor to Everest)
            # mirrors meld/elevation_survey.py:_terrarium_min_max and the Arnis
            # render-side nodata guards (regional val>-9999 / postprocess IQR).
            if elev < -500.0 or elev > 9000.0:
                continue
            if elev < lo:
                lo = elev
            if elev > hi:
                hi = elev
    return lo, hi


def survey_elevation(bbox: dict, zoom: int = 10) -> dict:
    """
    Return {"ok": bool, "min_m": float, "max_m": float, "tiles": int, "reason": str}.
    """
    try:
        from PIL import Image  # noqa: F401
    except Exception:
        return {"ok": False, "reason": "Pillow not installed (pip install Pillow); "
                                       "set the elevation range manually.",
                "min_m": None, "max_m": None, "tiles": 0}

    south, west = float(bbox["south"]), float(bbox["west"])
    north, east = float(bbox["north"]), float(bbox["east"])
    x1, y1 = _lat_lng_to_tile(north, west, zoom)
    x2, y2 = _lat_lng_to_tile(south, east, zoom)

    global_min, global_max, tiles = float("inf"), float("-inf"), 0
    for tx in range(min(x1, x2), max(x1, x2) + 1):
        for ty in range(min(y1, y2), max(y1, y2) + 1):
            url = TERRARIUM_URL.format(z=zoom, x=tx, y=ty)
            try:
                req = Request(url, headers={"User-Agent": "light-meld/0.1"})
                data = urlopen(req, timeout=30).read()
                lo, hi = _decode_minmax(data)
                global_min = min(global_min, lo)
                global_max = max(global_max, hi)
                tiles += 1
            except Exception:
                continue

    if tiles == 0 or global_min == float("inf"):
        return {"ok": False, "reason": "No elevation tiles fetched (network?).",
                "min_m": None, "max_m": None, "tiles": 0}
    return {"ok": True, "min_m": round(global_min, 1), "max_m": round(global_max, 1),
            "tiles": tiles, "reason": ""}
