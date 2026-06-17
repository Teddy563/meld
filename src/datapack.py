"""
datapack.py — region data packs: bulk-download a whole region's elevation (and, via the
existing OSM prefetch, its map data) ONCE into the global Meld cache, so generation runs
offline and is never rate-limited by per-cell API calls.

A pack is just files already in the shared cache:
  - elevation : <cache>/arnis-tile-cache/aws/z15_x{x}_y{y}.png   (Terrarium PNG, the exact
                format + path the Arnis fork reads — so a pre-pulled tile is a 100% cache hit)
  - osm       : <cache>/osm/osm_<hash>.json                       (baked by the existing prefetch)
  - manifest  : <cache>/datapacks/<region_id>.json                (what's downloaded, for the list)

Source: AWS Terrain Tiles (s3.amazonaws.com/elevation-tiles-prod/terrarium), a public AWS Open
Data bucket — the SAME source Arnis already uses per cell, just pulled in bulk ahead of time by
ONE controlled downloader (bounded concurrency + retries) instead of N parallel cells bursting it.

No third-party deps: stdlib urllib for the GETs, PIL (already present) only for the preview decode.
"""

from __future__ import annotations

import hashlib
import io
import json
import math
import os
import re
import shutil
import threading
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from .prefetch import aws_tile_cache_dir, meld_cache_root, meld_osm_cache_dir
from .survey import _lat_lng_to_tile

try:                                   # numpy is optional: only speeds up the overzoom re-encode
    import numpy as _np
except Exception:                      # noqa: BLE001
    _np = None

# The exact URL the Arnis fork fetches (aws_terrain.rs), so bytes + cache key match perfectly.
TERRARIUM_URL = "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png"
# Country/region bboxes clamp to z15 in Arnis; packs are z15.
PACK_ZOOM = 15
# Height preview renders cached z15 tiles when zoomed in, a sharp downsample of them at z13-14, and
# the NATIVE terrarium tile (one fetch each) for the far zoom-out — so you can pull all the way back
# to a regional/continental view (z6) and still see real terrain, not just the red coverage blocks.
PREVIEW_MIN_ZOOM = 6
# Polite, matches the per-tile downloader's intent.
_UA = "Meld/1.0 datapack (+https://github.com/Teddy563/meld)"
# Both our downloader and the Arnis fork only ever write a tile that DECODES, so presence == valid.
# Legit terrarium tiles vary wildly in size (uniform/ocean tiles PNG-compress to ~100-250 B, alpine
# tiles ~100 KB), so the only safe "missing" floor is the PNG minimum (~67 B = sig+IHDR+IDAT+IEND);
# anything at/above that is a real cached tile, anything below is absent/truncated junk.
_MIN_REAL_BYTES = 67
_TERRARIUM_OFFSET = 32768.0
# The AWS terrarium set has real GAPS at high zoom: in places it serves a ~270 B all-black tile that
# decodes to -32768 ("no data") at z14/z15, while the SAME spot has real elevation at z13 and below.
# Those holes are what show as dark bands in the preview AND as flat dips in-game. Anything decoding
# below this is a hole; we repair it by upsampling the deepest parent tile that actually has data.
_NODATA_BELOW = -32000.0
_OVERZOOM_FLOOR = 11               # don't climb past z11 (already a big footprint per tile)
_parent_img_cache: dict = {}       # (z,x,y) -> PIL.Image | None, so 16 siblings reuse one parent fetch
_parent_lock = threading.Lock()


# ── paths ────────────────────────────────────────────────────────────────────
def datapacks_dir() -> Path:
    d = meld_cache_root() / "datapacks"
    d.mkdir(parents=True, exist_ok=True)
    return d


def aws_tile_path(x: int, y: int, z: int = PACK_ZOOM) -> Path:
    return aws_tile_cache_dir() / f"z{z}_x{x}_y{y}.png"


# ── tile enumeration (matches Arnis web-mercator tiling via survey._lat_lng_to_tile) ──────
def tiles_for_bbox(bbox: dict, zoom: int = PACK_ZOOM) -> list[tuple[int, int]]:
    """Every (x, y) z-tile whose rectangle covers `bbox` {south,west,north,east}."""
    x1, y1 = _lat_lng_to_tile(bbox["north"], bbox["west"], zoom)   # NW corner -> min x, min y
    x2, y2 = _lat_lng_to_tile(bbox["south"], bbox["east"], zoom)   # SE corner -> max x, max y
    xlo, xhi = sorted((x1, x2))
    ylo, yhi = sorted((y1, y2))
    return [(x, y) for x in range(xlo, xhi + 1) for y in range(ylo, yhi + 1)]


