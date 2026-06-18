"""
prefetch.py — download a selection's OSM data ONCE and share it to every cell.

WHY
    Each Arnis instance normally queries Overpass for its own bbox. When Meld runs
    many cells in parallel they collide on Overpass's per-IP rate limit (~2 slots),
    so cells 3+ stall in retry loops. The bottleneck is CONCURRENCY, not size: a
    single serial request for a large area succeeds fine.

STRATEGY (top-down adaptive)
    1. Try ONE Overpass request covering the whole planned area (+ a margin).
    2. If that request fails (rate limit / timeout / too big), split the CELLS into
       four quadrants and retry each as its own request. Recurse until each piece
       succeeds or we reach a single cell (depth cap).
    3. Cache every successful chunk's JSON on disk, keyed by bbox. Re-runs reuse it.
    4. Map each planned cell to the chunk file that fully covers its seam-expanded
       bbox, and hand that file to the cell via `--file` so generation makes ZERO
       Overpass calls.

    Splits follow CELL boundaries (never cut a cell), so each cell is always fully
    inside exactly one chunk. Requests run SERIALLY, so they never collide.

    The fork's `--download-only` flag does the actual fetch: it saves the Overpass
    response to a file and exits before any world generation.
"""

from __future__ import annotations

import hashlib
import os
import re
import subprocess
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from . import osm_grid
from .constants import METERS_PER_DEG_LAT
from .coords import cell_bbox, expand_bbox_for_seam, mpd_lon

# A download attempt that runs longer than this is treated as "too big" and split.
# Try the WHOLE area first; only after 5 minutes (or a hard failure) do we tile it down.
_DOWNLOAD_TIMEOUT_S = 300
# Don't subdivide forever: a single cell is the smallest unit we fetch.
_MAX_DEPTH = 6
# How many OSM tiles may download at once. The public Overpass API allows 2 concurrent slots
# per IP, so 2 is the safe default (halves prefetch time without tripping the per-IP limit). A
# private/self-hosted endpoint can go higher; capped here so a stray setting can't hammer Overpass.
_MAX_PREFETCH_CONCURRENCY = 4
# Strings in Arnis output that confirm a split-worthy failure (vs a hard network error).
_SPLIT_MARKERS = ("rate limited", "timed out", "request timeout", "maxsize",
                  "too many", "timeout", "out of memory", "server overloaded")
# TRANSIENT failures worth a retry-with-backoff (vs "area too big" / maxsize which need a smaller
# bbox, not a retry). Retrying these lets a throttled tile succeed + cache, killing the per-run tail.
_RETRY_MARKERS = ("rate limited", "timed out", "request timeout", "timeout", "server overloaded")
# Tiling unit = REAL-WORLD km² (an Overpass query is real-world, so this is scale-independent:
# Romania is the same area of OSM at 1:1 or 1:10). Auto downloads the WHOLE selection as ONE
# query unless it exceeds this safe single-query cap, then quadrant-splits it — and the reactive
# split on a server rejection shrinks any tile that's still too dense. This is why a few big tiles
# work even on a downscaled (1:10) world: the tile size tracks ground area, NOT the cell count.
# ~30,000 km² (≈170 km across) is a query a public Overpass handles for typical/rural density;
# denser tiles get rejected and split automatically. The advanced slider overrides the cap.
_AUTO_MAX_QUERY_KM2 = 30000.0


def _margin_deg(meters: float, lat: float) -> tuple[float, float]:
    """A metre margin as (d_lat, d_lon) at a given latitude."""
    if meters <= 0:
        return 0.0, 0.0
    mlon = mpd_lon(lat) or METERS_PER_DEG_LAT
    return meters / METERS_PER_DEG_LAT, meters / mlon


def _bbox_area_km2(bbox: dict) -> float:
    """Real-world ground area of a bbox in km² (longitude scaled at the bbox mid-latitude)."""
    lat_mid = (bbox["south"] + bbox["north"]) / 2.0
    w_m = abs(bbox["east"] - bbox["west"]) * (mpd_lon(lat_mid) or METERS_PER_DEG_LAT)
    h_m = abs(bbox["north"] - bbox["south"]) * METERS_PER_DEG_LAT
    return (w_m * h_m) / 1_000_000.0


