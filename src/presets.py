"""presets.py — shareable settings presets: "my look, your place".

A preset is ONE json file a user can send to another user. The recipient applies it and
gets the sender's rendering choices — scale, terrain, trees, snow — on their own selection.
The file carries a settings blob plus enough metadata to show it in a list; it deliberately
carries nothing about the machine it came from.

That last part is the whole reason this module exists rather than "just export project.json".
The settings blob mixes two very different kinds of key: the look of the world, and the shape
of the computer that built it. A preset authored on a 24-core/64 GB desktop says max_workers
24, cpu_target_pct 95, master_world_dir D:\\big-disk — imported verbatim onto a laptop that
oversubscribes every core, and the path either errors or, worse, silently lands a world on a
drive that does not exist. So machine-specific keys are stripped in BOTH directions: on save
(they never leave the sender) and again on import/apply (a hand-edited or old file cannot
smuggle them back in).

File shape (schema 1):

    {"meld_preset": 1, "name": str, "description": str, "author": str,
     "meld_version": str, "created": "YYYY-MM-DD",
     "settings": {...}, "selection": {"bbox": {...}}}        # selection optional

Two sources, one shape:

    data_dir()/presets/      the user's own saves + imports. Writable.
    resource_dir()/presets/  shipped inside the app (see packaging/meld.spec). Read-only:
                             applying is fine, deleting/overwriting is refused, and "save as"
                             from a bundled one lands in the user dir.

Forward-compat rule: on import, any settings key this build does not know (not present in
default_settings) is dropped and REPORTED, not kept. Keeping it would round-trip garbage into
project.json where it sits forever; silently dropping it would make a newer preset "not work"
with no explanation. Dropped-with-a-note is the only variant a user can act on.
"""

from __future__ import annotations

import json
import re
from datetime import date
from pathlib import Path

from src import __version__ as MELD_VERSION
from src.paths import data_dir, resource_dir
from src.project import default_settings

PRESET_SCHEMA = 1

# A settings blob serialises to ~3 KB; the metadata adds a few hundred bytes. Anything over
# a megabyte is not a preset, it is a mistake (or a payload), and rejecting it before parsing
# keeps a hostile upload from ballooning the process.
MAX_PRESET_BYTES = 1 << 20

# ── the machine/world split ──────────────────────────────────────────────────────────────
# Built by reading default_settings() in src/project.py top to bottom and asking of each key:
# does this describe the WORLD, or the COMPUTER building it? Regenerate the same way after
# adding a setting — there is deliberately no import-time introspection here, because "is this
# machine-specific" is a judgement call (job_size_regions looks like a layout knob but is
# really a per-worker RAM budget; a 16-region cell that fits in 64 GB OOMs a laptop).
#
# Keys that never made it into default_settings still reach the blob through side doors
# (/api/mcserver/opts persists server_staging, /api/settings accepts rock_density...), so the
# exact-name set is backed by three RULES below: the server_ prefix, path-shaped suffixes
# (_dir/_path/_folder + save_location), and — on import only — anything unknown.
_MACHINE_KEYS = frozenset({
    # CPU/parallelism budgets, sized to this box's cores.
    "max_workers", "cpu_target_pct", "min_threads_per_worker",
    "cpu_stagger_seconds", "cpu_stagger_enabled", "cpu_stagger_adaptive",
    "osm_bake_workers", "export_compression_workers",
    "datapack_tile_concurrency", "prefetch_concurrency",
    # The governor: a scheduling policy for THIS box plus the numbers it measured on it.
    # governor_history is the worst offender — it carries another machine's cores-per-cell,
    # RSS p95 and cells-per-minute, which would warm-start a laptop straight at a 24-core
    # desktop's knee. worker_autoscale is its retired predecessor (same story, and it leaked
    # until now). ram_headroom_mb / flush_threads_cap / governor_max_workers are this box's
    # RAM and core budget, exactly like max_workers next to them.
    "governor_mode", "governor_history", "ram_headroom_mb",
    "flush_threads_cap", "governor_max_workers", "worker_autoscale",
    # GPU cave evaluation: names hardware the recipient may not have (dgpu/igpu), and its own
    # tooltip says it never travels — it just was not on this list. Falls back to CPU when no
    # adapter matches, so importing it was never fatal, only wrong.
    "gpu_accel",
    # RAM/disk shape: cell size is a per-worker memory budget, stream-to-disk is its escape
    # hatch, and the per-cell timeout is a statement about how fast this machine renders.
    "job_size_regions", "stream_to_disk", "timeout",
    # This machine's network/cache environment. overpass_url in particular can name someone's
    # PRIVATE endpoint — shipping it inside a shared file would hand that URL to strangers.
    "osm_cache_ttl_days", "overpass_url",
    # Observed per-region output size on THIS machine's renders; feeds the disk estimate.
    "mb_per_region_observed",
    # Phase-2 perf switches (src/project.py). Two are kill switches for work that is not written
    # yet and one turns on extra run-report timers: all three are decisions about how THIS box
    # renders and measures, and a shared preset must not flip a stranger's build onto an
    # experimental parser or an unbuilt region-write path.
    "canonical_regions", "osm_sidecars", "parse_fast_json", "phase2_timers",
    # A Mapillary API token. A preset is made to be shared - posted in a Discord thread,
    # mailed to someone rebuilding the same city - and a credential must not ride along in
    # it. The recipient supplies their own; the free tier costs them nothing.
    "mapillary_token",
    # Filesystem paths (also caught by the suffix rule; named here so the list reads whole).
    "master_world_dir",
})