def rings_bbox(rings) -> dict | None:
    """Bounding box of polygon rings [[(lat,lon),...], ...] or a single ring."""
    if not rings:
        return None
    if rings and isinstance(rings[0][0], (int, float)):   # single ring [(lat,lon),...]
        rings = [rings]
    lats, lons = [], []
    for ring in rings:
        for lat, lon in ring:
            lats.append(lat); lons.append(lon)
    if not lats:
        return None
    return {"south": min(lats), "west": min(lons), "north": max(lats), "east": max(lons)}


# ── coverage (pure disk, no network) ─────────────────────────────────────────
def _tile_cached(x: int, y: int, z: int = PACK_ZOOM) -> bool:
    p = aws_tile_path(x, y, z)
    try:
        return p.exists() and p.stat().st_size >= _MIN_REAL_BYTES
    except OSError:
        return False


_TILE_RE = re.compile(r"^z(\d+)_x(\d+)_y(\d+)\.png$")


def _cached_xy(zoom: int = PACK_ZOOM) -> set:
    """One scandir of the AWS tile cache -> set of (x,y) present at >= the PNG floor. Far faster
    than os.stat()-ing every expected tile (a country is ~534k tiles); scandir returns the size
    inline so there's no extra syscall per file."""
    have = set()
    try:
        with os.scandir(aws_tile_cache_dir()) as it:
            for ent in it:
                m = _TILE_RE.match(ent.name)
                if not m or int(m.group(1)) != zoom:
                    continue
                try:
                    if ent.stat().st_size >= _MIN_REAL_BYTES:
                        have.add((int(m.group(2)), int(m.group(3))))
                except OSError:
                    pass
    except FileNotFoundError:
        pass
    return have


def coverage_elevation(bbox: dict, zoom: int = PACK_ZOOM) -> dict:
    """How many of the region's z-tiles are already cached. missing[] is the redownload work order."""
    tiles = tiles_for_bbox(bbox, zoom)
    have = _cached_xy(zoom)
    missing = [(x, y) for (x, y) in tiles if (x, y) not in have]
    total = len(tiles)
    cached = total - len(missing)
    return {
        "zoom": zoom, "total": total, "cached": cached,
        "missing": [{"z": zoom, "x": x, "y": y} for (x, y) in missing],
        "pct": round(100.0 * cached / total, 1) if total else 100.0,
    }


# ── no-data overzoom: fill a z15/z14 terrarium hole from the deepest parent that HAS data ─────
def _img_is_nodata(im) -> bool:
    """True if a decoded terrarium tile is an all-no-data hole (every sample decodes to ~ -32768)."""
    px = im.load()
    w, h = im.size
    for j in range(0, h, 32):
        for i in range(0, w, 32):
            r, g, b = px[i, j]
            if (r * 256.0 + g + b / 256.0) - _TERRARIUM_OFFSET > _NODATA_BELOW:
                return False                          # one real sample -> not a hole
    return True


def _fetch_parent_img(z: int, x: int, y: int, timeout: float):
    """A decoded RGB tile (z<15 from S3, z15 from cache), memoised so 16 siblings share one fetch."""
    key = (z, x, y)
    with _parent_lock:
        if key in _parent_img_cache:
            return _parent_img_cache[key]
    from PIL import Image
    img = None
    try:
        if z == PACK_ZOOM:
            p = aws_tile_path(x, y, z)
            if p.exists() and p.stat().st_size >= _MIN_REAL_BYTES:
                img = Image.open(p).convert("RGB")
        else:
            url = TERRARIUM_URL.format(z=z, x=x, y=y)
            req = urllib.request.Request(url, headers={"User-Agent": _UA})
            with urllib.request.urlopen(req, timeout=timeout) as r:
                img = Image.open(io.BytesIO(r.read())).convert("RGB")
    except Exception:                                 # noqa: BLE001
        img = None
    with _parent_lock:
        _parent_img_cache[key] = img
    return img


