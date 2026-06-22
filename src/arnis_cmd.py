"""
arnis_cmd.py — build the Arnis-fork argv and run it.

Flag names verified against arnis-source/src/args.rs:
  --bbox S,W,N,E              required
  --output-dir DIR           (alias --path)
  --scale FLOAT              blocks per metre
  --ground-level INT        default -62
  --terrain                 BARE FLAG, OFF by default — must pass for elevation
  --roof / --interior / --land-cover  true|false (default true except interior)
  --master-origin-lat / --master-origin-lng   global coords
  --elevation-min / --elevation-max  global Y normalisation (the elevation lock)
  --tile-invariant-rendering N       deterministic building palette
  --road-detail max|clean|compact    default max (omit to keep upstream)
  --overpass-url A,B                 custom endpoints
  --rotation / --timeout / --disable-height-limit / --fillground / --debug

NOTE: this fork has NO 3D-structure-model flag (not pulled from upstream). The
project setting `generate_3d_models` is therefore a reserved no-op in v1 — it
emits nothing. Wire it to a real flag once the fork gains the upstream feature
(light-docs/05).
"""

from __future__ import annotations

import os
import re
import subprocess
from pathlib import Path

from .coords import recommended_elev_zoom, ELEV_ZOOM_MIN, ELEV_ZOOM_MAX
from .osm_grid import OSM_GRID_Z


# Biogeographic realm -> tree pack dir, picked from the selection centre (lat, lon). Ordered:
# the first box that contains the point wins (finer/subset realms first so they take priority).
# (code, lat_min, lat_max, lon_min, lon_max)
_REALM_BOXES = [
    ("fl",  8.0, 31.0,  -90.0, -60.0),   # Florida / SE US / Caribbean (subset of ENA, first)
    ("ena", 8.0, 62.0, -100.0, -52.0),   # eastern North America
    ("wna", 25.0, 72.0, -170.0, -100.0), # western North America
    ("sam", -56.0, 14.0,  -82.0, -34.0), # South America
    ("eur", 34.0, 72.0,  -25.0,  40.0),  # Europe + Mediterranean (Iceland/Azores to -25; before AFR)
    ("afr", -36.0, 37.0,  -19.0,  52.0), # Africa
    ("ind", -11.0, 29.0,   60.0, 155.0), # Indomalaya (tropical S/SE Asia; before ASN)
    ("asn", 5.0, 75.0,   40.0, 155.0),   # temperate Asia / Palearctic
    ("aus", -50.0, 0.0,  110.0, 180.0),  # Australia
    ("aus", -50.0, 32.0, -180.0, -130.0),# Oceania / Pacific / Hawaii (same pack)
]


def realm_for_latlon(lat: float, lon: float) -> str:
    """Pick the tree-pack realm code for a point. Falls back to 'vanilla-plus' if no realm
    box contains it (open ocean, polar, or a gap)."""
    for code, la0, la1, lo0, lo1 in _REALM_BOXES:
        if la0 <= lat <= la1 and lo0 <= lon <= lo1:
            return code
    return "vanilla-plus"


def effective_elev_zoom(settings: dict, origin_lat: float = 45.0) -> int:
    """Resolve the project's `elevation_zoom` setting to a concrete terrarium zoom for BOTH the data
    pack (download/coverage/preview) and the Arnis run (via ARNIS_ELEV_ZOOM). "auto"/blank -> the
    scale-matched recommendation; an explicit int is clamped to the valid [11,15] band."""
    raw = (settings or {}).get("elevation_zoom", "auto")
    scale = float((settings or {}).get("scale", 1.0) or 1.0)
    if raw in (None, "", "auto", "Auto", "AUTO"):
        return recommended_elev_zoom(scale, origin_lat)
    try:
        return max(ELEV_ZOOM_MIN, min(ELEV_ZOOM_MAX, int(raw)))
    except (TypeError, ValueError):
        return recommended_elev_zoom(scale, origin_lat)


