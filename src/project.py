"""
project.py — persisted project state. One project = one origin (locked), one
settings blob, one elevation lock + seed, one grid status map, one master world.

No database; just project.json + grid.json on disk.
"""

from __future__ import annotations

import json
import os
import threading
from pathlib import Path

_LOCK = threading.Lock()


def default_settings() -> dict:
    return {
        # 1:10 default so a first build is fast and a whole city fits. 1:1 (real size) is huge + slow;
        # users raise it deliberately. The guided (Simplified) UI starts here.
        "scale": 0.1,
        "job_size_regions": 4,   # sweet spot: small save bursts, safe on any disk (see Workers note)
        "seam_buffer_chunks": 8,    # 8 chunks = 128 blocks of overlap per side
        "ground_level": -56,
        "rotation": 0,
        "terrain": True,
        # Skip regional hi-res elevation (USGS/IGN/GSI) -> AWS-only. Fewer per-tile retries on
        # big parallel runs (the regional providers rate-limit under Meld's burst), at ~30m res.
        "aws_only_elevation": False,
        # Terrarium zoom used for elevation (pack download + Arnis generation). "auto" matches the
        # zoom's pixel to the block size for this scale (the right detail with no waste): 1:1->z15,
        # 1:10->z13, etc. Lower zoom = far fewer tiles + dodges the z14/z15 no-data holes. 11..15.
        "elevation_zoom": "auto",
        # Off by default for the fastest first build (roads + land cover + water + terrain only).
        # Turning it on is remembered per project via update_settings. roads/bridges/rails/water kept.
        "buildings": False,
        "roof": True,
        "interior": False,
        "land_cover": True,
        "fill_ground": True,   # solid floor under the surface, no holes
        "osm_bake_workers": 4,  # offline .pbf bake parallelism; UI caps at 8, auto from CPU cores
        "disable_height_limit": False,
        # Pre-bake per-chunk lighting so LOD mods (Voxy, Distant Horizons) render
        # distant chunks lit without visiting them (Arnis issue #1071). On by default
        # because Meld builds areas too large to fly through; slower + bigger files.
        "bake_lighting": True,
        "road_detail_level": "auto",       # auto: compact <0.7, clean >=0.7
        "trees": True,                      # stamp bundled schematic trees (off = procedural)
        "tree_realm": "auto",               # auto: realm from selection latlon; or a realm code
        # 5 height tiers to place (small <=6, medium 7-12, big 13-20, tall 21-28, giant 29-40 blocks).
        # Giant (very tall) is OFF by default + only renders at 1:1; tall is rare. Off tiers fall back.
        "tree_sizes": {"small": True, "medium": True, "big": True, "tall": True, "giant": False},
        "elevation_mode": "global",         # global = locked range, no cliffs
        # Vertical exaggeration: multiplies terrain HEIGHT only (not footprint). 1.0 = true scale;
        # 2-3 = dramatic mountains at the same map size. Auto-compresses to the build height.
        "vertical_exaggeration": 1.0,
        # Snow caps: off | realistic (real latitude snow line) | peaks (top N% of world height) |
        # manual (above snow_y). Default peaks so mountains always get a believable cap.
        "snow_mode": "peaks",
        "snow_percent": 6.0,
        "snow_y": 80,
        "tile_invariant_rendering": True,
        "generate_3d_models": False,        # reserved no-op in this fork (light-docs/05)
        "poi_3d_only": True,                # reserved
        "overpass_url": "",
        "timeout": 600,
        # Generation is mostly CPU bound now. The rule that matters: keep workers x threads at or
        # under your CPU cores. Going over OVERSUBSCRIBES the cores and slows the build. With the
        # default 4 workers x 4 threads = 16, fine on any 8+ core machine. Recommend tunes it to
        # the box (and still caps on RAM + save-disk speed as secondary safety).
        "max_workers": 4,
        # CPU core budget Meld spreads across workers. Each child gets
        # max(min_threads_per_worker, floor(cores*pct/100) / max_workers) rayon threads.
        # 95 (slider max) = use nearly the whole machine; lower leaves headroom for the OS +
        # disk-save phase. >100 (not reachable from the slider) oversubscribes. Default 90.
        "cpu_target_pct": 90,
        # Threads each worker (cell) uses for its in-process tile parallelism. The actual count is
        # max(this, floor(cores*pct/100) / max_workers). Keep workers x this AT OR UNDER your cores;
        # over the core count slows the build. On 24 cores, 12 workers x 2 and 8 x 3 perform about
        # the same, so the exact split barely matters as long as the product stays under the cores.
        "min_threads_per_worker": 4,
        # Per-worker first-job start delay (seconds) to desync CPU phases. Small on
        # purpose; big values just make generation look slow to start. Slider 1-4s.
        "cpu_stagger_seconds": 2,
        # Master toggle for the stagger. Off = all workers start at once (spikier CPU but
        # nothing sits idle at launch).
        "cpu_stagger_enabled": True,
        # Adaptive: pace worker starts from the observed average cell time (so each worker
        # enters the CPU phase as the previous frees). Off = fixed slider step.
        "cpu_stagger_adaptive": True,
        # OSM prefetch: download the selection's OSM once (one serial request, split
        # into 4 only on failure) and feed every cell via --file, so parallel
        # generation never hits the Overpass rate limit. See src/prefetch.py.
        "prefetch_enabled": True,
        "prefetch_margin_m": 256,   # metres added around each chunk so border buildings stay whole
        # Max real-world km² per shared OSM download tile (each tile = one Overpass query). 0 =
        # AUTO: download the whole selection in one query, or a handful of big tiles if it's huge
        # (cap ~30,000 km²), then auto-split any tile the server rejects. Because this is real-world
        # area it behaves identically at 1:1 and 1:10. The UI slider sets it as a tile EDGE in km
        # (stored here as edge²); raise it for fewer/bigger tiles, lower for a strict endpoint.
        "prefetch_tile_km2": 0,
        # How many OSM tiles download at once. 2 = the public Overpass per-IP slot allowance
        # (halves prefetch time without tripping the rate limit). Capped at 4 for private endpoints.
        "prefetch_concurrency": 2,
        # Region data pack: how many elevation tiles the bulk downloader pulls at once. 16 keeps
        # one controlled process well under any S3 throttle (vs the per-cell burst that flat-seams).
        "datapack_tile_concurrency": 16,
        # Pre-warm AWS terrain tiles once (serial, single process) before the parallel cells,
        # so the cells hit the cache instead of bursting S3 (which truncates tiles -> flat seams).
        "prefetch_terrain": True,
        # Stream regions to disk during generation (upstream Arnis --stream-to-disk). Lets a
        # single cell be 8x8/16x16 without OOM. Only used if the arnis binary supports the flag.
        "stream_to_disk": False,
        # World management
        "prune_cell_after_merge": True,   # delete per-cell subregion after merge (saves storage)
        "master_world_dir": "",            # where the merged world lives ("" = <project>/Meld World)
        "origin_corner": "nw",             # which selection corner the origin snaps to on Plan
    }


