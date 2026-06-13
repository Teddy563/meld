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
import subprocess
import time
from pathlib import Path

from .constants import METERS_PER_DEG_LAT
from .coords import cell_bbox, expand_bbox_for_seam, mpd_lon

# A download attempt that runs longer than this is treated as "too big" and split.
_DOWNLOAD_TIMEOUT_S = 220
# Don't subdivide forever: a single cell is the smallest unit we fetch.
_MAX_DEPTH = 6
# Strings in Arnis output that confirm a split-worthy failure (vs a hard network error).
_SPLIT_MARKERS = ("rate limited", "timed out", "request timeout", "maxsize",
                  "too many", "timeout", "out of memory", "server overloaded")


def _margin_deg(meters: float, lat: float) -> tuple[float, float]:
    """A metre margin as (d_lat, d_lon) at a given latitude."""
    if meters <= 0:
        return 0.0, 0.0
    mlon = mpd_lon(lat) or METERS_PER_DEG_LAT
    return meters / METERS_PER_DEG_LAT, meters / mlon


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
                  log) -> tuple[bool, str]:
    """Run `arnis --download-only` for one bbox. Returns (ok, reason).
    ok=False with a split-worthy reason means the caller should subdivide."""
    cmd = [
        str(exe),
        "--bbox", f"{bbox['south']},{bbox['west']},{bbox['north']},{bbox['east']}",
        "--save-json-file", str(out_json),
        "--download-only",
    ]
    if overpass_url:
        cmd += ["--overpass-url", ",".join(overpass_url)]
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True,
                              timeout=_DOWNLOAD_TIMEOUT_S, encoding="utf-8", errors="replace")
    except subprocess.TimeoutExpired:
        return False, "timed out"
    except Exception as ex:  # noqa: BLE001
        return False, f"spawn error: {ex}"

    output = (proc.stdout or "") + (proc.stderr or "")
    if proc.returncode == 0 and out_json.exists() and out_json.stat().st_size > 2:
        if _looks_complete(out_json):
            return True, "ok"
        return False, "truncated json"
    low = output.lower()
    reason = next((m for m in _SPLIT_MARKERS if m in low), None)
    return False, reason or f"exit {proc.returncode}"


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


def run_prefetch(cells, origin, settings, exe, cache_dir, log, on_chunk) -> dict:
    """Pre-fetch OSM for `cells` and return {cell_key: osm_file_path}.

    cells: list of {cell_key, bbox} (bbox unused here; recomputed canonically).
    on_chunk(chunk_dict): called whenever a chunk's state changes (for the UI overlay).
    Cells that can't be prefetched (all splits failed) are simply omitted from the
    map, so they fall back to live Overpass during generation.
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
        work.append({"cell_key": ck, "rx": rx, "rz": rz, "expanded": expanded})
    if not work:
        return {}

    osm_files: dict[str, str] = {}
    extra = f"rd-superset|op={','.join(overpass_url)}|s{scale}"

    def fetch_group(group: list[dict], depth: int) -> None:
        bboxes = [g["expanded"] for g in group]
        union = _union(bboxes)
        d_lat, d_lon = _margin_deg(margin_m, olat)
        chunk_bbox = {
            "south": union["south"] - d_lat, "west": union["west"] - d_lon,
            "north": union["north"] + d_lat, "east": union["east"] + d_lon,
        }
        keys = [g["cell_key"] for g in group]
        cid = _bbox_key(chunk_bbox, extra)
        out_json = cache_dir / f"osm_{cid}.json"
        chunk = {"id": cid, "bbox": chunk_bbox, "cells": keys, "depth": depth,
                 "state": "downloading", "file": str(out_json), "error": None}

        # Cache hit: reuse without a network call (only if the cached file is intact).
        if out_json.exists() and out_json.stat().st_size > 2 and _looks_complete(out_json):
            chunk["state"] = "cached"
            on_chunk(dict(chunk))
            for k in keys:
                osm_files[k] = str(out_json)
            log(f"  [Prefetch] cache hit for {len(keys)} cell(s) ({cid})")
            return

        on_chunk(dict(chunk))
        log(f"  [Prefetch] downloading 1 chunk for {len(keys)} cell(s) (depth {depth})…")
        ok, reason = _download_one(exe, chunk_bbox, out_json, overpass_url, log)
        if ok:
            chunk["state"] = "done"
            on_chunk(dict(chunk))
            for k in keys:
                osm_files[k] = str(out_json)
            log(f"  [Prefetch] OK: {len(keys)} cell(s) share {out_json.name}")
            return

        # Failed. Split into quadrants and retry, unless we're at a single cell.
        try:
            out_json.unlink(missing_ok=True)
        except Exception:
            pass
        if len(group) > 1 and depth < _MAX_DEPTH:
            chunk["state"] = "split"
            chunk["error"] = reason
            on_chunk(dict(chunk))
            log(f"  [Prefetch] '{reason}' — splitting {len(keys)} cells into quadrants")
            for sub in _split_cells(group):
                fetch_group(sub, depth + 1)
        else:
            chunk["state"] = "failed"
            chunk["error"] = reason
            on_chunk(dict(chunk))
            log(f"  [Prefetch] gave up on {keys} ('{reason}') — these cells fetch live")

    fetch_group(work, 0)
    return osm_files