def _overzoom_png(x: int, y: int, z: int = PACK_ZOOM, timeout: float = 30.0):
    """Build a valid terrarium PNG for a no-data hole by upsampling the deepest parent that has data.
    Decodes the parent's sub-region to heights, bilinear-upsamples, re-encodes to terrarium RGB (so
    the bytes are a real tile Arnis reads identically). Returns PNG bytes, or None if no parent helps."""
    from PIL import Image
    for zp in range(z - 1, _OVERZOOM_FLOOR - 1, -1):
        scale = 2 ** (z - zp)
        px, py = x // scale, y // scale
        pim = _fetch_parent_img(zp, px, py, timeout)
        if pim is None or _img_is_nodata(pim):
            continue
        sub = 256 // scale                            # this tile's pixel size inside the parent
        ox = (x - px * scale) * sub
        oy = (y - py * scale) * sub
        crop = pim.crop((ox, oy, ox + sub, oy + sub))
        if _np is not None:                           # vectorised decode -> resize -> re-encode
            a = _np.asarray(crop, dtype=_np.float64)
            hs = a[..., 0] * 256.0 + a[..., 1] + a[..., 2] / 256.0 - _TERRARIUM_OFFSET
            big = _np.asarray(Image.fromarray(hs.astype("float32"), "F").resize((256, 256), Image.BILINEAR),
                              dtype=_np.float64)
            e = _np.clip(big + _TERRARIUM_OFFSET, 0.0, 65535.99)
            ei = e.astype(_np.int32)
            rgb = _np.dstack([((ei >> 8) & 0xFF).astype(_np.uint8),
                              (ei & 0xFF).astype(_np.uint8),
                              (((e - ei) * 256).astype(_np.int32) & 0xFF).astype(_np.uint8)])
            out = Image.fromarray(rgb, "RGB")
        else:                                         # pure-PIL fallback (no numpy)
            hf = Image.new("F", (sub, sub)); hp = hf.load(); cp = crop.load()
            for j in range(sub):
                for i in range(sub):
                    r, g, b = cp[i, j]
                    hp[i, j] = (r * 256.0 + g + b / 256.0) - _TERRARIUM_OFFSET
            hf = hf.resize((256, 256), Image.BILINEAR); hp = hf.load()
            out = Image.new("RGB", (256, 256)); op = out.load()
            for j in range(256):
                for i in range(256):
                    e = max(0.0, min(65535.99, hp[i, j] + _TERRARIUM_OFFSET)); ei = int(e)
                    op[i, j] = ((ei >> 8) & 0xFF, ei & 0xFF, int((e - ei) * 256) & 0xFF)
        buf = io.BytesIO(); out.save(buf, "PNG")
        return buf.getvalue()
    return None


# ── bulk download (one controlled process, bounded concurrency, atomic, verified) ────────
def _fetch_one(x: int, y: int, z: int, timeout: float, force: bool = False) -> str:
    """Download one tile -> cache. Returns 'ok' | 'skip' | 'absent' | 'fail'.
    force=True re-downloads even if a (possibly stale/flat) tile is already cached."""
    dst = aws_tile_path(x, y, z)
    if not force and _tile_cached(x, y, z):
        return "skip"
    url = TERRARIUM_URL.format(z=z, x=x, y=y)
    req = urllib.request.Request(url, headers={"User-Agent": _UA})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            data = r.read()
    except urllib.error.HTTPError as e:
        return "absent" if e.code in (403, 404) else "fail"   # off-grid tiles legitimately 403/404
    except Exception:
        return "fail"
    if not data or len(data) < 67:                            # smaller than a PNG header = junk
        return "fail"
    # Verify it decodes (catches truncation) before committing it to the cache.
    try:
        from PIL import Image
        Image.open(io.BytesIO(data)).verify()
    except Exception:
        return "fail"
    # S3 serves all-black no-data tiles where the terrarium set has high-zoom gaps. Bake an overzoom
    # from the deepest parent that has data so the cached tile is real terrain, not a flat dip.
    if z > _OVERZOOM_FLOOR:
        try:
            if _img_is_nodata(Image.open(io.BytesIO(data)).convert("RGB")):
                fixed = _overzoom_png(x, y, z, timeout)
                if fixed is not None:
                    data = fixed
        except Exception:                                     # noqa: BLE001
            pass
    dst.parent.mkdir(parents=True, exist_ok=True)
    tmp = dst.with_suffix(f".png.tmp{os.getpid()}.{threading.get_ident()}")
    try:
        tmp.write_bytes(data)
        os.replace(tmp, dst)                                  # atomic: no half-written tiles
    except Exception:
        try: tmp.unlink(missing_ok=True)
        except Exception: pass
        return "fail"
    return "ok"