def _resolve_tile_budget(settings: dict, total_area_km2: float = 0.0) -> float:
    """Max real-world km² per Overpass tile.

    settings['prefetch_tile_km2'] > 0 is a manual cap (the advanced slider). 0/None/'auto' means
    AUTO: download the whole selection in ONE query unless it's bigger than the safe single-query
    cap, in which case cap it (the planner then quadrant-splits down to that size). So a small
    area is one request; a country is a handful of big tiles — both scale-independent, because
    total_area_km2 is real-world ground area (same at 1:1 and 1:10). Density still gets the last
    word via the reactive split in fetch_group when the server rejects a tile."""
    try:
        v = float(settings.get("prefetch_tile_km2", 0) or 0)
    except (TypeError, ValueError):
        v = 0.0
    if v > 0:
        return v
    if total_area_km2 and total_area_km2 > 0:
        return min(total_area_km2, _AUTO_MAX_QUERY_KM2)
    return _AUTO_MAX_QUERY_KM2


def _plan_clumps(work: list[dict], budget_km2: float,
                 max_depth: int = _MAX_DEPTH) -> list[list[dict]]:
    """Pre-split the cells into tiles whose seam-expanded union stays under budget_km2.

    Splits by region quadrant (via _split_cells, which never cuts a cell), so every cell ends up
    fully inside exactly one tile and the --file coverage invariant is preserved untouched. A
    single cell is the floor: if one cell alone exceeds the budget (huge real area at small scale)
    it still goes out as its own tile rather than being cut. Deterministic, no network."""
    out: list[list[dict]] = []
    stack = [(work, 0)]
    while stack:
        group, depth = stack.pop()
        if not group:
            continue
        union = _union([g["expanded"] for g in group])
        if len(group) <= 1 or depth >= max_depth or _bbox_area_km2(union) <= budget_km2:
            out.append(group)
        else:
            stack.extend((sub, depth + 1) for sub in _split_cells(group))
    return out


def _union(bboxes: list[dict]) -> dict:
    return {
        "south": min(b["south"] for b in bboxes),
        "west":  min(b["west"]  for b in bboxes),
        "north": max(b["north"] for b in bboxes),
        "east":  max(b["east"]  for b in bboxes),
    }


def _bbox_key(bbox: dict, extra: str) -> str:
    canon = f"{bbox['south']:.7f},{bbox['west']:.7f},{bbox['north']:.7f},{bbox['east']:.7f}|{extra}"
    return hashlib.sha1(canon.encode()).hexdigest()[:16]


def _looks_complete(path: Path) -> bool:
    """A whole Overpass response is a JSON object, so the last non-space byte must be
    '}'. Guards against a write that exited 0 but was truncated."""
    try:
        size = path.stat().st_size
        with open(path, "rb") as f:
            f.seek(max(0, size - 64))
            return f.read().rstrip().endswith(b"}")
    except Exception:
        return False


def _split_cells(cells: list[dict]) -> list[list[dict]]:
    """Split a cell group into up to 4 quadrants by region index. Never cuts a cell.
    A single row/column naturally collapses to a 2-way split; identical-position is
    impossible (cell keys are unique), so a group of >1 always reduces."""
    rxs = [c["rx"] for c in cells]
    rzs = [c["rz"] for c in cells]
    rx_split = (min(rxs) + max(rxs) + 1) // 2
    rz_split = (min(rzs) + max(rzs) + 1) // 2
    buckets: dict[tuple[int, int], list[dict]] = {}
    for c in cells:
        q = (0 if c["rx"] < rx_split else 1, 0 if c["rz"] < rz_split else 1)
        buckets.setdefault(q, []).append(c)
    return [g for g in buckets.values() if g]


