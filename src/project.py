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
        "scale": 1.0,
        "job_size_regions": 4,   # sweet spot: small save bursts, safe on any disk (see Workers note)
        "seam_buffer_chunks": 8,    # 8 chunks = 128 blocks of overlap per side
        "ground_level": -56,
        "rotation": 0,
        "terrain": True,
        "roof": True,
        "interior": False,
        "land_cover": True,
        "fill_ground": False,
        "disable_height_limit": False,
        # Pre-bake per-chunk lighting so LOD mods (Voxy, Distant Horizons) render
        # distant chunks lit without visiting them (Arnis issue #1071). On by default
        # because Meld builds areas too large to fly through; slower + bigger files.
        "bake_lighting": True,
        "road_detail_level": "auto",       # auto: compact <0.7, clean >=0.7
        "elevation_mode": "global",         # global = locked range, no cliffs
        "tile_invariant_rendering": True,
        "generate_3d_models": False,        # reserved no-op in this fork (light-docs/05)
        "poi_3d_only": True,                # reserved
        "overpass_url": "",
        "timeout": 600,
        "max_workers": 4,   # sweet spot: ~4x speedup, safe save load. Up to 16 (8+ warns).
        # OSM prefetch: download the selection's OSM once (one serial request, split
        # into 4 only on failure) and feed every cell via --file, so parallel
        # generation never hits the Overpass rate limit. See src/prefetch.py.
        "prefetch_enabled": True,
        "prefetch_margin_m": 256,   # metres added around each chunk so border buildings stay whole
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
