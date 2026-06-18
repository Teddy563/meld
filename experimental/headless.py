#!/usr/bin/env python3
"""
Meld headless tools (EXPERIMENTAL, power users).

Run Meld work from the command line, without the web UI or a running server. These tools
call the exact same internals the server uses (build_arnis_cmd to build the Arnis command,
merge_cell_into_master to stitch a cell into the world), so the result is identical to a UI
generate, just scripted.

USE WITH CARE. These write directly into your world folder on disk:

  - Close Minecraft first. An open world locks the region files and the merge will fail.
  - Make sure the save drive is connected. If it is a flaky external drive, a write can be
    lost mid-merge (each cell owns its own region files, so a failure never corrupts other
    cells, but the failed cell may need a re-run).

Actions:

  repair-gaps   Find cells marked "merged" whose region files are missing from the CURRENT
                world folder (this happens if the project was renamed or the world folder
                changed, since a "merged" cell is then skipped on a normal Generate), and
                regenerate plus re-merge only those. Fills gray voids without re-running the
                whole world.

  rerun-all     Regenerate EVERY cell with the deployed arnis.exe and re-merge it. Use this
                to roll a generator fix (for example the 2.9.1 water wedge fix) across an
                existing world. Slower, but guaranteed complete.

  scan-water    Read-only. Sample region files and report the surface water fraction of each,
                to spot a residual water "wedge" (a region that is mostly water with a hard
                straight edge). High values at the map edge are usually a real sea or delta.

Examples (run from the light-meld folder):

  python experimental/headless.py scan-water  --project meld-world-8-2 --sample 24
  python experimental/headless.py repair-gaps --project meld-world-8-2
  python experimental/headless.py rerun-all   --project meld-world-8-2 --workers 12
"""
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

MELD = Path(__file__).resolve().parent.parent          # the light-meld folder
sys.path.insert(0, str(MELD))
os.chdir(MELD)

from src.arnis_cmd import build_arnis_cmd, find_world_dir          # noqa: E402
from src.merge import merge_cell_into_master                       # noqa: E402
from src.coords import cell_bbox, expand_bbox_for_seam, canonical_region_bounds  # noqa: E402


def _safe_world_name(name: str) -> str:
    """Mirror server._safe_world_name: a world-name to a folder-safe string."""
    cleaned = re.sub(r'[<>:"/\\|?*]', "_", (name or "Meld World")).strip().rstrip(".")
    return cleaned or "Meld World"


def load_project(slug: str):
    """Return (settings, origin, elevation, master_world_dir Path, project_name)."""
    proot = MELD / "projects" / slug
    pj = proot / "project.json"
    if not pj.exists():
        sys.exit(f"project not found: {pj}")
    P = json.load(open(pj, encoding="utf-8"))
    s = P.get("settings", {})
    name = _safe_world_name(P.get("name", "Meld World"))
    parent = (s.get("master_world_dir") or "").strip()
    world = (Path(parent) if parent else proot) / name
    grid = json.load(open(proot / "grid.json", encoding="utf-8")) if (proot / "grid.json").exists() else {}
    return P, s, P.get("origin", {}), P.get("elevation", {}), world, grid


def _minecraft_open() -> bool:
    if sys.platform != "win32":
        return False
    try:
        out = subprocess.run(["tasklist", "/fi", "imagename eq javaw.exe"],
                             capture_output=True, text=True, timeout=10).stdout.lower()
        return "javaw.exe" in out
    except Exception:
        return False


def _regions_on_disk(world: Path) -> set:
    rdir = world / "region"
    out = set()
    if rdir.exists():
        for n in os.listdir(rdir):
            m = re.match(r"r\.(-?\d+)\.(-?\d+)\.mca$", n)
            if m:
                out.add((int(m.group(1)), int(m.group(2))))
    return out