def _download_one(exe: str, bbox: dict, out_json: Path, overpass_url: list[str],
                  log, retries: int = 3) -> tuple[bool, str]:
    """Run `arnis --download-only` for one bbox. Returns (ok, reason).

    Retries a few times with backoff on TRANSIENT failures (rate-limited / timeout / overloaded), so a
    throttled tile succeeds and gets CACHED instead of being re-fetched live on every future run (the
    per-run gap tail). Non-transient reasons (area too big, hard exit) return immediately so the caller
    can split or fall back to live. On success the tile is atomically published into the shared cache."""
    # Write to a per-PID temp then os.replace into place, so a reader in another project/process
    # sharing the GLOBAL cache never sees a half-written osm_<hash>.json (atomic publish).
    tmp = out_json.with_name(f"{out_json.stem}.{os.getpid()}.tmp")
    cmd = [
        str(exe),
        "--bbox", f"{bbox['south']},{bbox['west']},{bbox['north']},{bbox['east']}",
        "--save-json-file", str(tmp),
        "--download-only",
    ]
    if overpass_url:
        cmd += ["--overpass-url", ",".join(overpass_url)]
    reason = "exit"
    for attempt in range(max(1, retries)):
        try:
            proc = subprocess.run(cmd, capture_output=True, text=True,
                                  timeout=_DOWNLOAD_TIMEOUT_S, encoding="utf-8", errors="replace")
        except subprocess.TimeoutExpired:
            reason = "timed out"
        except Exception as ex:  # noqa: BLE001
            tmp.unlink(missing_ok=True)
            return False, f"spawn error: {ex}"
        else:
            output = (proc.stdout or "") + (proc.stderr or "")
            ok_file = proc.returncode == 0 and tmp.exists() and tmp.stat().st_size > 2
            if ok_file and _looks_complete(tmp):
                try:
                    os.replace(tmp, out_json)      # atomic publish into the shared cache
                    return True, "ok"
                except Exception as ex:  # noqa: BLE001
                    tmp.unlink(missing_ok=True)
                    return False, f"publish error: {ex}"
            tmp.unlink(missing_ok=True)
            if ok_file:                            # exited 0 with content but not a complete JSON
                return False, "truncated json"
            low = output.lower()
            reason = next((m for m in _SPLIT_MARKERS if m in low), None) or f"exit {proc.returncode}"
        # Retry only TRANSIENT throttle/timeout reasons (not "too big" — that needs a smaller bbox).
        if attempt < retries - 1 and reason in _RETRY_MARKERS:
            wait = 2 * (attempt + 1)
            log(f"  [Prefetch] tile fetch '{reason}' — retry {attempt + 1}/{retries - 1} in {wait}s…")
            time.sleep(wait)
            continue
        return False, reason
    return False, reason


def preview_union(cells, origin, settings) -> dict | None:
    """The top-level chunk Meld tries first: union of the cells' seam-expanded bboxes
    plus the building margin. For the UI overlay before generation starts. Returns
    None if there are no cells or no origin."""
    olat, olon = origin.get("lat"), origin.get("lon")
    if olat is None or olon is None or not cells:
        return None
    scale = float(settings.get("scale", 1.0) or 1.0)
    seam = int(settings.get("seam_buffer_chunks", 8) or 0)
    margin_m = float(settings.get("prefetch_margin_m", 256) or 0)
    expanded = []
    keys = []
    for c in cells:
        try:
            rx, rz, size = (int(x) for x in c["cell_key"].split(","))
        except (ValueError, KeyError):
            continue
        base = cell_bbox(rx, rz, size, olat, olon, scale)
        expanded.append(expand_bbox_for_seam(base, seam, origin, scale))
        keys.append(c["cell_key"])
    if not expanded:
        return None
    u = _union(expanded)
    d_lat, d_lon = _margin_deg(margin_m, olat)
    return {
        "id": "preview",
        "bbox": {"south": u["south"] - d_lat, "west": u["west"] - d_lon,
                 "north": u["north"] + d_lat, "east": u["east"] + d_lon},
        "cells": keys, "state": "planned",
    }