class Project:
    def __init__(self, root: Path):
        self.root = Path(root)
        self.root.mkdir(parents=True, exist_ok=True)
        self.json_path = self.root / "project.json"
        self.grid_path = self.root / "grid.json"
        self.master_world = self.root / "Meld World"   # merged world folder name
        self.cells_dir = self.root / "cells"

    # ── low-level IO (no lock — callers that mutate hold _LOCK) ──────────────
    def _read(self, path: Path, default):
        if path.exists():
            try:
                return json.loads(path.read_text(encoding="utf-8"))
            except Exception:
                pass
        return default

    def _write(self, path: Path, data) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(data, indent=2), encoding="utf-8")

    def _default_project(self) -> dict:
        return {
            "origin": {"lat": None, "lon": None, "locked": False},
            "settings": default_settings(),
            "elevation": {"min_m": None, "max_m": None, "seed": 1, "locked": False},
            "name": "Meld World",
        }

    @staticmethod
    def _clamp_seed(seed) -> int:
        try:
            s = int(seed)
        except (TypeError, ValueError):
            return 1
        s &= 0xFFFFFFFFFFFFFFFF      # the fork parses this as u64; reject negatives
        return s or 1

    # ── project.json ────────────────────────────────────────────────────────
    def load(self) -> dict:
        return self._read(self.json_path, self._default_project())

    def save(self, data: dict) -> None:
        with _LOCK:
            self._write(self.json_path, data)

    # ── drawn selection (so a restart redraws the area, per-project) ──────────
    def load_selection(self) -> dict | None:
        """The drawn area for THIS project: {bbox:{south,west,north,east}, polygons|None}. The grid
        cells already persist in grid.json; this persists the OUTLINE + lets the UI restore the live
        selection so coverage / data-pack / generate work right after a restart without re-drawing."""
        sel = self.load().get("selection")
        return sel if isinstance(sel, dict) and sel.get("bbox") else None

    def save_selection(self, sel: dict | None) -> None:
        """Persist (sel with a 'bbox') or clear (None) the selection in project.json, per-project."""
        with _LOCK:
            data = self._read(self.json_path, self._default_project())
            if isinstance(sel, dict) and sel.get("bbox"):
                data["selection"] = {"bbox": sel["bbox"], "polygons": sel.get("polygons")}
            else:
                data.pop("selection", None)
            self._write(self.json_path, data)

    # ── origin (locked once) ──────────────────────────────────────────────────
    def set_origin(self, lat: float, lon: float, force: bool = False) -> dict:
        with _LOCK:
            data = self._read(self.json_path, self._default_project())
            if data["origin"].get("locked") and not force:
                return {"ok": False, "error": "origin already locked",
                        "origin": data["origin"]}
            data["origin"] = {"lat": float(lat), "lon": float(lon), "locked": True}
            self._write(self.json_path, data)
            return {"ok": True, "origin": data["origin"]}

    def unlock_origin(self) -> dict:
        """Clear the origin lock so it can be moved/relocked. Keeps lat/lon."""
        with _LOCK:
            data = self._read(self.json_path, self._default_project())
            if data.get("origin"):
                data["origin"]["locked"] = False
            self._write(self.json_path, data)
            return data.get("origin", {"lat": None, "lon": None, "locked": False})

    def origin(self) -> dict:
        return self.load().get("origin", {"lat": None, "lon": None, "locked": False})

    def subworld_number(self, cell_key: str) -> int:
        """Stable 'Meld Sub World N' number for a cell. Assigns the next unused
        integer the first time a cell is seen and reuses it after — no duplicates."""
        with _LOCK:
            data = self._read(self.json_path, self._default_project())
            sw = data.get("subworlds") or {}
            if cell_key in sw:
                return int(sw[cell_key])
            n = (max(int(v) for v in sw.values()) + 1) if sw else 1
            sw[cell_key] = n
            data["subworlds"] = sw
            self._write(self.json_path, data)
            return n

    def set_name(self, name: str) -> str:
        name = (name or "").strip() or "Meld World"
        with _LOCK:
            data = self._read(self.json_path, self._default_project())
            data["name"] = name
            self._write(self.json_path, data)
            return name

    def settings(self) -> dict:
        return {**default_settings(), **(self.load().get("settings") or {})}

    def update_settings(self, patch: dict) -> dict:
        # Drop None values so a blank UI field can't poison a setting.
        patch = {k: v for k, v in (patch or {}).items() if v is not None}
        with _LOCK:
            data = self._read(self.json_path, self._default_project())
            data["settings"] = {**default_settings(), **(data.get("settings") or {}), **patch}
            self._write(self.json_path, data)
            return data["settings"]

    def set_elevation_lock(self, min_m: float, max_m: float, seed=None) -> dict:
        with _LOCK:
            data = self._read(self.json_path, self._default_project())
            ev = data.get("elevation") or {}
            ev.update(min_m=float(min_m), max_m=float(max_m), locked=True)
            if seed is not None:
                ev["seed"] = self._clamp_seed(seed)
            ev["seed"] = self._clamp_seed(ev.get("seed", 1))
            data["elevation"] = ev
            self._write(self.json_path, data)
            return ev

    def set_seed(self, seed) -> int:
        """Persist only the project seed (does not touch the elevation lock)."""
        with _LOCK:
            data = self._read(self.json_path, self._default_project())
            ev = data.get("elevation") or {"min_m": None, "max_m": None, "locked": False}
            ev["seed"] = self._clamp_seed(seed)
            data["elevation"] = ev
            self._write(self.json_path, data)
            return ev["seed"]

    def elevation(self) -> dict:
        return self.load().get("elevation", {"min_m": None, "max_m": None, "seed": 1, "locked": False})

    # ── grid.json (cell_key -> status) ───────────────────────────────────────
    def load_grid(self) -> dict:
        return self._read(self.grid_path, {})

    def save_grid(self, grid: dict) -> None:
        with _LOCK:
            self._write(self.grid_path, grid)

    def set_cell_status(self, cell_key: str, status: str) -> None:
        # Atomic read-modify-write so concurrent workers don't clobber each
        # other's statuses (the grid is the parallel pipeline's source of truth).
        with _LOCK:
            grid = self._read(self.grid_path, {})
            grid[cell_key] = status
            self._write(self.grid_path, grid)

    def bulk_set_cells(self, keys: list[str], op: str) -> int:
        """Add ('add' -> 'planned') or remove ('remove') many cells in ONE locked write.
        Merged cells are never touched. Returns the number actually changed."""
        with _LOCK:
            grid = self._read(self.grid_path, {})
            n = 0
            for k in keys:
                cur = grid.get(k)
                if cur == "merged":
                    continue
                if op == "add" and cur is None:
                    grid[k] = "planned"; n += 1
                elif op == "remove" and cur is not None:
                    grid.pop(k, None); n += 1
            if n:
                self._write(self.grid_path, grid)
            return n