def download_tiles(tiles: list[tuple[int, int]], on_progress=None, *, zoom: int = PACK_ZOOM,
                   concurrency: int = 16, retries: int = 2, timeout: float = 30.0,
                   force: bool = False, should_stop=None) -> dict:
    """Download `tiles` into the cache. Bounded concurrency keeps us well under any S3 throttle.
    on_progress(done, total, ok, skip, absent, fail) is called as tiles complete."""
    total = len(tiles)
    counts = {"ok": 0, "skip": 0, "absent": 0, "fail": 0}
    done = [0]
    lock = threading.Lock()

    def work(tile):
        if should_stop and should_stop():
            return "fail"
        x, y = tile
        res = _fetch_one(x, y, zoom, timeout, force=force)
        if res != "fail":                                     # terminal outcome (count once)
            with lock:
                counts[res] += 1
                done[0] += 1
                if on_progress:
                    on_progress(done[0], total, counts["ok"], counts["skip"], counts["absent"], counts["fail"])
        return res

    pending = list(tiles)
    for attempt in range(retries + 1):
        if not pending:
            break
        with ThreadPoolExecutor(max_workers=max(1, min(32, concurrency))) as ex:
            results = list(ex.map(work, pending))
        pending = [t for t, res in zip(pending, results) if res == "fail"]
        if pending and attempt < retries:
            time.sleep(1.0 + attempt)                         # brief backoff before retrying failures

    counts["fail"] = len(pending)
    counts["total"] = total
    return counts


def repair_nodata(tiles: list[tuple[int, int]], on_progress=None, *, zoom: int = PACK_ZOOM,
                  concurrency: int = 8, timeout: float = 30.0, should_stop=None,
                  size_skip: int = 4096) -> dict:
    """Scan already-cached tiles; any that decode to an all-no-data hole get overzoom-baked in place
    from the deepest parent that has data. Doesn't touch good tiles or re-download anything good.
    A no-data hole is a uniform black tile, so it PNG-compresses tiny — tiles bigger than `size_skip`
    can't be holes and are skipped without decoding (huge speed-up for a whole-cache scan).
    on_progress(done, total, fixed, unfixable) as tiles complete."""
    from PIL import Image
    total = len(tiles)
    counts = {"checked": 0, "fixed": 0, "unfixable": 0, "absent": 0, "ok": 0}
    done = [0]
    lock = threading.Lock()

    def work(t):
        if should_stop and should_stop():
            return
        x, y = t
        p = aws_tile_path(x, y, zoom)
        out = "ok"
        try:
            sz = p.stat().st_size if p.exists() else 0
            if sz < _MIN_REAL_BYTES:
                out = "absent"
            elif size_skip and sz > size_skip:
                out = "ok"                                    # too big to be a uniform hole; skip decode
            elif _img_is_nodata(Image.open(p).convert("RGB")):
                fixed = _overzoom_png(x, y, zoom, timeout)
                if fixed is not None:
                    tmp = p.with_suffix(f".png.tmp{os.getpid()}.{threading.get_ident()}")
                    tmp.write_bytes(fixed); os.replace(tmp, p)
                    out = "fixed"
                else:
                    out = "unfixable"
        except Exception:                                     # noqa: BLE001
            out = "ok"
        with lock:
            counts["checked"] += 1
            counts[out] = counts.get(out, 0) + 1
            done[0] += 1
            if on_progress:
                on_progress(done[0], total, counts["fixed"], counts["unfixable"])

    with ThreadPoolExecutor(max_workers=max(1, min(16, concurrency))) as ex:
        list(ex.map(work, tiles))
    counts["total"] = total
    return counts


# ── Terrarium decode + height-preview rendering ──────────────────────────────
def _decode_heights(path: Path):
    """Return (PIL RGB image, width, height) for a cached terrarium tile, or None."""
    try:
        from PIL import Image
        return Image.open(path).convert("RGB")
    except Exception:
        return None


def _red_tile() -> bytes:
    """A translucent red 256x256 tile = 'no elevation here' (hole flag on the map)."""
    from PIL import Image
    img = Image.new("RGBA", (256, 256), (220, 60, 60, 90))
    buf = io.BytesIO(); img.save(buf, "PNG"); return buf.getvalue()


