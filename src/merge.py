"""
merge.py — copy a generated cell's CANONICAL region files into the master
world, discarding the seam-buffer regions.

Light-meld assumes native global coordinates: the Arnis fork was run with
--master-origin-lat/lng, so region files are already named at their global
positions. The canonical region rectangle is derived exactly from the cell_key
(light-docs/02). Two safety mechanisms:

  1. Canonical strip — keep only files inside the cell's owned region rectangle;
     discard the seam buffer.
  2. Drift guard — if the generated files do not cover the canonical rectangle
     within the seam buffer, the coordinate convention is broken (e.g. the fork
     regressed to avg_lat). Refuse the merge LOUDLY instead of silently leaving
     grass strips (light-docs/03, decided fix = patch + guard).
"""

from __future__ import annotations

import math
import re
import shutil
import threading
from pathlib import Path

from .constants import REGION_CHUNKS
from .coords import canonical_region_bounds
from .level_dat import patch_level_name, gold_name

_MCA_RE = re.compile(r"^r\.(-?\d+)\.(-?\d+)\.mca$")

# Serialises mutation of the shared master world (level.dat copy/patch) across
# the concurrent worker threads. Region .mca writes are disjoint per cell so they
# need no lock; level.dat is a single shared file and must not be rewritten by two
# workers at once.
_MASTER_LOCK = threading.Lock()


class MeldCollisionError(RuntimeError):
    pass


class MeldCoordinateDriftError(RuntimeError):
    """Generated regions don't cover the canonical rectangle — coords are broken."""


def _parse_mca(name: str) -> tuple[int, int] | None:
    m = _MCA_RE.match(name)
    if not m:
        return None
    return int(m.group(1)), int(m.group(2))