def _missing_cells(grid: dict, world: Path) -> list[str]:
    present = _regions_on_disk(world)
    missing = []
    for ck, st in grid.items():
        if st != "merged":
            continue
        a, b, c, d = canonical_region_bounds(ck)
        need = [(rx, rz) for rx in range(a, b + 1) for rz in range(c, d + 1)]
        if any(r not in present for r in need):
            missing.append(ck)
    return missing


def _gen_and_merge(ck, s, origin, elev, cache, env, world, seam, seed, world_name):
    rx, rz, size = (int(v) for v in ck.split(","))
    bbox = expand_bbox_for_seam(cell_bbox(rx, rz, size, origin["lat"], origin["lon"], float(s["scale"])),
                                seam, origin, float(s["scale"]))
    work = MELD / "experimental" / "_work" / ck.replace(",", "_").replace("-", "m")
    try:
        cmd = build_arnis_cmd(str(MELD / "arnis.exe"), bbox, str(work), s, origin, elev, seed, osm_file=cache)
        p = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace",
                           env=env, timeout=900)
        if p.returncode != 0:
            return ck, "gen-fail"
        wd = find_world_dir(str(work))
        if not wd:
            return ck, "no-world"
        merge_cell_into_master(wd, str(world), ck, seam_buffer_chunks=seam,
                               world_name=world_name, overwrite_collisions=True)
        return ck, "ok"
    except Exception as ex:  # noqa: BLE001
        return ck, f"err:{ex}"
    finally:
        shutil.rmtree(work, ignore_errors=True)


def run_cells(cells, s, origin, elev, world, grid_name, workers):
    if not cells:
        print("nothing to do.")
        return
    if _minecraft_open():
        sys.exit("Minecraft (javaw.exe) is running. Close it first so the world is not locked.")
    if not (world / "level.dat").exists():
        print(f"warning: {world} has no level.dat yet (a fresh world folder).")
    env = dict(os.environ)
    env["ARNIS_CACHE_ROOT"] = str((MELD / "cache").resolve())
    env["ARNIS_ELEV_ZOOM"] = str(int(elev.get("zoom", 0)) or 13) if isinstance(elev, dict) else "13"
    env["RAYON_NUM_THREADS"] = "2"
    seam = int(s.get("seam_buffer_chunks", 8) or 0)
    seed = int((elev or {}).get("seed", 1) or 1)
    cache = str((MELD / "cache" / "osm").resolve())
    shutil.rmtree(MELD / "experimental" / "_work", ignore_errors=True)
    t0 = time.time()
    done = 0
    failed = []
    with ThreadPoolExecutor(max_workers=workers) as ex:
        futs = {ex.submit(_gen_and_merge, ck, s, origin, elev, cache, env, world, seam, seed, grid_name): ck
                for ck in cells}
        for fut in as_completed(futs):
            ck, status = fut.result()
            done += 1
            if status != "ok":
                failed.append(ck)
                print(f"  FAIL {ck}: {status}", flush=True)
            if done % 25 == 0 or done == len(cells):
                el = time.time() - t0
                print(f"  {done}/{len(cells)}  ({el/60:.1f} min, {done/max(el,1):.2f} cells/s)  "
                      f"failures={len(failed)}", flush=True)
    shutil.rmtree(MELD / "experimental" / "_work", ignore_errors=True)
    print(f"done in {(time.time()-t0)/60:.1f} min. ok={len(cells)-len(failed)} failed={len(failed)}")
    if failed:
        print("failed cells (re-run them):", json.dumps(failed))