def preview_clumps(cells, origin, settings) -> list[dict]:
    """The tiles Meld will request, PRE-SPLIT by the area budget — for the UI overlay before a
    run. Each entry is one planned Overpass tile (state 'planned'), drawn as a gray-blue dotted
    box. Mirrors run_prefetch's planning exactly (same _plan_clumps + budget) so the preview the
    user sees matches what actually downloads. Returns [] if there's nothing to plan."""
    olat, olon = origin.get("lat"), origin.get("lon")
    if olat is None or olon is None or not cells:
        return []
    scale = float(settings.get("scale", 1.0) or 1.0)
    seam = int(settings.get("seam_buffer_chunks", 8) or 0)
    margin_m = float(settings.get("prefetch_margin_m", 256) or 0)
    work = []
    for c in cells:
        try:
            rx, rz, size = (int(x) for x in c["cell_key"].split(","))
        except (ValueError, KeyError):
            continue
        base = cell_bbox(rx, rz, size, olat, olon, scale)
        work.append({"cell_key": c["cell_key"], "rx": rx, "rz": rz, "size": size,
                     "expanded": expand_bbox_for_seam(base, seam, origin, scale)})
    if not work:
        return []
    d_lat, d_lon = _margin_deg(margin_m, olat)
    total_area = _bbox_area_km2(_union([g["expanded"] for g in work]))
    out = []
    for i, grp in enumerate(_plan_clumps(work, _resolve_tile_budget(settings, total_area))):
        u = _union([g["expanded"] for g in grp])
        bbox = {"south": u["south"] - d_lat, "west": u["west"] - d_lon,
                "north": u["north"] + d_lat, "east": u["east"] + d_lon}
        out.append({"id": f"plan-{i}", "bbox": bbox, "state": "planned",
                    "cells": [g["cell_key"] for g in grp]})
    return out


# OSM responses drift (new buildings/roads), unlike terrain/ESA tiles, so the shared OSM
# cache entries expire. Bump OSM_CACHE_VERSION if the Overpass query shape ever changes
# (it salts the key, invalidating every old file).
OSM_CACHE_TTL_DAYS = 30
OSM_CACHE_VERSION = "v1"


def meld_cache_root() -> Path:
    """The ONE shared Meld cache root (OSM + terrain + land-cover), kept inside the Meld
    project so it's visible and reused by EVERY project/world. Override with the MELD_CACHE_DIR
    env var (e.g. point it at a drive with space). Default: light-meld/cache. The Arnis fork is
    told this path via ARNIS_CACHE_ROOT so terrain/ESA land here too, not in hidden AppData."""
    env = os.environ.get("MELD_CACHE_DIR")
    if env and env.strip():
        # Normalize: strip surrounding quotes (common when pasting a spaced path), expand ~,
        # and resolve to absolute so Python and the child arnis (ARNIS_CACHE_ROOT) never diverge
        # if their CWDs differ.
        env = env.strip().strip('"').strip("'")
        if env:
            return Path(env).expanduser().resolve()
    return Path(__file__).resolve().parent.parent / "cache"   # = light-meld/cache


def meld_osm_cache_dir() -> Path:
    """Shared OSM prefetch cache (content-keyed by bbox + scale + overpass set) under the Meld
    cache root, so a new world over an already-fetched area reuses the verified OSM instead of
    re-downloading. Lives OUTSIDE any single project root."""
    return meld_cache_root() / "osm"


# ── terrain (elevation) prefetch ────────────────────────────────────────────
# Matches the Arnis fork's get_cache_dir("aws") under ARNIS_CACHE_ROOT: <root>/arnis-tile-cache/aws.
def aws_tile_cache_dir() -> Path:
    return meld_cache_root() / "arnis-tile-cache" / "aws"


def purge_small_tiles(min_bytes: int = 67, log=None) -> int:
    """Delete only genuinely-junk terrain tiles (smaller than a minimal PNG, ~67 B = truncated /
    0-byte) from the shared cache. NOTE: legit ocean / uniform-elevation Terrarium tiles compress
    to ~100-750 B and are VALID, so the old 2048 floor wrongly nuked them and broke offline packs;
    the Arnis fork now decode-verifies on read, so size is only used to drop sub-PNG garbage."""
    d = aws_tile_cache_dir()
    if not d.exists():
        return 0
    n = 0
    for f in d.glob("*.png"):
        try:
            if f.stat().st_size < min_bytes:
                f.unlink()
                n += 1
        except Exception:
            pass
    if log and n:
        log(f"  [Terrain] purged {n} poisoned (<{min_bytes}B) cached tile(s)")
    return n