def render_preview_tile(x: int, y: int, z: int = PACK_ZOOM, *, lo: float | None = None,
                        hi: float | None = None, mode: str = "grayscale") -> bytes:
    """Decode a cached terrarium tile into a grayscale/hillshade preview PNG. Missing -> red tile."""
    from PIL import Image
    path = aws_tile_path(x, y, z)
    if not path.exists() or path.stat().st_size < _MIN_REAL_BYTES:
        return _red_tile()
    src = _decode_heights(path)
    if src is None:
        return _red_tile()
    w, h = src.size
    px = src.load()

    def height_at(ix, iy):
        r, g, b = px[ix, iy]
        return (r * 256.0 + g + b / 256.0) - _TERRARIUM_OFFSET

    # auto range when not pinned
    if lo is None or hi is None:
        vals = [height_at(ix, iy) for iy in range(0, h, 16) for ix in range(0, w, 16)]
        amin, amax = (min(vals), max(vals)) if vals else (0.0, 1.0)
        lo = amin if lo is None else lo
        hi = amax if hi is None else hi
    span = (hi - lo) or 1.0

    out = Image.new("RGBA", (w, h))
    op = out.load()
    if mode == "hillshade":
        for iy in range(h):
            for ix in range(w):
                hl = height_at(max(0, ix - 1), iy)
                hr = height_at(min(w - 1, ix + 1), iy)
                hu = height_at(ix, max(0, iy - 1))
                hd = height_at(ix, min(h - 1, iy + 1))
                dzdx = (hr - hl); dzdy = (hd - hu)
                shade = max(0.0, min(1.0, 0.6 + 0.18 * (dzdx + dzdy)))
                base = max(0, min(255, int((height_at(ix, iy) - lo) / span * 255)))
                v = max(0, min(255, int(base * (0.5 + 0.5 * shade))))
                op[ix, iy] = (v, v, v, 170)
    else:  # grayscale
        for iy in range(h):
            for ix in range(w):
                v = max(0, min(255, int((height_at(ix, iy) - lo) / span * 255)))
                op[ix, iy] = (v, v, v, 150)
    buf = io.BytesIO(); out.save(buf, "PNG"); return buf.getvalue()


def _preview_cache_dir(mode: str) -> Path:
    return meld_cache_root() / "preview" / mode


def clear_preview_cache() -> None:
    """Drop rendered preview tiles so a fresh download's tiles show on the next preview."""
    try:
        shutil.rmtree(meld_cache_root() / "preview", ignore_errors=True)
    except Exception:
        pass