def build_arnis_cmd(arnis_exe: str, bbox: dict, output_path: str,
                    settings: dict, origin: dict, elevation: dict | None,
                    seed: int, osm_file: str | None = None) -> list[str]:
    s, w, n, e = bbox["south"], bbox["west"], bbox["north"], bbox["east"]
    scale = float(settings.get("scale", 1.0) or 1.0)
    cmd = [
        str(arnis_exe),
        "--bbox", f"{s},{w},{n},{e}",
        "--output-dir", str(output_path),
        "--scale", str(scale),
        f"--ground-level={int(settings.get('ground_level', -62))}",
        "--rotation", str(settings.get("rotation", 0)),
    ]

    # Pre-fetched OSM. Two shapes, both Overpass-free at generation time:
    #   • a DIRECTORY → Meld's stable z11 grid cache. Arnis computes this cell's covering
    #     tiles from --bbox and reads them straight from the dir (--osm-tile-dir), so there
    #     is NO per-cell clump-merge step on Meld's side — the slow "assembling" phase.
    #   • a FILE → a single pre-merged Overpass JSON (legacy / live-fetched cell). Arnis
    #     clips its elements to --bbox.
    # When osm_file is None, Arnis fetches Overpass itself (original behaviour).
    if osm_file:
        if os.path.isdir(osm_file):
            cmd += ["--osm-tile-dir", str(osm_file), "--osm-tile-z", str(OSM_GRID_Z)]
        else:
            cmd += ["--file", str(osm_file)]

    # Global origin + deterministic building palette (the seamless-tiling pair).
    if origin and origin.get("lat") is not None and origin.get("lon") is not None:
        cmd += ["--master-origin-lat", str(origin["lat"])]
        cmd += ["--master-origin-lng", str(origin["lon"])]
        if settings.get("tile_invariant_rendering", True):
            # v2.8.3 exposes --seed (alias of --tile-invariant-rendering). u64,
            # rejects negatives → clamp.
            safe_seed = (int(seed or 1) & 0xFFFFFFFFFFFFFFFF) or 1
            cmd += ["--seed", str(safe_seed)]

    # Terrain is OFF by default in the fork — turn it on for real elevation.
    if settings.get("terrain", True):
        cmd.append("--terrain")
        # Vertical exaggeration: scale mountain HEIGHT (not footprint). 1.0 = true scale.
        try:
            ve = float(settings.get("vertical_exaggeration", 1.0) or 1.0)
        except (TypeError, ValueError):
            ve = 1.0
        if abs(ve - 1.0) > 1e-9:
            cmd += ["--vertical-exaggeration", str(ve)]
        # Snow caps: off | realistic (latitude line) | peaks (top N%) | manual (above a Y).
        snow_mode = str(settings.get("snow_mode", "realistic") or "realistic").strip().lower()
        if snow_mode in ("off", "realistic", "peaks", "manual"):
            cmd += ["--snow-mode", snow_mode]
            if snow_mode == "peaks":
                cmd += ["--snow-percent",
                        str(float(settings.get("snow_percent", 6.0) or 6.0))]
            elif snow_mode == "manual":
                cmd += ["--snow-y", str(int(settings.get("snow_y", 80) or 80))]
    cmd += ["--roof", "true" if settings.get("roof", True) else "false"]
    cmd += ["--interior", "true" if settings.get("interior", False) else "false"]
    cmd += ["--land-cover", "true" if settings.get("land_cover", True) else "false"]
    # Skip OSM buildings (keeps roads, bridges, railways, land cover, water, terrain).
    if not settings.get("buildings", True):
        cmd.append("--no-buildings")
    if settings.get("fill_ground"):
        cmd.append("--fillground")
    if settings.get("disable_height_limit"):
        cmd.append("--disable-height-limit")
    # NOTE: stream-to-disk is NOT a CLI flag in the merged Arnis (upstream removed the
    # flag in eebecb5; it's now the ARNIS_STREAM_TO_DISK env var + a RAM heuristic).
    # Meld sets that env per-cell in server._runner for big cells, so nothing is added
    # to argv here. See run_arnis(env=...).
    # Bake chunk lighting so LOD mods (Voxy, Distant Horizons) render distant chunks
    # lit without visiting them (Arnis issue #1071). Default on.
    if settings.get("bake_lighting", True):
        cmd.append("--bake-lighting")
    if settings.get("timeout"):
        cmd += ["--timeout", str(int(settings["timeout"]))]

    # Global elevation lock → consistent Y mapping across all cells (no cliffs).
    # The fork only consumes --elevation-min/max inside its `if args.terrain`
    # path, so emitting them without --terrain would silently do nothing. Gate on
    # terrain so the no-cliff guarantee can't be silently broken.
    if (settings.get("terrain", True)
            and settings.get("elevation_mode", "global") == "global" and elevation
            and elevation.get("min_m") is not None and elevation.get("max_m") is not None):
        cmd += ["--elevation-min", str(elevation["min_m"])]
        cmd += ["--elevation-max", str(elevation["max_m"])]

    # AWS-only elevation: skip the regional hi-res providers (USGS / IGN / GSI). Those are
    # great single-shot but flaky under Meld's parallel burst (many cells hit them at once ->
    # "Elevation request retry" per tile -> slow), and the terrain prefetch only warms AWS.
    # On for big parallel runs trades ~30m AWS for far fewer retries.
    if settings.get("terrain", True) and settings.get("aws_only_elevation"):
        cmd.append("--aws-only-elevation")

    # Road detail — auto: compact below scale 0.7, clean at/above. max => omit.
    rd = (settings.get("road_detail_level") or "auto").strip().lower()
    if rd == "auto":
        rd = "compact" if scale < 0.7 else "clean"
    if rd in ("compact", "clean"):
        cmd += ["--road-detail", rd]

    # Overpass endpoint override — only relevant when actually querying Overpass
    # (i.e. no pre-fetched --file for this cell).
    if not osm_file:
        op = settings.get("overpass_url") or []
        if isinstance(op, str):
            op = [u.strip() for u in op.split(",") if u.strip()]
        if op:
            cmd += ["--overpass-url", ",".join(op)]

    # 3D models: v2.8.3 fetches 3D structure models (3DMR + Wikimedia) by default;
    # --no-3d disables them. Meld defaults 3D OFF, so emit --no-3d unless the user
    # ticked the 3D toggle in the UI.
    if not settings.get("generate_3d_models", False):
        cmd.append("--no-3d")

    # Schematic trees (default on): stamp a bundled region pack so the fork places detailed
    # schematic trees instead of procedural ones. The realm is picked from the selection centre
    # (Auto) or forced via the "tree_realm" setting; the fork loads <realm>/region.json and the
    # sibling vanilla-plus for the 85/12/3 blend. Packs live in light-meld/tree-packs/ (gitignored).
    if settings.get("trees", True):
        tp_root = os.path.join(
            os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
            "tree-packs",
        )
        choice = str(settings.get("tree_realm", "auto") or "auto").strip().lower()
        if choice in ("", "auto"):
            realm = realm_for_latlon((s + n) / 2.0, (w + e) / 2.0)
        else:
            realm = choice
        pack = os.path.join(tp_root, realm)
        if not os.path.isdir(pack):
            pack = os.path.join(tp_root, "vanilla-plus")  # fallback
        if os.path.isdir(pack):
            cmd += ["--tree-pack", pack]

        # Size-tier toggle (5 stages: small/medium/big/tall/giant). The UI stores `tree_sizes` as a
        # dict of bools or a list of enabled tiers; default keeps all but Giant (the very-tall 29+
        # tier stays off). Giant only renders at 1:1 even when enabled (gated in the fork).
        TIERS = ("small", "medium", "big", "tall", "giant")
        ts = settings.get("tree_sizes")
        if isinstance(ts, dict):
            enabled = [t for t in TIERS if ts.get(t)]
        elif isinstance(ts, (list, tuple)):
            enabled = [str(t).strip().lower() for t in ts if str(t).strip().lower() in TIERS]
        else:
            enabled = ["small", "medium", "big", "tall"]
        if enabled:
            cmd += ["--tree-sizes", ",".join(enabled)]
    return cmd