def run_terrain_prefetch(bboxes, exe, log, on_progress=None, timeout_s: int = 1200,
                         elev_zoom: int | None = None) -> dict:
    """Warm the AWS terrain tiles for each bbox SEQUENTIALLY via `arnis --download-terrain-only`
    (one process at a time, 8 concurrent inside). This pre-fills the shared tile cache without
    the ~64-concurrent S3 burst that the parallel cells would otherwise cause (which truncates
    tiles into flat-terrain seams). Best-effort: failures just mean those cells fetch live.

    elev_zoom: the SAME elevation zoom the cells will request (ARNIS_ELEV_ZOOM). The warm MUST be
    told this — terrain tiles are cached zoom-keyed (z{z}_x_y.png), so a warm at the binary's default
    zoom (15) does NOT satisfy cells that fetch z13 at 1:10, and every cell re-downloads live. Pinning
    the env makes the warm fill exactly the tiles the cells need (the whole point of warming)."""
    total = len(bboxes)
    ok_tiles = 0
    failed_tiles = 0
    # Pin the child's elevation zoom to match generation; without this the warm fills the wrong
    # zoom and the cache-hit it exists to create never happens. None → inherit (legacy behaviour).
    env = None
    if elev_zoom is not None:
        env = {**os.environ, "ARNIS_ELEV_ZOOM": str(int(elev_zoom))}
        log(f"  [Terrain] warming at z{int(elev_zoom)} (matched to generation)")
    for i, bbox in enumerate(bboxes):
        cmd = [
            str(exe),
            "--bbox", f"{bbox['south']},{bbox['west']},{bbox['north']},{bbox['east']}",
            "--download-terrain-only",
        ]
        try:
            proc = subprocess.run(cmd, capture_output=True, text=True, env=env,
                                  timeout=timeout_s, encoding="utf-8", errors="replace")
            out = (proc.stdout or "") + (proc.stderr or "")
            m = re.search(r"(\d+) tile\(s\) cached, (\d+) failed", out)
            if m:
                ok_tiles += int(m.group(1))
                failed_tiles += int(m.group(2))
        except subprocess.TimeoutExpired:
            log(f"  [Terrain] sweep {i + 1}/{total} timed out (cells will fetch live)")
        except Exception as ex:  # noqa: BLE001
            log(f"  [Terrain] sweep {i + 1}/{total} error: {ex}")
        if on_progress:
            on_progress(i + 1, total, ok_tiles, failed_tiles)
    log(f"  [Terrain] warmed {ok_tiles} tile(s), {failed_tiles} failed across {total} sweep(s)")
    return {"sweeps": total, "ok": ok_tiles, "failed": failed_tiles}


