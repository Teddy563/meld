#!/usr/bin/env python3
"""
light-meld orchestrator.

Flow: set origin -> survey (lock elevation) -> grid (split selection) ->
queue (parallel Arnis) -> per-cell merge into the master world.

See ../light-docs/ for the full spec. The coordinate convention lives in
src/coords.py and is matched by the Arnis fork's transform_point fix
(light-docs/03).
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path

# Windows consoles default to cp1252; Arnis stdout and log lines can contain
# Unicode (arrows, degree signs, accented place names). Without this, a single
# non-cp1252 character printed by log() would crash the whole server. Force
# UTF-8 with replacement so logging can never take the process down.
for _stream in (sys.stdout, sys.stderr):
    try:
        _stream.reconfigure(encoding="utf-8", errors="replace")
    except Exception:
        pass

from flask import Flask, request, jsonify, send_from_directory, abort

BASE_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(BASE_DIR))

from src.project import Project, default_settings
from src.grid import cells_for_bbox
from src.coords import expand_bbox_for_seam, cell_bbox, snap_to_region_grid
from src.arnis_cmd import build_arnis_cmd, run_arnis, find_world_dir, clean_output_dir, parse_progress
from src.prefetch import run_prefetch, preview_union
from src.merge import (merge_cell_into_master, strip_buffer_regions,
                        MeldCoordinateDriftError, MeldCollisionError)
from src.survey import survey_elevation
from src.workers import WorkerPool

app = Flask(__name__)   # no static catch-all — assets served via /assets/<f> below

PROJECT = Project(BASE_DIR / "projects" / "default")
POOL = WorkerPool(max_workers=PROJECT.settings().get("max_workers", 4))

_LOG: list[str] = []

# Generation run stats (for the live timer + final report).
_RUN_LOCK = threading.Lock()
_RUN = {"started": None, "ended": None, "total": 0, "done": 0, "failed": 0,
        "est_regions": 0, "est_mb": 0, "actual_mb": None}
MB_PER_REGION = 4   # rough estimate for the size report

# OSM prefetch state (for the live cyan-chunk overlay + status). Populated while a
# selection's OSM is being downloaded once and shared to all cells (src/prefetch.py).
_PREFETCH_LOCK = threading.Lock()
_PREFETCH = {"active": False, "done": False, "chunks": [], "started": None, "note": ""}


def _osm_cache_dir() -> Path:
    return PROJECT.root / "osm_cache"


def _prefetch_on_chunk(chunk: dict) -> None:
    """Upsert a chunk by id so the UI can recolor it live as state changes."""
    with _PREFETCH_LOCK:
        chunks = _PREFETCH["chunks"]
        for i, c in enumerate(chunks):
            if c["id"] == chunk["id"]:
                chunks[i] = chunk
                return
        chunks.append(chunk)


def _safe_world_name(name: str) -> str:
    name = (name or "Meld World").strip() or "Meld World"
    return "".join(c for c in name if c not in '<>:"/\\|?*').strip() or "Meld World"


def _world_icon_src() -> Path | None:
    """Optional custom Minecraft world icon (icon.png, 64x64). Drop one of these
    in and every generated world gets it in its world-selection list."""
    for c in (BASE_DIR / "web" / "world_icon.png", BASE_DIR / "world_icon.png"):
        if c.exists():
            return c
    return None


def _apply_world_icon(world_path) -> None:
    src = _world_icon_src()
    if src:
        try:
            shutil.copy2(src, Path(world_path) / "icon.png")
        except Exception:
            pass


def master_world_path(create: bool = True) -> Path:
    """Path of the merged Minecraft world.

    Save location (settings.master_world_dir) is the PARENT FOLDER where worlds are
    kept — e.g. .minecraft/saves. The world itself is a SUBFOLDER named by the
    World Name, so several worlds (Meld World, Meld World 2, …) can live in one
    folder. Blank save location → the project folder."""
    s = PROJECT.settings()
    name = _safe_world_name(PROJECT.load().get("name", "Meld World"))
    d = (s.get("master_world_dir") or "").strip()
    parent = Path(d) if d else PROJECT.root
    p = parent / name
    if create:
        p.mkdir(parents=True, exist_ok=True)
        (p / "region").mkdir(parents=True, exist_ok=True)
        _apply_world_icon(p)
    return p


def _dir_size_mb(p: Path) -> float:
    try:
        total = sum(f.stat().st_size for f in p.rglob("*") if f.is_file())
        return round(total / (1024 * 1024), 1)
    except Exception:
        return 0.0


def log(msg: str) -> None:
    line = str(msg)
    _LOG.append(line)
    if len(_LOG) > 2000:
        del _LOG[:1000]
    print(line, flush=True)


# Arnis prints hundreds of lines per cell (one per tile, per element). We can't
# dump all of that into the live log tab without burying the RUN/MERGE lines, so
# every line is captured to a persistent per-cell file (logs/cell-*.log) and only
# the diagnostically useful lines are surfaced into the _LOG buffer the UI polls.
# These keywords are chosen to make the elevation-fetch path visible: tile
# downloads, retries, rate-limit recovery, provider fallback, and flat-ground.
_ARNIS_LOG_KEYWORDS = (
    "warning", "error", "failed", "panic", "fallback", "falling back",
    "elevation", "still missing", "retry", "could not be fetched",
    "flat ground", "unavailable", "corrupted", "land cover", "downloading",
)
# Per-tile chatter that matches a keyword above but is pure noise at scale.
_ARNIS_LOG_EXCLUDE = (
    "fetching tile x=", "loading cached tile", "bilinear sampling",
)


def _arnis_should_surface(text: str) -> bool:
    low = text.lower()
    if any(x in low for x in _ARNIS_LOG_EXCLUDE):
        return False
    return any(k in low for k in _ARNIS_LOG_KEYWORDS)


# ── arnis binary resolution ────────────────────────────────────────────────

def resolve_arnis_exe() -> Path | None:
    candidates = [
        BASE_DIR / "arnis.exe",
        BASE_DIR / "arnis",
        BASE_DIR.parent / "arnis.exe",                       # the parent Meld project
        BASE_DIR.parent / "arnis",
        BASE_DIR.parent / "arnis-source" / "target" / "release" / "arnis.exe",
        BASE_DIR.parent / "arnis-source" / "target" / "release" / "arnis",
    ]
    for c in candidates:
        if c.exists():
            return c
    return None


# ── worker runner: generate one cell, then merge it ────────────────────────

def _runner(job: dict, state: dict) -> bool:
    cell_key = job["cell_key"]
    out = job["output_path"]
    settings = job["settings"]
    origin = job["origin"]
    elevation = job["elevation"]
    seed = int((elevation or {}).get("seed", 1) or 1)
    world_name = job.get("world_name", "Meld World")

    exe = resolve_arnis_exe()
    if not exe:
        state.update(message="Arnis binary not found — build arnis-source (cargo build --release).")
        log(state["message"])
        return False

    PROJECT.set_cell_status(cell_key, "running")
    clean_output_dir(out)
    Path(out).mkdir(parents=True, exist_ok=True)

    seam = int(settings.get("seam_buffer_chunks", 8) or 0)
    scale_f = float(settings.get("scale", 1.0) or 1.0)
    # Anchor generation to the cell's REGION corner + region size derived from the
    # cell_key — NOT the raw user selection. cell_bbox() places the SW corner on an
    # exact region boundary (rx*size, rz*size), so Arnis always generates whole,
    # region-aligned cells that the canonical merge keeps cleanly. Falls back to the
    # passed bbox only for a bare bbox job with no cell_key.
    base_bbox = job["bbox"]
    parts = cell_key.split(",") if cell_key else []
    if len(parts) == 3 and origin.get("lat") is not None:
        rx, rz, size = int(parts[0]), int(parts[1]), int(parts[2])
        base_bbox = cell_bbox(rx, rz, size, origin["lat"], origin["lon"], scale_f)
    arnis_bbox = expand_bbox_for_seam(base_bbox, seam, origin, scale_f)

    cmd = build_arnis_cmd(str(exe), arnis_bbox, out, settings, origin, elevation, seed,
                          osm_file=job.get("osm_file"))
    if job.get("osm_file"):
        log(f"  [{cell_key}] using pre-fetched OSM (no Overpass call)")
    log("RUN " + " ".join(cmd))

    # Full Arnis stdout/stderr capture → persistent per-cell file (survives the
    # post-merge prune that wipes the cell output dir), plus filtered surfacing
    # into the live Meld log tab. `arnis_log_verbose` setting pushes every line.
    cell_tag = (cell_key or "bbox").replace(",", "_")
    cell_log_fp = None
    try:
        logs_dir = Path(out).parent.parent / "logs"
        logs_dir.mkdir(parents=True, exist_ok=True)
        cell_log_fp = open(logs_dir / f"cell-{cell_tag}.log", "w",
                           encoding="utf-8", errors="replace")
        cell_log_fp.write("RUN " + " ".join(cmd) + "\n")
        cell_log_fp.flush()
    except Exception:
        cell_log_fp = None
    verbose = bool(settings.get("arnis_log_verbose", False))
    _last_surfaced = {"line": None}

    def on_line(text: str):
        if not text:
            return
        state["message"] = text[:140]
        state["progress"] = parse_progress(text, state.get("progress", 0))
        if cell_log_fp is not None:
            try:
                cell_log_fp.write(text + "\n")
            except Exception:
                pass
        # De-duplicate consecutive identical lines (retry spam) before surfacing.
        if (verbose or _arnis_should_surface(text)) and text != _last_surfaced["line"]:
            _last_surfaced["line"] = text
            log(f"[{cell_tag}] {text}")

    def on_proc(p):
        state["process"] = p   # published so /api/stop can terminate this run

    ok = run_arnis(cmd, cwd=str(BASE_DIR), on_line=on_line, on_proc=on_proc)
    if cell_log_fp is not None:
        try:
            cell_log_fp.write(f"\n=== arnis exit ok={ok} ===\n")
            cell_log_fp.close()
        except Exception:
            pass
    if not ok:
        PROJECT.set_cell_status(cell_key, "failed")
        state.update(message="Arnis generation failed.")
        return False

    world_dir = find_world_dir(out)
    if not world_dir:
        PROJECT.set_cell_status(cell_key, "failed")
        state.update(message="No world dir produced.")
        return False

    # Name the per-cell subregion world "Meld Sub World N" (stable, no duplicates).
    try:
        from src.level_dat import patch_level_name
        n = PROJECT.subworld_number(cell_key)
        patch_level_name(Path(world_dir) / "level.dat", f"Meld Sub World {n}")
    except Exception:
        pass

    state.update(progress=96, message="Merging…")
    try:
        master = str(master_world_path())
        # overwrite_collisions=True is safe under the v1 uniform grid: each cell
        # owns a disjoint canonical region rectangle, so any collision is the
        # SAME cell re-merging its own regions (a re-run/repair), never two
        # different cells fighting over one region.
        res = merge_cell_into_master(
            world_dir, master, cell_key,
            seam_buffer_chunks=seam, world_name=world_name,
            overwrite_collisions=True,
        )
        log(f"MERGE {cell_key}: +{res['regions_copied']} regions, "
            f"-{res['regions_skipped']} seam, level.dat={res['level_dat']}")
    except MeldCoordinateDriftError as ex:
        PROJECT.set_cell_status(cell_key, "drift")
        state.update(message=f"DRIFT GUARD: {ex}")
        log("ERROR " + str(ex))
        return False
    except MeldCollisionError as ex:
        PROJECT.set_cell_status(cell_key, "collision")
        state.update(message=f"COLLISION: {ex}")
        log("ERROR " + str(ex))
        return False
    except Exception as ex:  # noqa: BLE001
        PROJECT.set_cell_status(cell_key, "failed")
        state.update(message=f"Merge error: {ex}")
        log("ERROR " + str(ex))
        return False

    PROJECT.set_cell_status(cell_key, "merged")
    # Prune the per-cell subregion world now that its canonical regions live in the
    # master world — avoids doubling storage. Toggle via settings.prune_cell_after_merge.
    if settings.get("prune_cell_after_merge", True):
        try:
            shutil.rmtree(out, ignore_errors=True)
            log(f"  [Prune] removed cell subregion {cell_key} (merged into master)")
        except Exception:
            pass
    else:
        # Keeping the subregion: strip its seam-buffer region files so it holds ONLY
        # its canonical regions. Kept subregions are then disjoint, so their region/
        # files can be drag-and-dropped straight into one master world.
        try:
            n = strip_buffer_regions(world_dir, cell_key)
            log(f"  [Keep] {cell_key}: stripped {n} seam-buffer region files (canonical-only, drag-drop ready)")
        except Exception:
            pass
    state.update(progress=100, message="Merged.")
    return True


def _on_complete(job, ok, err):
    with _RUN_LOCK:
        if ok:
            _RUN["done"] += 1
        else:
            _RUN["failed"] += 1
        if _RUN["total"] and (_RUN["done"] + _RUN["failed"]) >= _RUN["total"] and not _RUN["ended"]:
            _RUN["ended"] = time.time()
            try:
                _RUN["actual_mb"] = _dir_size_mb(master_world_path(create=False))
            except Exception:
                _RUN["actual_mb"] = None


POOL.configure(_runner, _on_complete)


# ── routes ──────────────────────────────────────────────────────────────────

@app.route("/")
def index():
    return send_from_directory(str(BASE_DIR / "web"), "index.html")


@app.route("/assets/<path:fname>")
def assets(fname):
    """Serve static web assets (logo, etc.) from web/ under an explicit prefix so
    there is no catch-all rule that could shadow the /api POST routes."""
    if "/" in fname or "\\" in fname:
        abort(404)
    target = BASE_DIR / "web" / fname
    if not target.is_file():
        abort(404)
    return send_from_directory(str(BASE_DIR / "web"), fname)


@app.route("/docs/")
def docs_index():
    """Serve the bundled docs site (site/) so the in-app Guide can link to it."""
    return send_from_directory(str(BASE_DIR / "site"), "index.html")


@app.route("/docs/<path:fname>")
def docs_file(fname):
    return send_from_directory(str(BASE_DIR / "site"), fname)   # safe_join guards traversal


@app.route("/api/state")
def api_state():
    return jsonify({
        "origin": PROJECT.origin(),
        "settings": PROJECT.settings(),
        "elevation": PROJECT.elevation(),
        "grid": PROJECT.load_grid(),
        "name": PROJECT.load().get("name", "Meld World"),
        "arnis_found": resolve_arnis_exe() is not None,
        "master_world": str(master_world_path(create=False)),   # the world subfolder
        "save_location": (PROJECT.settings().get("master_world_dir") or "").strip() or str(PROJECT.root),
        "world_icon": _world_icon_src() is not None,
    })


@app.route("/api/name", methods=["POST"])
def api_name():
    d = request.json or {}
    name = PROJECT.set_name(d.get("name") or "Meld World")
    # Patch the master world's LevelName in place if it already exists.
    dat = master_world_path(create=False) / "level.dat"
    if dat.exists():
        from src.level_dat import patch_level_name, gold_name
        patch_level_name(dat, gold_name(name))
    return jsonify({"ok": True, "name": name})


@app.route("/api/worlds")
def api_worlds():
    grid = PROJECT.load_grid()
    cells = []
    for cell_key, status in sorted(grid.items()):
        sub = PROJECT.cells_dir / cell_key.replace(",", "_")
        cells.append({
            "cell_key": cell_key,
            "status": status,
            "has_source": sub.exists(),
            "size_mb": _dir_size_mb(sub) if sub.exists() else 0.0,
        })
    mp = master_world_path(create=False)
    region_dir = mp / "region"
    master = {
        "path": str(mp),
        "name": PROJECT.load().get("name", "Meld World"),
        "exists": region_dir.exists(),
        "regions": len(list(region_dir.glob("*.mca"))) if region_dir.exists() else 0,
        "size_mb": _dir_size_mb(mp) if mp.exists() else 0.0,
    }
    return jsonify({"cells": cells, "master": master})


@app.route("/api/world/delete", methods=["POST"])
def api_world_delete():
    """Delete one cell's subregion: its source world AND its canonical regions in
    the master world. Resets the cell to 'planned' so it can be regenerated."""
    from src.coords import canonical_region_bounds
    d = request.json or {}
    cell_key = d.get("cell_key")
    if not cell_key:
        return jsonify({"ok": False, "error": "cell_key required"}), 400

    sub = PROJECT.cells_dir / cell_key.replace(",", "_")
    if sub.exists():
        shutil.rmtree(sub, ignore_errors=True)

    removed = 0
    b = canonical_region_bounds(cell_key)
    mregion = master_world_path(create=False) / "region"
    if b and mregion.exists():
        rx_min, rx_max, rz_min, rz_max = b
        for rx in range(rx_min, rx_max + 1):
            for rz in range(rz_min, rz_max + 1):
                f = mregion / f"r.{rx}.{rz}.mca"
                if f.exists():
                    try:
                        f.unlink()
                        removed += 1
                    except Exception:
                        pass

    grid = PROJECT.load_grid()
    if cell_key in grid:
        grid[cell_key] = "planned"
        PROJECT.save_grid(grid)
    log(f"  [Delete] {cell_key}: removed source + {removed} master region(s)")
    return jsonify({"ok": True, "cell_key": cell_key, "removed_regions": removed})


@app.route("/api/pick-folder", methods=["POST"])
def api_pick_folder():
    """Open a native folder-select dialog on the local machine (the server runs on
    the user's box) and return the chosen path. Uses a throwaway tkinter subprocess
    so it can't block or crash the server."""
    code = (
        "import tkinter as tk, tkinter.filedialog as fd\n"
        "r=tk.Tk(); r.withdraw(); r.attributes('-topmost', True)\n"
        "p=fd.askdirectory(title='Select save location')\n"
        "print(p or '')\n"
    )
    try:
        out = subprocess.run([sys.executable, "-c", code],
                             capture_output=True, text=True, timeout=180)
        lines = [ln for ln in (out.stdout or "").splitlines() if ln.strip()]
        return jsonify({"ok": True, "path": lines[-1].strip() if lines else ""})
    except Exception as e:  # noqa: BLE001
        return jsonify({"ok": False, "error": str(e)}), 500


@app.route("/api/open-folder", methods=["POST"])
def api_open_folder():
    """Open the save-location folder in the OS file browser (server runs locally)."""
    target = master_world_path(create=False)
    if not target.exists():
        target = target.parent
    try:
        if sys.platform == "win32":
            os.startfile(str(target))   # type: ignore[attr-defined]  # noqa: S606
        elif sys.platform == "darwin":
            subprocess.Popen(["open", str(target)])
        else:
            subprocess.Popen(["xdg-open", str(target)])
        return jsonify({"ok": True, "path": str(target)})
    except Exception as ex:  # noqa: BLE001
        return jsonify({"ok": False, "error": str(ex)}), 500


@app.route("/api/log")
def api_log():
    return jsonify({"log": _LOG[-400:]})


@app.route("/logs")
def logs_page():
    return (
        "<!doctype html><html><head><meta charset='utf-8'><title>Meld - Log</title>"
        "<style>body{background:#13110d;color:#cdc3ad;margin:0;padding:12px;"
        "font:12px/1.5 ui-monospace,Consolas,monospace}"
        "pre{white-space:pre-wrap;word-break:break-word;margin:0}</style></head>"
        "<body><pre id='l'>loading...</pre><script>"
        "async function t(){try{const s=await fetch('/api/log').then(r=>r.json());"
        "const el=document.getElementById('l');const atBottom="
        "window.innerHeight+window.scrollY>=document.body.scrollHeight-40;"
        "el.textContent=(s.log||[]).join('\\n');"
        "if(atBottom)window.scrollTo(0,document.body.scrollHeight);}catch(e){}"
        "setTimeout(t,1500);}t();</script></body></html>"
    )


@app.route("/api/master/reset", methods=["POST"])
def api_master_reset():
    """Wipe the merged master world (region/poi/entities/level.dat). Merged cells
    revert to 'planned'."""
    mp = master_world_path(create=False)
    removed = 0
    for sub in ("region", "poi", "entities"):
        p = mp / sub
        if p.exists():
            removed += len(list(p.glob("*.mca")))
            shutil.rmtree(p, ignore_errors=True)
    dat = mp / "level.dat"
    if dat.exists():
        try:
            dat.unlink()
        except Exception:
            pass
    grid = PROJECT.load_grid()
    for k, v in list(grid.items()):
        if v == "merged":
            grid[k] = "planned"
    PROJECT.save_grid(grid)
    log(f"  [Master reset] removed {removed} region(s)")
    return jsonify({"ok": True, "removed_regions": removed})


@app.route("/api/origin", methods=["POST"])
def api_set_origin():
    d = request.json or {}
    if d.get("lat") is None or d.get("lon") is None:
        return jsonify({"ok": False, "error": "lat and lon required"}), 400
    # Snap the origin onto the global region grid (anchored at 0,0) so it lands on
    # an exact region corner and is deterministic: the same coords always snap to
    # the same origin. That makes pasting an origin from another project reproduce
    # it exactly, and keeps the cell grid predefined/stable.
    scale = float(PROJECT.settings().get("scale", 1.0) or 1.0)
    slat, slon = snap_to_region_grid(float(d["lat"]), float(d["lon"]), scale)
    res = PROJECT.set_origin(slat, slon, force=bool(d.get("force")))
    return jsonify(res), (200 if res.get("ok") else 409)


@app.route("/api/origin/unlock", methods=["POST"])
def api_unlock_origin():
    return jsonify({"ok": True, "origin": PROJECT.unlock_origin()})


@app.route("/api/settings", methods=["GET", "POST"])
def api_settings():
    if request.method == "GET":
        return jsonify(PROJECT.settings())
    patch = request.json or {}
    # Seed is stored on the elevation block, not the settings blob — persist it
    # independently so editing the Seed field actually sticks.
    if "seed" in patch:
        PROJECT.set_seed(patch.pop("seed"))
    s = PROJECT.update_settings(patch)
    POOL.set_max_workers(int(s.get("max_workers") or 4))
    return jsonify({**s, "seed": PROJECT.elevation().get("seed", 1)})


@app.route("/api/survey", methods=["POST"])
def api_survey():
    d = request.json or {}
    bbox = d.get("bbox")
    if not bbox:
        return jsonify({"ok": False, "error": "bbox required"}), 400
    res = survey_elevation(bbox, zoom=int(d.get("zoom", 10)))
    if res.get("ok"):
        seed = int(PROJECT.elevation().get("seed", 1) or 1)
        PROJECT.set_elevation_lock(res["min_m"], res["max_m"], seed=seed)
    return jsonify(res)


@app.route("/api/elevation/manual", methods=["POST"])
def api_elevation_manual():
    d = request.json or {}
    if d.get("min_m") is None or d.get("max_m") is None:
        return jsonify({"ok": False, "error": "min_m and max_m required"}), 400
    ev = PROJECT.set_elevation_lock(d["min_m"], d["max_m"], seed=d.get("seed"))
    return jsonify({"ok": True, "elevation": ev})


@app.route("/api/grid", methods=["POST"])
def api_grid():
    d = request.json or {}
    bbox = d.get("bbox")
    origin = PROJECT.origin()
    if origin.get("lat") is None:
        return jsonify({"ok": False, "error": "set origin first"}), 400
    if not bbox:
        return jsonify({"ok": False, "error": "bbox required"}), 400
    settings = PROJECT.settings()
    size = int(d.get("size") or settings.get("job_size_regions") or 4)
    cells = cells_for_bbox(bbox, origin, float(settings.get("scale", 1.0)), size)
    grid = PROJECT.load_grid()
    for c in cells:
        grid.setdefault(c["cell_key"], "planned")
    PROJECT.save_grid(grid)
    return jsonify({"ok": True, "cells": cells, "count": len(cells)})


@app.route("/api/grid/clear", methods=["POST"])
def api_grid_clear():
    """Revert the grid plan. Drops planned/queued/failed cells so the selection
    can be re-split at a different cell size. Keeps 'merged' cells by default
    (their content is already in the master world)."""
    d = request.json or {}
    keep_merged = d.get("keep_merged", True)
    grid = PROJECT.load_grid()
    kept = {k: v for k, v in grid.items() if keep_merged and v == "merged"}
    removed = [k for k in grid if k not in kept]
    PROJECT.save_grid(kept)
    return jsonify({"ok": True, "removed": removed, "kept": list(kept)})


def _bbox_from_cell_key(cell_key: str, origin: dict, scale: float) -> dict:
    rx, rz, size = (int(x) for x in cell_key.split(","))
    return cell_bbox(rx, rz, size, origin["lat"], origin["lon"], scale)


def _submit_cells(cells: list[dict], osm_files: dict | None = None,
                  settings: dict | None = None, origin: dict | None = None) -> list[str]:
    """Submit a list of {cell_key, bbox} to the pool and (re)start the run clock.
    osm_files maps cell_key -> pre-fetched OSM json path (passed to Arnis as --file).
    settings/origin may be a snapshot taken before a prefetch, so the cells generate
    with the SAME scale/seam/origin the chunk bboxes were built from (otherwise a
    settings change mid-prefetch could leave a cell's bbox outside its chunk file).
    Shared by /api/queue, /api/cell/regenerate and /api/resume."""
    osm_files = osm_files or {}
    settings = settings if settings is not None else PROJECT.settings()
    origin = origin if origin is not None else PROJECT.origin()
    elevation = PROJECT.elevation()
    world_name = PROJECT.load().get("name", "Meld World")
    # Set the run clock (incl. total) BEFORE submitting, so a fast cell completing can't
    # see total=0 in _on_complete and mark the run ended prematurely.
    est_regions = sum(int(c["cell_key"].split(",")[2]) ** 2 for c in cells)
    with _RUN_LOCK:
        _RUN.update(started=time.time(), ended=None, total=len(cells), done=0, failed=0,
                    est_regions=est_regions, est_mb=est_regions * MB_PER_REGION, actual_mb=None)
    queued = []
    for c in cells:
        ck = c["cell_key"]
        out = str(PROJECT.cells_dir / ck.replace(",", "_"))
        PROJECT.set_cell_status(ck, "queued")   # atomic; won't clobber a worker's status
        POOL.submit({
            "cell_key": ck, "bbox": c["bbox"], "settings": settings,
            "origin": origin, "elevation": elevation, "output_path": out,
            "world_name": world_name, "osm_file": osm_files.get(ck),
        })
        queued.append(ck)
    return queued


def _start_generation(cells: list[dict]) -> tuple[list[str], bool]:
    """Pre-fetch the selection's OSM once, then submit the cells. Returns
    (cell_keys, prefetching). When prefetch is enabled the fetch runs in a background
    thread (so the HTTP call returns at once) and the cells are submitted to the pool
    only after the OSM is cached; otherwise cells are submitted immediately.

    settings + origin are snapshotted ONCE here and used for both the prefetch (chunk
    bboxes) and the generation (cell bboxes), so the two always agree."""
    settings = PROJECT.settings()
    origin = PROJECT.origin()
    exe = resolve_arnis_exe()
    if not settings.get("prefetch_enabled", True) or not exe or not cells:
        return _submit_cells(cells, settings=settings, origin=origin), False

    # Mark the cells queued now so the grid/overlay shows them during the prefetch.
    for c in cells:
        PROJECT.set_cell_status(c["cell_key"], "queued")
    with _PREFETCH_LOCK:
        _PREFETCH.update(active=True, done=False, chunks=[], started=time.time(),
                         note=f"prefetching OSM for {len(cells)} cell(s)…")

    def _worker():
        try:
            osm_files = run_prefetch(cells, origin, settings, str(exe),
                                     _osm_cache_dir(), log, _prefetch_on_chunk)
        except Exception as ex:  # noqa: BLE001
            log(f"[Prefetch] error, falling back to live fetch: {ex}")
            osm_files = {}
        with _PREFETCH_LOCK:
            _PREFETCH.update(active=False, done=True,
                             note=f"{len(osm_files)}/{len(cells)} cells from cached OSM")
        log(f"[Prefetch] done — {len(osm_files)}/{len(cells)} cells share cached OSM; "
            f"starting generation")
        _submit_cells(cells, osm_files, settings=settings, origin=origin)

    threading.Thread(target=_worker, daemon=True).start()
    return [c["cell_key"] for c in cells], True


def _elevation_gate_ok() -> bool:
    s = PROJECT.settings()
    return s.get("elevation_mode", "global") != "global" or PROJECT.elevation().get("locked")


@app.route("/api/queue", methods=["POST"])
def api_queue():
    d = request.json or {}
    origin = PROJECT.origin()
    if origin.get("lat") is None:
        return jsonify({"ok": False, "error": "set origin first"}), 400

    settings = PROJECT.settings()
    if not _elevation_gate_ok():
        return jsonify({"ok": False, "error": "elevation_mode is 'global' but no "
                        "elevation lock — run the survey first or set a manual range, "
                        "or switch elevation_mode to 'local'."}), 400

    # Worker cap: hard-capped at 16 (WorkerPool). The user owns the 8-16 range now — the UI
    # warns above 8 about the heavy save phase (disk + RAM) and lets them accept the risk —
    # so we do NOT silently reduce their choice here. The only forced clamp is the low-scale
    # one: scale<0.5 means a huge per-region real area where >2 concurrent AWS-tile fetches
    # corrupt elevation.
    workers = min(WorkerPool.MAX_WORKERS_HARD_CAP, int(settings.get("max_workers") or 4))
    scale = float(settings.get("scale", 1.0) or 1.0)
    note = ""
    if scale < 0.5 and workers > 2:
        workers = 2
        note = "clamped workers to 2 at scale<0.5 (AWS tile rate-limit safety)"
        log("[Workers] " + note)
    POOL.set_max_workers(workers)

    cells = d.get("cells")
    if not cells:
        bbox = d.get("bbox")
        if not bbox:
            return jsonify({"ok": False, "error": "cells or bbox required"}), 400
        size = int(d.get("size") or settings.get("job_size_regions") or 4)
        cells = cells_for_bbox(bbox, origin, scale, size)

    # Continue-where-left-off: skip cells already merged unless force=true.
    if not d.get("force"):
        grid = PROJECT.load_grid()
        cells = [c for c in cells if grid.get(c["cell_key"]) != "merged"]
    if not cells:
        return jsonify({"ok": True, "queued": [], "count": 0,
                        "note": "nothing to do — all cells already merged"})

    queued, prefetching = _start_generation(cells)
    return jsonify({"ok": True, "queued": queued, "count": len(queued), "note": note,
                    "prefetching": prefetching})


@app.route("/api/cell/regenerate", methods=["POST"])
def api_cell_regenerate():
    """Re-queue a single cell (click-a-square to retry/regenerate)."""
    d = request.json or {}
    ck = d.get("cell_key")
    if not ck or len(ck.split(",")) != 3:
        return jsonify({"ok": False, "error": "valid cell_key required"}), 400
    origin = PROJECT.origin()
    if origin.get("lat") is None:
        return jsonify({"ok": False, "error": "no origin set"}), 400
    scale = float(PROJECT.settings().get("scale", 1.0) or 1.0)
    bbox = _bbox_from_cell_key(ck, origin, scale)
    queued, prefetching = _start_generation([{"cell_key": ck, "bbox": bbox}])
    return jsonify({"ok": True, "queued": queued, "prefetching": prefetching})


@app.route("/api/resume", methods=["POST"])
def api_resume():
    """Re-queue every NOT-merged cell — crash/overnight recovery."""
    origin = PROJECT.origin()
    if origin.get("lat") is None:
        return jsonify({"ok": False, "error": "no origin set"}), 400
    scale = float(PROJECT.settings().get("scale", 1.0) or 1.0)
    grid = PROJECT.load_grid()
    todo = [k for k, v in grid.items() if v != "merged" and len(k.split(",")) == 3]
    if not todo:
        return jsonify({"ok": True, "queued": [], "count": 0, "note": "all cells already merged"})
    cells = [{"cell_key": k, "bbox": _bbox_from_cell_key(k, origin, scale)} for k in todo]
    queued, prefetching = _start_generation(cells)
    return jsonify({"ok": True, "queued": queued, "count": len(queued), "prefetching": prefetching})


@app.route("/api/new-world", methods=["POST"])
def api_new_world():
    """Start a fresh world (Arnis-style 'new world'). The current world stays saved
    on disk; the project resets — next free 'Meld World N' name, cleared plan, and a
    fresh origin + elevation so the next selection starts clean."""
    POOL.clear()
    parent = Path((PROJECT.settings().get("master_world_dir") or "").strip() or PROJECT.root)
    existing = set()
    try:
        existing = {p.name for p in parent.iterdir() if p.is_dir()}
    except Exception:
        pass
    base, name, n = "Meld World", "Meld World", 2
    while name in existing:
        name = f"{base} {n}"
        n += 1
    data = PROJECT.load()
    data["name"] = name
    data["origin"] = {"lat": None, "lon": None, "locked": False}
    ev = data.get("elevation") or {"seed": 1}
    ev.update(min_m=None, max_m=None, locked=False)
    data["elevation"] = ev
    PROJECT.save(data)
    PROJECT.save_grid({})
    with _RUN_LOCK:
        _RUN.update(started=None, ended=None, total=0, done=0, failed=0,
                    est_regions=0, est_mb=0, actual_mb=None)
    log(f"[New world] reset → '{name}'")
    return jsonify({"ok": True, "name": name})


@app.route("/api/queue/clear", methods=["POST"])
def api_queue_clear():
    n = POOL.clear()
    return jsonify({"ok": True, "cleared": n})


@app.route("/api/stop", methods=["POST"])
def api_stop():
    POOL.clear()
    n = POOL.terminate_all()
    return jsonify({"ok": True, "terminated": n})


@app.route("/api/status")
def api_status():
    with _RUN_LOCK:
        run = dict(_RUN)
    now = time.time()
    run["elapsed"] = ((run["ended"] or now) - run["started"]) if run["started"] else 0
    run["active"] = bool(run["started"] and not run["ended"])
    with _PREFETCH_LOCK:
        prefetch = {"active": _PREFETCH["active"], "done": _PREFETCH["done"],
                    "note": _PREFETCH["note"], "chunks": list(_PREFETCH["chunks"])}
    return jsonify({
        "workers": POOL.get_states(),
        "queue_size": POOL.queue_size(),
        "running": POOL.is_running(),
        "grid": PROJECT.load_grid(),
        "run": run,
        "prefetch": prefetch,
        "log": _LOG[-150:],
    })


@app.route("/api/prefetch/plan")
def api_prefetch_plan():
    """Preview the OSM download footprint for the current plan: the single chunk Meld
    will TRY first (whole selection + margin). Drawn as a cyan dashed box in the UI.
    At run time this box may split into quadrants if the endpoint rejects it; the live
    /api/status prefetch.chunks then reflect the real splits."""
    origin = PROJECT.origin()
    settings = PROJECT.settings()
    if origin.get("lat") is None or not settings.get("prefetch_enabled", True):
        return jsonify({"enabled": bool(settings.get("prefetch_enabled", True)), "chunks": []})
    scale = float(settings.get("scale", 1.0) or 1.0)
    grid = PROJECT.load_grid()
    cells = [{"cell_key": k, "bbox": _bbox_from_cell_key(k, origin, scale)}
             for k, v in grid.items() if v != "merged" and len(k.split(",")) == 3]
    chunk = preview_union(cells, origin, settings)
    return jsonify({"enabled": True, "chunks": [chunk] if chunk else []})


# ── recommend settings wizard: probe this PC + the save disk ────────────────────
def _total_ram_gb() -> float | None:
    try:  # Windows
        import ctypes

        class _MEMSTAT(ctypes.Structure):
            _fields_ = [("dwLength", ctypes.c_ulong), ("dwMemoryLoad", ctypes.c_ulong),
                        ("ullTotalPhys", ctypes.c_ulonglong), ("ullAvailPhys", ctypes.c_ulonglong),
                        ("ullTotalPageFile", ctypes.c_ulonglong), ("ullAvailPageFile", ctypes.c_ulonglong),
                        ("ullTotalVirtual", ctypes.c_ulonglong), ("ullAvailVirtual", ctypes.c_ulonglong),
                        ("ullAvailExtendedVirtual", ctypes.c_ulonglong)]
        m = _MEMSTAT(); m.dwLength = ctypes.sizeof(_MEMSTAT)
        if ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(m)):
            return round(m.ullTotalPhys / (1024 ** 3), 1)
    except Exception:
        pass
    try:  # POSIX
        return round(os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES") / (1024 ** 3), 1)
    except Exception:
        return None


def _disk_write_mbps(target_dir: str) -> float | None:
    """Sustained write speed of the disk the worlds save to (~192 MB, fsync'd)."""
    p = Path(target_dir)
    try:
        p.mkdir(parents=True, exist_ok=True)
    except Exception:
        return None
    f = p / ".meld_diskbench.tmp"
    chunk = b"\0" * (8 * 1024 * 1024)   # 8 MB
    n = 24                              # ~192 MB
    try:
        t0 = time.time()
        with open(f, "wb") as fh:
            for _ in range(n):
                fh.write(chunk)
            fh.flush()
            os.fsync(fh.fileno())
        dt = time.time() - t0
        return round(n * 8 / dt) if dt > 0 else None
    except Exception:
        return None
    finally:
        try:
            f.unlink(missing_ok=True)
        except Exception:
            pass


@app.route("/api/recommend")
def api_recommend():
    """Probe CPU, RAM and the save-disk write speed, then recommend cell size + workers.
    Generation is bound by the save phase (disk write + a RAM burst), not CPU, so the
    recommendation is the min of the CPU/RAM/disk budgets, capped at the safe ceiling of 8
    (the UI lets the user push to 16 manually, with a warning)."""
    cores = os.cpu_count() or 4
    ram_gb = _total_ram_gb()
    save_dir = str(master_world_path(create=True).parent)
    disk_mbps = _disk_write_mbps(save_dir)
    drive = (os.path.splitdrive(save_dir)[0] or save_dir)[:24]

    ram = ram_gb or 16.0
    disk = float(disk_mbps or 800)
    by_cpu = max(2, cores // 2)
    by_ram = max(2, int(ram // 3))     # ~3 GB per concurrent heavy (baked) save
    by_disk = max(2, int(disk // 90))  # ~90 MB/s sustained per worker during save bursts
    rec_workers = max(2, min(8, by_cpu, by_ram, by_disk))
    rec_cell = 4 if (disk < 600 or ram < 16) else 6
    rec_bake = ram >= 16
    bound = min([("CPU", by_cpu), ("RAM", by_ram), ("disk", by_disk)], key=lambda x: x[1])[0]
    note = (f"Limited by {bound}. The save phase is disk + RAM bound, so {rec_workers} "
            f"workers at cell size {rec_cell} keeps saves smooth. You can push higher "
            f"manually (the app warns above 8).")
    return jsonify({"ok": True, "cores": cores, "ram_gb": ram_gb, "disk_mbps": disk_mbps,
                    "drive": drive, "rec_cell": rec_cell, "rec_workers": rec_workers,
                    "rec_bake": rec_bake, "note": note})


if __name__ == "__main__":
    port = int(os.environ.get("PORT", 5630))
    print(f"light-meld -> http://127.0.0.1:{port}")
    print(f"arnis binary: {resolve_arnis_exe()}")
    app.run(host="127.0.0.1", port=port, threaded=True)