def find_world_dir(output_path: str) -> str | None:
    """Arnis creates a world subfolder (e.g. 'Arnis World 1') containing region/.
    Return the path to the dir that holds a region/ folder, or None.

    Picks the MOST RECENTLY MODIFIED matching subdir, not the lexicographically
    first. If clean_output_dir failed to remove a stale 'Arnis World 1' (Windows
    file lock / AV), Arnis writes a fresh 'Arnis World 2'; a lexical scan would
    wrongly return the stale world. mtime always picks the fresh generation."""
    base = Path(output_path)
    if (base / "region").is_dir():
        return str(base)
    if base.is_dir():
        candidates = [c for c in base.iterdir()
                      if c.is_dir() and (c / "region").is_dir()]
        if candidates:
            newest = max(candidates, key=lambda c: c.stat().st_mtime)
            return str(newest)
    return None


def clean_output_dir(output_path: str) -> None:
    """Remove leftover incomplete worlds so Arnis always creates 'World 1'."""
    base = Path(output_path)
    if not base.exists():
        return
    import shutil
    for child in base.iterdir():
        try:
            if child.is_dir():
                shutil.rmtree(child, ignore_errors=True)
            else:
                child.unlink(missing_ok=True)
        except Exception:
            pass


_PROGRESS_RE = re.compile(r"(\d+)\s*/\s*(\d+)")


def parse_progress(line: str, current: int) -> int:
    """Best-effort progress percent from Arnis stdout. Monotonic, capped 95."""
    new = current
    low = line.lower()
    for kw, pct in (("fetching", 8), ("processing", 20), ("ground", 35),
                    ("generating", 55), ("saving", 90), ("done", 100)):
        if kw in low and pct > new:
            new = pct
    m = _PROGRESS_RE.search(line)
    if m:
        try:
            done, total = int(m.group(1)), int(m.group(2))
            if 0 < done <= total and total > 10:
                mapped = int(35 + (done / total) * 53)
                new = max(new, min(95, mapped))
        except Exception:
            pass
    return new


def run_arnis(cmd: list[str], cwd: str, on_line=None, on_proc=None,
              env: dict | None = None) -> bool:
    """Run Arnis, streaming stdout line-by-line to on_line(text). Returns ok.

    on_proc(proc) is called once with the Popen handle so the caller can publish
    it (e.g. to worker state) for termination via /api/stop. It's cleared with
    on_proc(None) before returning.

    env (optional) is overlaid on the inherited environment for THIS child only
    (used to pin RAYON_NUM_THREADS so N parallel cells don't oversubscribe cores,
    and ARNIS_STREAM_TO_DISK=1 for big cells). The post-merge Arnis reads both;
    an older binary harmlessly ignores them."""
    child_env = {**os.environ, **(env or {})}
    proc = subprocess.Popen(
        cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        text=True, bufsize=1, cwd=str(cwd), env=child_env,
    )
    if on_proc:
        on_proc(proc)
    try:
        for raw in proc.stdout:                       # type: ignore[union-attr]
            if on_line:
                on_line(raw.rstrip())
        proc.wait()
        return proc.returncode == 0
    except Exception:
        try:
            proc.kill()
        except Exception:
            pass
        return False
    finally:
        if on_proc:
            on_proc(None)