def _machine_specific(key: str) -> bool:
    return (key in _MACHINE_KEYS
            or key.startswith("server_")                       # the whole Leaf-server profile
            or key.endswith(("_dir", "_path", "_folder"))      # any path, present or future
            or key == "save_location")


def clean_settings(settings: dict, *, known_only: bool) -> tuple[dict, list[str], list[str]]:
    """Split a settings blob into (kept, machine_stripped, unknown_dropped).

    known_only=False is the SAVE direction: strip the machine keys, keep everything else the
    sender's Meld understood. known_only=True is the IMPORT/APPLY direction: additionally
    drop any key absent from this build's default_settings — the forward-compat rule above,
    and the reason applying a preset can never poison project.json with foreign keys."""
    known = default_settings()
    kept: dict = {}
    machine: list[str] = []
    unknown: list[str] = []
    for k, v in (settings or {}).items():
        if not isinstance(k, str):
            continue
        if _machine_specific(k):
            machine.append(k)
        elif known_only and k not in known:
            unknown.append(k)
        else:
            kept[k] = v
    return kept, sorted(machine), sorted(unknown)


# ── the two sources ──────────────────────────────────────────────────────────────────────
def user_dir() -> Path:
    d = data_dir() / "presets"
    d.mkdir(parents=True, exist_ok=True)
    return d


def bundled_dir() -> Path:
    return resource_dir() / "presets"


def _fname(name: str) -> str:
    """Filename from a display name. The NAME in the json stays the identity; the filename
    only has to be safe on every filesystem a preset gets mailed across."""
    s = re.sub(r"[^a-z0-9]+", "-", (name or "").lower()).strip("-")
    return (s or "preset") + ".json"


# ── shape ────────────────────────────────────────────────────────────────────────────────
def validate(obj) -> str | None:
    """Error message, or None when the object is a readable schema-1 preset."""
    if not isinstance(obj, dict):
        return "not a preset: expected a JSON object"
    if obj.get("meld_preset") != PRESET_SCHEMA:
        return (f"unsupported preset schema {obj.get('meld_preset')!r} "
                f"(this Meld reads schema {PRESET_SCHEMA})")
    if not isinstance(obj.get("name"), str) or not obj["name"].strip():
        return "preset has no name"
    if not isinstance(obj.get("settings"), dict):
        return "preset has no settings object"
    return None


def normalize_selection(sel) -> dict | None:
    """A usable embedded selection, or None. Anything short of four finite bbox numbers is
    treated as 'no selection' rather than an error — a preset is first a look, the place is
    the optional extra, and a mangled extra must not block the look."""
    if not isinstance(sel, dict) or not isinstance(sel.get("bbox"), dict):
        return None
    try:
        bbox = {k: float(sel["bbox"][k]) for k in ("south", "west", "north", "east")}
    except (KeyError, TypeError, ValueError):
        return None
    out: dict = {"bbox": bbox}
    if isinstance(sel.get("polygons"), list) and sel["polygons"]:
        out["polygons"] = sel["polygons"]
    return out


def build(name: str, description: str = "", author: str = "",
          settings: dict | None = None, selection: dict | None = None) -> dict:
    """Assemble a schema-1 preset. `settings` should already be through clean_settings —
    build() does not re-strip, so the caller keeps the stripped-key list to report."""
    p = {
        "meld_preset": PRESET_SCHEMA,
        "name": (name or "").strip()[:80],
        "description": str(description or "").strip()[:500],
        "author": str(author or "").strip()[:80],
        "meld_version": MELD_VERSION,
        "created": date.today().isoformat(),
        "settings": dict(settings or {}),
    }
    sel = normalize_selection(selection)
    if sel:
        p["selection"] = sel
    return p