def _surface_water_fraction(mca: Path) -> float:
    import zlib
    import gzip
    import struct
    from io import BytesIO
    import nbtlib
    data = mca.read_bytes()
    if len(data) < 8192:
        return 0.0
    surfw = surftot = 0
    for i in range(1024):
        off, = struct.unpack(">I", b"\x00" + data[i * 4:i * 4 + 3])
        if off == 0:
            continue
        st = off * 4096
        ln, = struct.unpack(">I", data[st:st + 4])
        ct = data[st + 4]
        pl = data[st + 5:st + 4 + ln]
        try:
            raw = zlib.decompress(pl) if ct == 2 else gzip.decompress(pl) if ct == 1 else pl
            root = nbtlib.File.parse(BytesIO(raw))
        except Exception:
            continue
        sw = [[None] * 16 for _ in range(16)]
        for sec in sorted((dict(x) for x in (dict(root).get("sections") or [])),
                          key=lambda s: int(s.get("Y", 0)), reverse=True):
            bs = sec.get("block_states")
            if bs is None:
                continue
            pal = [str(p.get("Name", "")) for p in bs.get("palette", [])]
            da = bs.get("data")
            if da is None:
                if pal[0] in ("minecraft:air", "minecraft:cave_air"):
                    continue
                for oz in range(16):
                    for ox in range(16):
                        if sw[oz][ox] is None:
                            sw[oz][ox] = (pal[0] == "minecraft:water")
                continue
            bits = max(4, (len(pal) - 1).bit_length())
            per = 64 // bits
            mb = (1 << bits) - 1
            idx = []
            for L in (int(x) & 0xFFFFFFFFFFFFFFFF for x in da):
                for j in range(per):
                    idx.append((L >> (j * bits)) & mb)
                    if len(idx) >= 4096:
                        break
            for p in range(4095, -1, -1):
                zz = (p >> 4) & 15
                xx = p & 15
                if sw[zz][xx] is not None:
                    continue
                nm = pal[idx[p]] if idx[p] < len(pal) else "?"
                if nm in ("minecraft:air", "minecraft:cave_air"):
                    continue
                sw[zz][xx] = (nm == "minecraft:water")
        for oz in range(16):
            for ox in range(16):
                if sw[oz][ox] is not None:
                    surftot += 1
                    surfw += 1 if sw[oz][ox] else 0
    return (surfw / surftot * 100) if surftot else 0.0


def scan_water(world: Path, sample: int):
    rdir = world / "region"
    files = sorted(n for n in os.listdir(rdir) if re.match(r"r\.-?\d+\.-?\d+\.mca$", n))
    if not files:
        sys.exit(f"no region files in {rdir}")
    picks = [files[int(i * len(files) / sample)] for i in range(min(sample, len(files)))]
    print(f"surface water % across {len(picks)} sampled regions (a wedge is a region far over ~35%):")
    high = []
    for n in picks:
        f = _surface_water_fraction(rdir / n)
        flag = "  <- check (sea/delta, or residual wedge)" if f > 35 else ""
        print(f"  {n:22} {f:5.1f}%{flag}")
        if f > 35:
            high.append(n)
    print(f"\nregions over 35% water: {len(high)} (high values at the map edge are usually real sea or delta)")


def main():
    ap = argparse.ArgumentParser(description="Meld headless tools (experimental).")
    ap.add_argument("action", choices=["repair-gaps", "rerun-all", "scan-water"])
    ap.add_argument("--project", required=True, help="project slug under projects/ (for example meld-world-8-2)")
    ap.add_argument("--workers", type=int, default=12, help="parallel cells for repair-gaps / rerun-all")
    ap.add_argument("--sample", type=int, default=24, help="regions to sample for scan-water")
    args = ap.parse_args()

    P, s, origin, elev, world, grid = load_project(args.project)
    name = _safe_world_name(P.get("name", "Meld World"))
    print(f"project {args.project}  world {world}")

    if args.action == "scan-water":
        scan_water(world, args.sample)
        return
    if origin.get("lat") is None:
        sys.exit("project has no origin set.")
    if args.action == "repair-gaps":
        cells = _missing_cells(grid, world)
        print(f"merged cells missing from this world folder: {len(cells)}")
        run_cells(cells, s, origin, elev, world, name, args.workers)
    elif args.action == "rerun-all":
        cells = [ck for ck, st in grid.items() if st == "merged"]
        print(f"re-running {len(cells)} merged cells with the deployed arnis.exe")
        run_cells(cells, s, origin, elev, world, name, args.workers)


if __name__ == "__main__":
    main()
