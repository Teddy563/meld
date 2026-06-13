"""
Patch the LevelName (and optionally WorldBorder) of a Minecraft Java level.dat
in place. Ported from the experimental meld/level_dat.py — no full NBT parser
needed because the tags are found by their unique byte signature.

Java only. Bedrock level.dat is a different format and is not handled.
"""

import gzip
import struct
from pathlib import Path

_LEVEL_NAME_TAG = b"\x08\x00\x09LevelName"
_BORDER_CENTER_X_TAG = b"\x06\x00\x0DBorderCenterX"
_BORDER_CENTER_Z_TAG = b"\x06\x00\x0DBorderCenterZ"
_BORDER_SIZE_TAG = b"\x06\x00\x0ABorderSize"


def _find_level_name_field(blob: bytes):
    idx = blob.find(_LEVEL_NAME_TAG)
    if idx < 0:
        return None
    value_len_offset = idx + len(_LEVEL_NAME_TAG)
    if value_len_offset + 2 > len(blob):
        return None
    value_len = struct.unpack(">H", blob[value_len_offset:value_len_offset + 2])[0]
    value_start = value_len_offset + 2
    if value_start + value_len > len(blob):
        return None
    return value_start, value_len


def get_level_name(level_dat_path) -> str | None:
    p = Path(level_dat_path)
    if not p.exists():
        return None
    try:
        with gzip.open(p, "rb") as f:
            blob = f.read()
    except OSError:
        return None
    found = _find_level_name_field(blob)
    if not found:
        return None
    start, length = found
    try:
        return blob[start:start + length].decode("utf-8")
    except UnicodeDecodeError:
        return None


GOLD_CODE = "§6"   # Minecraft §6 = gold (#FFAA00)


def gold_name(name: str) -> str:
    """Prefix a world name with the Minecraft gold colour code (§6) so it renders
    gold in Minecraft's world-selection list. Idempotent; the on-disk FOLDER name
    stays plain (this only colours the in-game LevelName)."""
    name = name or "Meld World"
    return name if name.startswith(GOLD_CODE) else GOLD_CODE + name


def patch_level_name(level_dat_path, new_name: str) -> bool:
    p = Path(level_dat_path)
    if not p.exists():
        return False
    try:
        with gzip.open(p, "rb") as f:
            blob = f.read()
    except OSError:
        return False
    found = _find_level_name_field(blob)
    if not found:
        return False
    value_start, value_len = found
    # Truncate on a character boundary so we never split a multibyte UTF-8
    # sequence (the NBT string length prefix is uint16, max 0xFFFF bytes).
    while len(new_name.encode("utf-8")) > 0xFFFF:
        new_name = new_name[:-1]
    new_bytes = new_name.encode("utf-8")
    length_prefix_offset = value_start - 2
    patched = (
        blob[:length_prefix_offset]
        + struct.pack(">H", len(new_bytes))
        + new_bytes
        + blob[value_start + value_len:]
    )
    with gzip.open(p, "wb") as f:
        f.write(patched)
    return True


def _patch_double_field(blob: bytes, tag_marker: bytes, value: float):
    idx = blob.find(tag_marker)
    if idx < 0:
        return None
    value_offset = idx + len(tag_marker)
    if value_offset + 8 > len(blob):
        return None
    return blob[:value_offset] + struct.pack(">d", float(value)) + blob[value_offset + 8:]


def patch_world_border(level_dat_path, center_x: float, center_z: float, size: float) -> bool:
    p = Path(level_dat_path)
    if not p.exists():
        return False
    try:
        with gzip.open(p, "rb") as f:
            blob = f.read()
    except OSError:
        return False
    patched, any_patched = blob, False
    for tag, value in ((_BORDER_CENTER_X_TAG, center_x),
                       (_BORDER_CENTER_Z_TAG, center_z),
                       (_BORDER_SIZE_TAG, size)):
        cand = _patch_double_field(patched, tag, value)
        if cand is not None:
            patched, any_patched = cand, True
    if not any_patched:
        return False
    with gzip.open(p, "wb") as f:
        f.write(patched)
    return True