def merge_cell_into_master(
    cell_world_path: str,
    master_world_path: str,
    cell_key: str,
    seam_buffer_chunks: int = 8,
    world_name: str | None = None,
    overwrite_collisions: bool = False,
) -> dict:
    """
    Merge one cell world into the master world.

    All validation is a read-only PRE-FLIGHT before any filesystem mutation, so a
    refused merge leaves the master genuinely unchanged. Raises:
      - MeldCoordinateDriftError ONLY on gross displacement: the generated region
        files do not reach the canonical rectangle within the seam buffer (real
        coordinate-convention break, e.g. the avg_lat shear), or zero region files
        were produced (empty/failed run). A few missing edge/corner regions — a
        normal Arnis edge effect or a partial/interrupted run — is NOT drift.
      - MeldCollisionError on canonical collisions (unless overwrite_collisions).
    """
    cell_p = Path(cell_world_path)
    master_p = Path(master_world_path)

    bounds = canonical_region_bounds(cell_key)
    if bounds is None:
        raise ValueError(f"merge: bad cell_key {cell_key!r}")
    rx_min, rx_max, rz_min, rz_max = bounds

    result = {
        "cell_key": cell_key,
        "canonical_bounds": bounds,
        "regions_copied": 0,
        "regions_skipped": 0,
        "collisions": 0,
        "subdirs_merged": [],
        "level_dat": "skipped",
    }

    # ── PRE-FLIGHT (read-only) ───────────────────────────────────────────────
    copy_plans: list[tuple[Path, Path, str]] = []   # (src, dst, sub)
    collisions: list[str] = []
    region_coords: list[tuple[int, int]] = []       # all region files (canonical + seam)

    for sub in ("region", "poi", "entities"):
        cell_sub = cell_p / sub
        if not cell_sub.exists():
            continue
        master_sub = master_p / sub
        for src in cell_sub.glob("*.mca"):
            rc = _parse_mca(src.name)
            if rc is not None:
                frx, frz = rc
                if sub == "region":
                    region_coords.append((frx, frz))
                if not (rx_min <= frx <= rx_max and rz_min <= frz <= rz_max):
                    result["regions_skipped"] += 1
                    continue  # seam-buffer region — discard
            dst = master_sub / src.name
            if dst.exists():
                collisions.append(f"{sub}/{src.name}")
            copy_plans.append((src, dst, sub))

    # Drift guard: gross displacement only. Real coordinate drift moves content
    # MANY regions away, so the generated extent fails to reach the canonical
    # rectangle within the seam buffer. A few missing edge regions (Arnis edge
    # effect, or a partial/interrupted run) is tolerated — only a wholesale miss
    # or a totally empty generation is refused.
    buffer_regions = math.ceil(max(0, seam_buffer_chunks) / REGION_CHUNKS) + 1
    rxs = [c[0] for c in region_coords]
    rzs = [c[1] for c in region_coords]
    if not rxs:
        raise MeldCoordinateDriftError(
            f"{cell_key}: generation produced no region files (empty/failed run). "
            f"Merge refused — no files changed."
        )
    if (min(rxs) > rx_min + buffer_regions or max(rxs) < rx_max - buffer_regions or
            min(rzs) > rz_min + buffer_regions or max(rzs) < rz_max - buffer_regions):
        raise MeldCoordinateDriftError(
            f"{cell_key}: generated regions X[{min(rxs)}..{max(rxs)}] "
            f"Z[{min(rzs)}..{max(rzs)}] do not reach canonical "
            f"X[{rx_min}..{rx_max}] Z[{rz_min}..{rz_max}] within the seam buffer "
            f"({buffer_regions} regions). Coordinate-convention mismatch "
            f"(see light-docs/03). Merge refused — no files changed."
        )

    if collisions and not overwrite_collisions:
        result["collisions"] = len(collisions)
        raise MeldCollisionError(
            f"{len(collisions)} collision(s): " + ", ".join(collisions[:5])
            + (f" … (+{len(collisions) - 5} more)" if len(collisions) > 5 else "")
            + ". Merge aborted — no files changed. (Re-running a cell? pass "
            "overwrite_collisions=True — its canonical regions are uniquely owned.)"
        )
    result["collisions"] = len(collisions)

    # ── MUTATE ───────────────────────────────────────────────────────────────
    subs_seen: set[str] = set()
    for src, dst, sub in copy_plans:
        dst.parent.mkdir(parents=True, exist_ok=True)
        subs_seen.add(sub)
        shutil.copy2(src, dst)
        result["regions_copied"] += 1
    result["subdirs_merged"] = sorted(subs_seen)

    # level.dat — copy once, patch the name. Serialised across concurrent merges.
    src_dat = cell_p / "level.dat"
    dst_dat = master_p / "level.dat"
    if src_dat.exists():
        with _MASTER_LOCK:
            if not dst_dat.exists():
                master_p.mkdir(parents=True, exist_ok=True)
                shutil.copy2(src_dat, dst_dat)
                if world_name and patch_level_name(dst_dat, gold_name(world_name)):
                    result["level_dat"] = "copied+renamed"
                else:
                    result["level_dat"] = "copied"

    return result


def strip_buffer_regions(cell_world_path: str, cell_key: str) -> int:
    """Delete the seam-buffer region files from a cell world, leaving ONLY the
    canonical regions the cell owns. With the buffer regions gone, every kept
    subregion holds a DISJOINT set of region files, so you can drag-and-drop all
    their region/ (poi/, entities/) files straight into one master world with no
    collisions, no Meld merge needed. Returns the count removed."""
    bounds = canonical_region_bounds(cell_key)
    if bounds is None:
        return 0
    rx_min, rx_max, rz_min, rz_max = bounds
    p = Path(cell_world_path)
    removed = 0
    for sub in ("region", "poi", "entities"):
        d = p / sub
        if not d.exists():
            continue
        for f in d.glob("*.mca"):
            rc = _parse_mca(f.name)
            if rc is None:
                continue
            frx, frz = rc
            if not (rx_min <= frx <= rx_max and rz_min <= frz <= rz_max):
                try:
                    f.unlink()
                    removed += 1
                except Exception:
                    pass
    return removed