def run_prefetch(cells, origin, settings, exe, cache_dir, log, on_chunk) -> dict:
    """Pre-fetch OSM for `cells` and return {cell_key: osm_source}.

    osm_source is the shared grid-cache DIRECTORY (str): the cell hands it to Arnis as
    `--osm-tile-dir`, and Arnis reads its own covering z11 tiles from it directly — there
    is no per-cell merge/clump file anymore.

    cells: list of {cell_key, bbox} (bbox unused here; recomputed canonically).
    on_chunk(chunk_dict): called whenever a chunk's state changes (for the UI overlay).
    Cells whose covering tiles couldn't all be fetched are simply omitted from the map,
    so they fall back to live Overpass during generation.
    """
    olat, olon = origin.get("lat"), origin.get("lon")
    if olat is None or olon is None:
        return {}
    scale = float(settings.get("scale", 1.0) or 1.0)
    seam = int(settings.get("seam_buffer_chunks", 8) or 0)
    margin_m = float(settings.get("prefetch_margin_m", 256) or 0)
    overpass_url = settings.get("overpass_url") or []
    if isinstance(overpass_url, str):
        overpass_url = [u.strip() for u in overpass_url.split(",") if u.strip()]

    cache_dir = Path(cache_dir)
    cache_dir.mkdir(parents=True, exist_ok=True)

    # Annotate each cell with its region indices and its seam-expanded bbox (exactly
    # what the cell will hand Arnis), so a chunk that covers the union covers them all.
    work = []
    for c in cells:
        ck = c["cell_key"]
        try:
            rx, rz, size = (int(x) for x in ck.split(","))
        except ValueError:
            continue
        base = cell_bbox(rx, rz, size, olat, olon, scale)
        expanded = expand_bbox_for_seam(base, seam, origin, scale)
        work.append({"cell_key": ck, "rx": rx, "rz": rz, "size": size, "expanded": expanded})
    if not work:
        return {}

    osm_files: dict[str, str] = {}
    _files_lock = threading.Lock()   # tiles download in parallel; guard the shared map
    ttl_s = OSM_CACHE_TTL_DAYS * 86400

    # Each cell's covering set of stable grid tiles (the reuse unit). Computed from the SAME
    # seam-expanded bbox the cell hands Arnis, so the tiles fully cover what the cell needs.
    for g in work:
        g["tiles"] = osm_grid.grid_tiles_for_bbox(g["expanded"])

    # Shared per-tile state so a tile straddling two clumps downloads ONCE across the whole run.
    _tile_state: dict[tuple, str] = {}          # (x,y) -> 'cached' | 'done' | 'fail'
    _locks: dict = {}                           # per (x,y) tile
    _locks_guard = threading.Lock()

    def _lock_for(key) -> threading.Lock:
        with _locks_guard:
            lk = _locks.get(key)
            if lk is None:
                lk = threading.Lock()
                _locks[key] = lk
            return lk

    def _fresh_on_disk(xy: tuple) -> bool:
        """True if grid tile (x,y) is already on disk, intact and within the TTL — a baked or
        previously-cached tile that needs NO network. Used both to skip the download and to label
        the overlay/log honestly (a fully-baked clump must not read as 'downloading')."""
        p = cache_dir / osm_grid.tile_filename(xy[0], xy[1])
        try:
            info = p.stat()
        except OSError:
            return False
        return info.st_size > 2 and (time.time() - info.st_mtime) < ttl_s and _looks_complete(p)

    def _ensure_tile(xy: tuple) -> str:
        """Make grid tile (x,y) present once (cache-aware) and return 'cached'|'done'|'fail'.
        A grid tile (z11 ≈ 190 km²) is well under the single-query budget, so density never forces
        a split — on the rare hard failure the tile is 'fail' and its cells fetch live."""
        x, y = xy
        with _lock_for(xy):
            st = _tile_state.get(xy)
            if st is not None:
                return st
            if _fresh_on_disk(xy):                  # baked .pbf tile or prior cache → no download
                _tile_state[xy] = "cached"
                return "cached"
            path = cache_dir / osm_grid.tile_filename(x, y)
            ok, reason = _download_one(exe, osm_grid.tile_bounds_ll(x, y), path, overpass_url, log)
            if not ok:
                log(f"  [Prefetch] grid tile {osm_grid.OSM_GRID_Z}/{x}/{y} failed "
                    f"({reason}) — cells using it fetch live")
            _tile_state[xy] = "done" if ok else "fail"
            return _tile_state[xy]

    def fetch_group(group: list[dict], depth: int) -> None:
        """Process one clump: ensure its grid tiles are present (shared globally), then point each
        fully-covered cell at the grid-cache dir (Arnis reads its own tiles — no merge). The clump
        is only an overlay/terrain GROUPING now — the cache lives on the grid tiles underneath, so
        overlapping selections reuse them."""
        keys = [g["cell_key"] for g in group]
        union = _union([g["expanded"] for g in group])
        d_lat, d_lon = _margin_deg(margin_m, olat)
        chunk_bbox = {
            "south": union["south"] - d_lat, "west": union["west"] - d_lon,
            "north": union["north"] + d_lat, "east": union["east"] + d_lon,
        }
        cid = _bbox_key(chunk_bbox, f"{OSM_CACHE_VERSION}|grid{osm_grid.OSM_GRID_Z}")
        clump_tiles = sorted({t for g in group for t in g["tiles"]})
        # Peek the cache so the overlay/log are honest: a clump whose tiles are all already on disk
        # (baked from a .pbf, or fetched on a prior run) is ASSEMBLED locally, never "downloading".
        n_need = sum(1 for xy in clump_tiles
                     if _tile_state.get(xy) not in ("cached", "done") and not _fresh_on_disk(xy))
        chunk = {"id": cid, "bbox": chunk_bbox, "cells": keys, "depth": depth,
                 "state": "downloading" if n_need else "cached", "file": "", "error": None}
        on_chunk(dict(chunk))
        if n_need:
            log(f"  [Prefetch] clump of {len(keys)} cell(s): fetching {n_need} of "
                f"{len(clump_tiles)} grid tile(s) ({len(clump_tiles) - n_need} already local)…")
        else:
            log(f"  [Prefetch] clump of {len(keys)} cell(s): all {len(clump_tiles)} grid tile(s) "
                f"baked/local — NO download, NO merge (Arnis reads tiles directly)")
        n_dl = n_cache = 0
        any_fail = False
        for xy in clump_tiles:
            st = _ensure_tile(xy)
            if st == "done":
                n_dl += 1
            elif st == "cached":
                n_cache += 1
            else:
                any_fail = True

        # NO merge step: each fully-covered cell reads its z11 grid tiles STRAIGHT from the cache
        # dir via Arnis `--osm-tile-dir <cache_dir>`. Arnis recomputes the covering tile set from
        # --bbox (the same tiles grid_tiles_for_bbox planned, same web-mercator formula), reads
        # each osm_g1_z11_{x}_{y}.json and dedups them itself. The clump stays purely an
        # overlay/terrain grouping; the cache lives on the shared grid tiles underneath. This kills
        # the per-run "assembling" pass entirely (the slow part) and shrinks each cell's Arnis parse
        # from the whole clump superset to only its own ~9-16 covering tiles.
        n_ok = 0
        for g in group:
            if all(_tile_state.get(t) in ("cached", "done") for t in g["tiles"]):
                with _files_lock:
                    osm_files[g["cell_key"]] = str(cache_dir)
                n_ok += 1

        if n_ok == 0:
            chunk["state"] = "failed"
            chunk["error"] = "all covering tiles failed"
        else:
            chunk["state"] = "done" if n_dl else "cached"
        on_chunk(dict(chunk))
        parts = ([f"{n_dl} fetched"] if n_dl else []) + ([f"{n_cache} from local"] if n_cache else [])
        log(f"  [Prefetch] clump ({', '.join(parts) or 'no tiles'}); "
            f"{n_ok}/{len(keys)} cell(s) served from grid"
            + (" (some tiles failed → live)" if any_fail else ""))

    # Group cells into clumps for the overlay/terrain (the area budget keeps each overlay box a
    # sane size); the real download unit is the stable grid tile inside _ensure_tile, deduped
    # across all clumps so the SAME tile never downloads twice even when two clumps share it.
    total_area = _bbox_area_km2(_union([g["expanded"] for g in work]))
    budget_km2 = _resolve_tile_budget(settings, total_area)
    clumps = _plan_clumps(work, budget_km2)
    tile_set = {t for g in work for t in g["tiles"]}
    n_tiles = len(tile_set)
    n_local = sum(1 for xy in tile_set if _fresh_on_disk(xy))
    try:
        conc = int(settings.get("prefetch_concurrency", 2) or 2)
    except (TypeError, ValueError):
        conc = 2
    conc = max(1, min(_MAX_PREFETCH_CONCURRENCY, conc))
    if n_local >= n_tiles:
        log(f"  [Prefetch] {len(work)} cell(s) over {n_tiles} stable z{osm_grid.OSM_GRID_Z} grid "
            f"tile(s) — ALL local (baked/cached), 0 to download; cells read tiles directly (no merge)")
    else:
        log(f"  [Prefetch] {len(work)} cell(s) over {n_tiles} stable z{osm_grid.OSM_GRID_Z} grid "
            f"tile(s) in {len(clumps)} overlay group(s); {n_tiles - n_local} to fetch, "
            f"{n_local} already local; {conc} download(s) at a time")
    if conc <= 1 or len(clumps) <= 1:
        for grp in clumps:
            fetch_group(grp, 0)
    else:
        # Up to `conc` clumps processed at once; a shared tile serializes on its own lock so the
        # number of live Overpass requests never exceeds `conc` (the public per-IP slot allowance).
        with ThreadPoolExecutor(max_workers=conc) as ex:
            list(ex.map(lambda g: fetch_group(g, 0), clumps))
    return osm_files