def _render_overview(z: int, x: int, y: int, lo, hi, mode: str) -> bytes:
    """Downsample the z15 tiles under (z,x,y) into ONE 256px tile so the height map is visible when
    zoomed out: each underlying z15 tile contributes one cell (its mean height); a missing one is
    red. Normalized by the passed global lo/hi so flat tiles read as their true gray, not black."""
    from PIL import Image
    factor = 2 ** (PACK_ZOOM - z)
    x0, y0 = x * factor, y * factor
    grid = [[None] * factor for _ in range(factor)]
    vals = []
    for gy in range(factor):
        for gx in range(factor):
            p = aws_tile_path(x0 + gx, y0 + gy)
            try:
                if not p.exists() or p.stat().st_size < _MIN_REAL_BYTES:
                    continue
                r, g, b = Image.open(p).convert("RGB").resize((1, 1)).getpixel((0, 0))
                h = (r * 256.0 + g + b / 256.0) - _TERRARIUM_OFFSET
                grid[gy][gx] = h
                vals.append(h)
            except Exception:
                pass
    if not vals:
        return _red_tile()
    if lo is None:
        lo = min(vals)
    if hi is None:
        hi = max(vals)
    span = (hi - lo) or 1.0
    cell = max(1, 256 // factor)
    out = Image.new("RGBA", (256, 256), (0, 0, 0, 0))
    op = out.load()
    for gy in range(factor):
        for gx in range(factor):
            h = grid[gy][gx]
            if h is None:
                col = (224, 80, 74, 95)
            else:
                v = max(0, min(255, int((h - lo) / span * 255)))
                col = (v, v, v, 150)
            for py in range(gy * cell, min(256, (gy + 1) * cell)):
                for px in range(gx * cell, min(256, (gx + 1) * cell)):
                    op[px, py] = col
    buf = io.BytesIO()
    out.save(buf, "PNG")
    return buf.getvalue()


# ── numpy-based smooth renderer: full 256px detail at every zoom (no 1-mean-per-tile blocks) ─────
def _heights_from_img(img):
    """RGB terrarium image -> (heights float ndarray, nodata bool mask). numpy required."""
    a = _np.asarray(img, dtype=_np.float64)
    h = a[..., 0] * 256.0 + a[..., 1] + a[..., 2] / 256.0 - _TERRARIUM_OFFSET
    return h, (h < _NODATA_BELOW)


def _render_height_array(h, missing, lo: float, hi: float, mode: str) -> bytes:
    """Render a 256x256 height array to a preview PNG, normalized to [lo,hi]. NaN/missing -> red."""
    from PIL import Image
    span = (hi - lo) or 1.0
    hh = _np.nan_to_num(h, nan=lo)
    if mode == "hillshade":
        # np.gradient uses ONE-SIDED differences at the array edges instead of a zeroed seam column,
        # so the tile's border pixels shade like their neighbours -> no bright/dark line every 256px.
        dzdy, dzdx = _np.gradient(hh)
        shade = _np.clip(0.6 + 0.36 * (dzdx + dzdy), 0.0, 1.0)
        base = _np.clip((hh - lo) / span * 255.0, 0, 255)
        v = _np.clip(base * (0.5 + 0.5 * shade), 0, 255).astype(_np.uint8); alpha = 170
    else:
        v = _np.clip((hh - lo) / span * 255.0, 0, 255).astype(_np.uint8); alpha = 150
    rgba = _np.dstack([v, v, v, _np.full(hh.shape, alpha, _np.uint8)])
    if missing is not None:
        rgba[missing] = (224, 80, 74, 95)                      # translucent red = no data here
    out = Image.fromarray(rgba, "RGBA")
    buf = io.BytesIO(); out.save(buf, "PNG"); return buf.getvalue()


def _fetch_source_tile(z: int, x: int, y: int, pack_zoom: int = PACK_ZOOM, timeout: float = 20.0):
    """Decoded RGB terrarium tile at zoom z for the preview. At the PACK zoom it reads the (repaired)
    cache — the exact tiles Arnis generates from; other zooms are pulled from S3 and disk-cached under
    preview-src so panning stays fast. Returns a PIL RGB image, or None."""
    from PIL import Image
    if z == pack_zoom:
        p = aws_tile_path(x, y, z)
        try:
            if p.exists() and p.stat().st_size >= _MIN_REAL_BYTES:
                return Image.open(p).convert("RGB")
        except Exception:                                      # noqa: BLE001
            return None
        return None
    sdir = meld_cache_root() / "preview-src"
    sp = sdir / f"z{z}_x{x}_y{y}.png"
    try:
        if sp.exists() and sp.stat().st_size >= _MIN_REAL_BYTES:
            return Image.open(sp).convert("RGB")
    except Exception:                                          # noqa: BLE001
        pass
    try:
        url = TERRARIUM_URL.format(z=z, x=x, y=y)
        req = urllib.request.Request(url, headers={"User-Agent": _UA})
        with urllib.request.urlopen(req, timeout=timeout) as r:
            data = r.read()
        Image.open(io.BytesIO(data)).verify()
        sdir.mkdir(parents=True, exist_ok=True)
        tmp = sp.with_suffix(f".png.tmp{os.getpid()}.{threading.get_ident()}")
        tmp.write_bytes(data); os.replace(tmp, sp)
        return Image.open(io.BytesIO(data)).convert("RGB")
    except Exception:                                          # noqa: BLE001
        return None


def _render_native(z: int, x: int, y: int, lo, hi, mode: str, pack_zoom: int = PACK_ZOOM) -> bytes:
    """Render the native terrarium tile at zoom z directly — full 256px detail, smooth, one fetch per
    tile. Used for the far zoom-out (z below the pack zoom). No-data -> red."""
    img = _fetch_source_tile(z, x, y, pack_zoom)
    if img is None:
        return _red_tile()
    h, nod = _heights_from_img(img)
    if bool(nod.all()):
        return _red_tile()
    return _render_height_array(h, nod if nod.any() else None, lo, hi, mode)


def _render_from_pack(z: int, x: int, y: int, lo, hi, mode: str, pack_zoom: int) -> bytes:
    """Render a preview tile AT or ABOVE the pack zoom from the (repaired) cached pack tile: z==pack
    renders the whole cached tile; z>pack crops its sub-region and upsamples (smooth, what Arnis also
    interpolates). Reads the cache so it reflects repairs + works offline. No-data/missing -> red."""
    from PIL import Image
    up = z - pack_zoom                                        # >= 0
    f = 1 << up
    pxp, pyp = x // f, y // f
    img = _fetch_source_tile(pack_zoom, pxp, pyp, pack_zoom)
    if img is None:
        return _red_tile()
    h, nod = _heights_from_img(img)
    if bool(nod.all()):
        return _red_tile()
    if up == 0:
        return _render_height_array(h, nod if nod.any() else None, lo, hi, mode)
    sub = 256 >> up
    ox, oy = (x - pxp * f) * sub, (y - pyp * f) * sub
    hcrop = h[oy:oy + sub, ox:ox + sub]
    big = _np.asarray(Image.fromarray(hcrop.astype("float32"), "F").resize((256, 256), Image.BILINEAR),
                      dtype=_np.float64)
    return _render_height_array(big, None, lo, hi, mode)


def _render_overview_hd(z: int, x: int, y: int, lo, hi, mode: str, pack_zoom: int = PACK_ZOOM) -> bytes:
    """Non-pixelated overview just below the pack zoom from the (repaired) cached pack tiles. Assembles
    the full-res height MOSAIC (factor*256 px) first, then does ONE bilinear downsample to 256 — so
    sub-tile boundaries blend instead of leaving a seam. Hole-free + offline. Missing -> red region."""
    from PIL import Image
    factor = 2 ** (pack_zoom - z)                             # 2 or 4 (kept small by the caller)
    full = factor * 256
    big = _np.full((full, full), _np.nan)
    miss = _np.zeros((full, full), dtype=bool)
    any_data = False
    for gy in range(factor):
        for gx in range(factor):
            p = aws_tile_path(x * factor + gx, y * factor + gy, pack_zoom)
            y0, x0 = gy * 256, gx * 256
            placed = False
            try:
                if p.exists() and p.stat().st_size >= _MIN_REAL_BYTES:
                    hsub, nod = _heights_from_img(Image.open(p).convert("RGB"))
                    big[y0:y0 + 256, x0:x0 + 256] = hsub
                    if nod.any():
                        miss[y0:y0 + 256, x0:x0 + 256] = nod
                    placed = True; any_data = True
            except Exception:                                  # noqa: BLE001
                placed = False
            if not placed:
                miss[y0:y0 + 256, x0:x0 + 256] = True
    if not any_data:
        return _red_tile()
    # one resize over the whole mosaic -> continuous across former sub-tile edges (fill NaN so the
    # bilinear pass doesn't smear holes, then re-mark the holes red from the nearest-resized mask).
    fill = float(_np.nanmean(big)) if bool(_np.isnan(big).any()) else 0.0
    filled = _np.where(_np.isnan(big), fill, big).astype("float32")
    small = _np.asarray(Image.fromarray(filled, "F").resize((256, 256), Image.BILINEAR), dtype=_np.float64)
    miss_small = _np.asarray(
        Image.fromarray((miss.astype("uint8") * 255), "L").resize((256, 256), Image.NEAREST)) > 127
    return _render_height_array(small, miss_small if miss_small.any() else None, lo, hi, mode)


def render_tile(z: int, x: int, y: int, lo=None, hi=None, mode: str = "grayscale",
                pack_zoom: int = PACK_ZOOM) -> bytes:
    """Preview tile for the map, full 256px detail relative to the chosen PACK zoom (the zoom Arnis
    generates from): at/above pack zoom it renders the cached pack tile (cropped+upsampled when zoomed
    past it); just below, a sharp seam-free downsample of the cached pack tiles; far below, the native
    terrarium tile (1 fetch). Disk-cached per (z,x,y,pack,mode,lo,hi) so packs at different zooms and
    contrast ranges never collide and panning stays fast."""
    if lo is None:
        lo = -100.0
    if hi is None:
        hi = 3000.0
    cdir = _preview_cache_dir(mode)
    sig = f"p{pack_zoom}_{int(round(lo))}_{int(round(hi))}"     # pack zoom + contrast in the key
    cpath = cdir / f"z{z}_x{x}_y{y}_{sig}.png"
    try:
        if cpath.exists() and cpath.stat().st_size > 0:
            return cpath.read_bytes()
    except OSError:
        pass
    if _np is None:                                            # safe degrade to the original blocky path
        if z == pack_zoom:
            png = render_preview_tile(x, y, z, lo=lo, hi=hi, mode=mode)
        elif pack_zoom - 3 < z < pack_zoom:
            png = _render_overview(z, x, y, lo, hi, mode)
        else:
            return _red_tile()
    elif z >= pack_zoom:
        png = _render_from_pack(z, x, y, lo, hi, mode, pack_zoom)   # cached pack tile (upsample if z>pack)
    elif pack_zoom - z <= 2:
        png = _render_overview_hd(z, x, y, lo, hi, mode, pack_zoom)  # seam-free downsample of cache
    elif z >= PREVIEW_MIN_ZOOM:
        png = _render_native(z, x, y, lo, hi, mode, pack_zoom)      # native low-zoom terrarium tile
    else:
        return _red_tile()
    try:
        cdir.mkdir(parents=True, exist_ok=True)
        tmp = cpath.with_suffix(f".png.tmp{os.getpid()}")
        tmp.write_bytes(png)
        os.replace(tmp, cpath)
    except Exception:
        pass
    return png


def _tile_bounds_ll(x: int, y: int, z: int = PACK_ZOOM) -> dict:
    """Web-mercator tile -> its lat/lon bbox."""
    n = 2 ** z
    lon1 = x / n * 360 - 180
    lon2 = (x + 1) / n * 360 - 180
    lat1 = math.degrees(math.atan(math.sinh(math.pi * (1 - 2 * y / n))))
    lat2 = math.degrees(math.atan(math.sinh(math.pi * (1 - 2 * (y + 1) / n))))
    return {"south": min(lat1, lat2), "west": min(lon1, lon2),
            "north": max(lat1, lat2), "east": max(lon1, lon2)}


def tile_info(x: int, y: int, z: int = PACK_ZOOM) -> dict:
    """Everything about one elevation tile for the click popup: cached?, size, decoded height
    min/max/mean, whether it's flat (suspect), and its lat/lon bbox."""
    p = aws_tile_path(x, y, z)
    info = {"z": z, "x": x, "y": y, "bbox": _tile_bounds_ll(x, y, z), "present": False,
            "size_bytes": 0, "min_m": None, "max_m": None, "mean_m": None, "flat": False,
            "url": TERRARIUM_URL.format(z=z, x=x, y=y)}
    try:
        if p.exists():
            info["size_bytes"] = p.stat().st_size
            info["present"] = info["size_bytes"] >= _MIN_REAL_BYTES
    except OSError:
        pass
    if info["present"]:
        try:
            from PIL import Image
            px = Image.open(p).convert("RGB").load()
            lo, hi, s, n = 1e9, -1e9, 0.0, 0
            for yy in range(0, 256, 8):
                for xx in range(0, 256, 8):
                    r, g, b = px[xx, yy]
                    h = (r * 256.0 + g + b / 256.0) - _TERRARIUM_OFFSET
                    lo = min(lo, h); hi = max(hi, h); s += h; n += 1
            if n:
                info["min_m"] = round(lo, 1); info["max_m"] = round(hi, 1)
                info["mean_m"] = round(s / n, 1); info["flat"] = (hi - lo) < 2
        except Exception:
            pass
    return info


# ── manifest (the "what's downloaded" list, reusable across every project) ────────────────
def region_id(bbox: dict, name: str = "") -> str:
    canon = f"{bbox['south']:.4f},{bbox['west']:.4f},{bbox['north']:.4f},{bbox['east']:.4f}|{name.strip().lower()}"
    return hashlib.sha1(canon.encode("utf-8")).hexdigest()[:16]


def write_manifest(rid: str, *, name: str, bbox: dict, cov: dict, polygons=None,
                   osm: dict | None = None) -> dict:
    man = {
        "region_id": rid, "name": name or "region", "bbox": bbox,
        "zoom": cov.get("zoom", PACK_ZOOM),
        "polygons": polygons, "updated": int(time.time()),
        "elevation": {"total": cov.get("total"), "cached": cov.get("cached"), "pct": cov.get("pct")},
        "osm": osm or {},
    }
    (datapacks_dir() / f"{rid}.json").write_text(json.dumps(man, indent=2), encoding="utf-8")
    return man


def list_packs() -> list[dict]:
    out = []
    for f in sorted(datapacks_dir().glob("*.json")):
        try:
            man = json.loads(f.read_text(encoding="utf-8"))
        except Exception:
            continue
        # Re-validate elevation coverage against disk so the list reflects reality, not the snapshot.
        bbox = man.get("bbox")
        if bbox:
            cov = coverage_elevation(bbox, zoom=int(man.get("zoom", PACK_ZOOM)))
            man["elevation"] = {"total": cov["total"], "cached": cov["cached"], "pct": cov["pct"]}
        out.append(man)
    return out


# ── drop-in folder import (offline / shared packs) ───────────────────────────
def import_pack_folder(src_dir: str, *, link: bool = True, log=None) -> dict:
    """Bring an external folder of pack files into the global cache. Accepts a folder that
    contains z*_x*_y*.png tiles (anywhere under it) and/or osm_*.json files. Hardlinks by default
    (no 2x disk), copies if linking fails (cross-device)."""
    src = Path(src_dir).expanduser()
    if not src.is_dir():
        return {"ok": False, "error": f"not a folder: {src}"}
    aws_dir = aws_tile_cache_dir(); aws_dir.mkdir(parents=True, exist_ok=True)
    osm_dir = meld_osm_cache_dir(); osm_dir.mkdir(parents=True, exist_ok=True)
    tiles = 0; osm = 0
    for p in src.rglob("*"):
        if not p.is_file():
            continue
        nm = p.name
        if nm.startswith("z") and nm.endswith(".png") and "_x" in nm and "_y" in nm:
            dst = aws_dir / nm
        elif nm.startswith("osm_") and nm.endswith(".json"):
            dst = osm_dir / nm
        else:
            continue
        if dst.exists():
            continue
        try:
            if link:
                os.link(p, dst)
            else:
                shutil.copy2(p, dst)
        except OSError:
            try:
                shutil.copy2(p, dst)
            except Exception:
                continue
        if dst.parent == aws_dir: tiles += 1
        else: osm += 1
    if log:
        log(f"  [Datapack] imported {tiles} tiles + {osm} osm files from {src}")
    return {"ok": True, "tiles": tiles, "osm": osm}