# ── listing / lookup ─────────────────────────────────────────────────────────────────────
def _read(path: Path) -> dict | None:
    """A preset from disk, or None. Unreadable files are skipped, not raised: one corrupt
    json in the user dir must not take the whole preset list down with it."""
    try:
        if path.stat().st_size > MAX_PRESET_BYTES:
            return None
        obj = json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return None
    return obj if validate(obj) is None else None


def _entry(obj: dict, path: Path, bundled: bool) -> dict:
    # A file can also DECLARE itself bundled: the shipped starters are seeded into the user
    # folder as editable copies carrying "bundled": true, so the tag survives the copy.
    return {"name": obj["name"], "description": obj.get("description", ""),
            "author": obj.get("author", ""), "created": obj.get("created", ""),
            "meld_version": obj.get("meld_version", ""),
            "bundled": bool(bundled or obj.get("bundled")),
            "file": path.name,
            "has_selection": normalize_selection(obj.get("selection")) is not None}


def seed_bundled() -> list[str]:
    """Copy each shipped starter into the user presets folder, once, as an EDITABLE file.

    The starters used to live only inside the app bundle, listed read-only - which made them
    impossible to tune. The owner's workflow is the whole reason this exists: open the JSON in
    the presets folder, adjust it, test, and eventually feed the tuned values back into the
    repo. So the resource copy is a SEED, not the source of truth: copied only when the user
    folder has no preset of that name, stamped "bundled": true so the tag survives, and never
    overwritten after that - a reseed that clobbered edits would defeat the point. Deleting a
    seeded file resets it to pristine on the next start, which doubles as reset-to-default.
    """
    seeded: list[str] = []
    src = bundled_dir()
    if not src.is_dir():
        return seeded
    have = {e["name"].strip().lower() for e in _dir_entries(user_dir(), False)}
    for p in sorted(src.glob("*.json")):
        obj = _read(p)
        if not obj or obj["name"].strip().lower() in have:
            continue
        obj["bundled"] = True
        (user_dir() / p.name).write_text(json.dumps(obj, indent=2), encoding="utf-8")
        seeded.append(obj["name"])
    return seeded


def _dir_entries(d: Path, bundled: bool) -> list[dict]:
    out = []
    if d.is_dir():
        for p in sorted(d.glob("*.json")):
            obj = _read(p)
            if obj:
                out.append(_entry(obj, p, bundled))
    return out


def _sources() -> list[tuple[Path, bool]]:
    # User presets FIRST — both for the list order the UI shows and so a user file wins a
    # name lookup. (Bundled names are refused at save/import, so a real shadow cannot
    # happen; the order still decides who wins if someone drops a file in by hand.)
    return [(user_dir(), False), (bundled_dir(), True)]


def list_presets() -> list[dict]:
    # Deduped by name, user copy first: after seeding, every shipped starter exists in BOTH
    # directories, and listing both would show each twice - with the read-only resource copy
    # shadowing the edits the seeding exists to allow.
    out: list[dict] = []
    seen: set[str] = set()
    for d, bundled in _sources():
        for e in _dir_entries(d, bundled):
            k = e["name"].strip().lower()
            if k in seen:
                continue
            seen.add(k)
            out.append(e)
    return out


def find(name: str):
    """(preset, path, bundled) for a display name (case-insensitive), or None."""
    want = (name or "").strip().lower()
    if not want:
        return None
    for d, bundled in _sources():
        if not d.is_dir():
            continue
        for p in sorted(d.glob("*.json")):
            obj = _read(p)
            if obj and obj["name"].strip().lower() == want:
                return obj, p, bundled
    return None


def bundled_names() -> set[str]:
    """Lower-cased names of the shipped presets — the reserved namespace."""
    d = bundled_dir()
    out: set[str] = set()
    if d.is_dir():
        for p in d.glob("*.json"):
            obj = _read(p)
            if obj:
                out.add(obj["name"].strip().lower())
    return out


def unique_name(name: str) -> str:
    """First of 'Name', 'Name (2)', ... not already a USER preset. Imports auto-suffix
    instead of overwriting — the same never-clobber rule project creation uses — because an
    import must not be able to destroy the preset the user made themselves."""
    base = (name or "").strip()
    taken = {e["name"].strip().lower() for e in list_presets() if not e["bundled"]}
    n, i = base, 2
    while n.strip().lower() in taken:
        n = f"{base} ({i})"
        i += 1
    return n


def save_user(preset: dict) -> Path:
    path = user_dir() / _fname(preset["name"])
    path.write_text(json.dumps(preset, indent=2), encoding="utf-8")
    return path
