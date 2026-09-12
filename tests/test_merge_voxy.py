"""
The Voxy LOD cache has to survive the merge, or asking for it does nothing.

Arnis writes `<world>/voxy/<world-id>/storage/...` into the CELL world. Meld's merge
copies region files, level.dat and datapacks into the master and then deletes the cell
folder, so without an explicit copy the cache is generated, thrown away, and the toggle
silently costs time and disk for nothing.

It is one database keyed on the world seed, not a set of per-region files, so it cannot
be merged cell by cell: it travels only when the master has none yet. That is the
single-cell render `server._runner` allows the flag for.

Run: python -m pytest tests -q
"""

import struct
import sys
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from src.merge import merge_cell_into_master


def _write_region(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = zlib.compress(b"\x00")
    header = bytearray(8192)
    header[0:4] = struct.pack(">I", (2 << 8) | 1)
    chunk = struct.pack(">IB", len(payload) + 1, 2) + payload
    chunk += b"\x00" * (4096 - (len(chunk) % 4096))
    path.write_bytes(bytes(header) + chunk)


def _cell_world(root: Path, cell_key: str, *, world_id: str) -> Path:
    """A cell world as arnis leaves it after `--voxy-lod`."""
    from src.coords import canonical_region_bounds

    world = root / f"cell-{cell_key.replace(',', '_')}"
    rx_min, rx_max, rz_min, rz_max = canonical_region_bounds(cell_key)
    for rx in range(rx_min, rx_max + 1):
        for rz in range(rz_min, rz_max + 1):
            _write_region(world / "region" / f"r.{rx}.{rz}.mca")
    (world / "level.dat").write_bytes(b"\x1f\x8b\x08\x00fake-level-dat")
    storage = world / "voxy" / world_id / "storage"
    storage.mkdir(parents=True)
    (storage / "000004.log").write_bytes(b"wal")
    (storage / "CURRENT").write_text("MANIFEST-000005\n", encoding="utf-8")
    (world / "voxy" / "config.json").write_text("{}", encoding="utf-8")
    return world


def test_voxy_cache_reaches_the_master_world(tmp_path):
    master = tmp_path / "master"
    cell = _cell_world(tmp_path, "0,0,1", world_id="abc123")

    res = merge_cell_into_master(str(cell), str(master), "0,0,1")

    assert res["voxy"] == "copied"
    assert (master / "voxy" / "abc123" / "storage" / "CURRENT").exists()
    assert (master / "voxy" / "config.json").exists()


def test_a_second_cell_does_not_overwrite_an_existing_cache(tmp_path):
    """One database per world: the second cell's is a different world-id built from its
    own chunks, and stacking them would leave the master holding two half-databases."""
    master = tmp_path / "master"
    merge_cell_into_master(str(_cell_world(tmp_path, "0,0,1", world_id="abc123")),
                           str(master), "0,0,1")
    second = _cell_world(tmp_path / "b", "1,0,1", world_id="def456")

    res = merge_cell_into_master(str(second), str(master), "1,0,1")

    assert res["voxy"] == "already present"
    assert not (master / "voxy" / "def456").exists()
    assert (master / "voxy" / "abc123" / "storage" / "CURRENT").exists()


def test_a_cell_without_a_cache_merges_unchanged(tmp_path):
    master = tmp_path / "master"
    cell = _cell_world(tmp_path, "0,0,1", world_id="abc123")
    import shutil

    shutil.rmtree(cell / "voxy")

    res = merge_cell_into_master(str(cell), str(master), "0,0,1")

    assert res["voxy"] == "skipped"
    assert not (master / "voxy").exists()
