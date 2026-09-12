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

# Region containers a cell may be generated in. `.b_linear` is Leaf's B_Linear v3,
# written natively by the fork when a project turns on native region generation; the
# merge treats both identically because it never looks inside a region file.
REGION_SUFFIXES = (".mca", ".b_linear")

_MCA_RE = re.compile(r"^r\.(-?\d+)\.(-?\d+)\.(?:mca|b_linear)$")

# Serialises mutation of the shared master world (level.dat copy/patch) across
# the concurrent worker threads. Region writes are disjoint per cell so they
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


def _region_files(directory: Path):
    """Region files of any supported container, in a stable order."""
    for suffix in REGION_SUFFIXES:
        yield from sorted(directory.glob(f"*{suffix}"))


def world_container(world_path: str | Path) -> str | None:
    """Which container a world's region/ directory holds: "mca", "b_linear", or None.

    Returns None for an empty or missing directory. A world holding both is a bug
    upstream of here (a cell generated in the other container, or a stale sibling left
    by a format switch), so callers treat "mixed" as an error rather than merging.
    """
    region_dir = Path(world_path) / "region"
    if not region_dir.is_dir():
        return None
    found = {suffix.lstrip(".") for suffix in REGION_SUFFIXES
             if any(region_dir.glob(f"r.*{suffix}"))}
    if not found:
        return None
    if len(found) > 1:
        return "mixed"
    return found.pop()


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
        "datapacks": "skipped",
        "voxy": "skipped",
    }

    # ── PRE-FLIGHT (read-only) ───────────────────────────────────────────────
    # A master world holds exactly one container. Merging a cell of the other kind
    # would leave a world no server can open as a whole, and because the two use
    # different file names the mismatch would not even show up as a collision.
    cell_container = world_container(cell_p)
    master_container = world_container(master_p)
    if cell_container == "mixed" or master_container == "mixed":
        raise MeldCollisionError(
            f"{cell_key}: a world already holds both .mca and .b_linear regions "
            f"(cell={cell_container}, master={master_container}). Merge refused — "
            f"no files changed."
        )
    if cell_container and master_container and cell_container != master_container:
        raise MeldCollisionError(
            f"{cell_key}: cell was generated as .{cell_container} but the master world "
            f"is .{master_container}. Regenerate the cell with the project's region "
            f"format. Merge refused — no files changed."
        )

    copy_plans: list[tuple[Path, Path, str]] = []   # (src, dst, sub)
    collisions: list[str] = []
    region_coords: list[tuple[int, int]] = []       # all region files (canonical + seam)

    for sub in ("region", "poi", "entities"):
        cell_sub = cell_p / sub
        if not cell_sub.exists():
            continue
        master_sub = master_p / sub
        for src in _region_files(cell_sub):
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

    # datapacks/ MUST reach the master world, and BEFORE level.dat does.
    #
    # Arnis installs its extended-height pack into the CELL world and registers
    # `file/arnis_tall` inside that cell's level.dat. Copying the level.dat without the
    # pack files leaves the master world asking for a datapack that isn't there;
    # Minecraft then loads it at vanilla height and every block above y=319 of an
    # extended world is gone. Copying packs first means an interrupted merge can leave
    # spare packs but never a level.dat referencing a missing one.
    #
    # Idempotent and run on every cell, so a world whose first merge predates this fix,
    # or one where the pack appeared part-way through, gets backfilled.
    if (cell_p / "datapacks").is_dir():
        with _MASTER_LOCK:
            master_p.mkdir(parents=True, exist_ok=True)
            result["datapacks"] = _copy_datapacks(cell_p, master_p)

    # voxy/ — the generator's LOD cache, when --voxy-lod built one. The merge is a
    # file-level copy of region files; this directory is a single database keyed on the
    # world seed, so it cannot be merged cell by cell. It is carried over only when the
    # master has none yet, which is the single-cell render the flag is allowed for (see
    # the gate in server._runner). A world that later grows more cells keeps the LOD it
    # has and voxy fills the rest when those chunks are visited.
    if (cell_p / "voxy").is_dir():
        with _MASTER_LOCK:
            dst_voxy = master_p / "voxy"
            if dst_voxy.exists():
                result["voxy"] = "already present"
            else:
                master_p.mkdir(parents=True, exist_ok=True)
                shutil.copytree(cell_p / "voxy", dst_voxy)
                result["voxy"] = "copied"

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


def _copy_datapacks(cell_p: Path, master_p: Path) -> str:
    """Copy the cell world's datapacks/ into the master world, skipping packs already
    there. Call under _MASTER_LOCK. Returns a short status for the merge result."""
    src_root = cell_p / "datapacks"
    if not src_root.is_dir():
        return "none"
    copied = []
    for pack in sorted(src_root.iterdir()):
        dst = master_p / "datapacks" / pack.name
        if dst.exists():
            continue
        dst.parent.mkdir(parents=True, exist_ok=True)
        if pack.is_dir():
            shutil.copytree(pack, dst)
        else:
            shutil.copy2(pack, dst)
        copied.append(pack.name)
    return ("copied: " + ", ".join(copied)) if copied else "already present"


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
        for f in _region_files(d):
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
