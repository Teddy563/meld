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

import inspect
import math
import os
import re
import subprocess
import threading
import time
from pathlib import Path

from .coords import recommended_elev_zoom, ELEV_ZOOM_MIN, ELEV_ZOOM_MAX
from .osm_grid import OSM_GRID_Z

# Cave biome names the fork's --cave-biomes accepts, in display order.
CAVE_BIOMES = ["lush", "dripstone", "deepdark", "mushroom", "ice", "amethyst",
               "volcanic", "coral"]


def cave_biomes_spec(settings: dict) -> str:
    """`--cave-biomes` value from the cave_biome_amounts setting, or '' when every
    biome sits at its default 100 (so default runs stay byte-identical)."""
    amounts = settings.get("cave_biome_amounts") or {}
    parts = []
    for name in CAVE_BIOMES:
        try:
            pct = int(amounts.get(name, 100))
        except (TypeError, ValueError):
            pct = 100
        pct = max(0, min(200, pct))
        if pct != 100:
            parts.append(f"{name}={pct}")
    return ",".join(parts)


# Farmland texture categories the fork's --field-mix accepts, in order.
FIELD_MIX_KEYS = ["coarse", "plains", "flower", "farm", "moss"]


def _mix_spec(mix: dict) -> str:
    """Render a 5-share mix dict as the fork's `name=pct` list ('' when farm-only)."""
    parts = {}
    for name in FIELD_MIX_KEYS:
        try:
            pct = int((mix or {}).get(name, 0))
        except (TypeError, ValueError):
            pct = 0
        parts[name] = max(0, min(200, pct))
    if parts["coarse"] + parts["plains"] + parts["flower"] + parts["moss"] == 0:
        return ""
    return ",".join(f"{k}={v}" for k, v in parts.items() if v > 0)


def field_mix_spec(settings: dict) -> str:
    """`--field-mix` value, or '' when stock (farm-only) so default runs stay
    byte-identical (no flag at all)."""
    return _mix_spec(settings.get("field_mix"))


# Farm-plot crop keys + default shares (the "combined" patchwork; must match the fork).
FARM_CROPS = [("wheat", 40), ("potato", 15), ("carrot", 15), ("beetroot", 8),
              ("sunflower", 12), ("pumpkin", 5), ("fallow", 5)]


def farm_crops_spec(settings: dict) -> str:
    """`--farm-crops` value from the farm_crops setting, or '' when every crop sits at
    its default share (so default runs need no flag)."""
    crops = settings.get("farm_crops") or {}
    vals = {}
    changed = False
    for name, default in FARM_CROPS:
        try:
            pct = int(crops.get(name, default))
        except (TypeError, ValueError):
            pct = default
        pct = max(0, min(200, pct))
        vals[name] = pct
        if pct != default:
            changed = True
    if not changed:
        return ""
    return ",".join(f"{k}={v}" for k, v in vals.items() if v > 0)


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


_HELP_CACHE: dict[str, str] = {}


def arnis_supports(arnis_exe: str, flag: str) -> bool:
    """Does this generator advertise `flag` in --help? Cached per exe path.

    Meld and the fork ship and update separately: a user can run a new Meld against the 3.0.7
    binary already sitting next to it, and clap rejects an unknown argument outright, so an
    ungated new flag turns every cell into "error: unexpected argument '--overture'". Asking the
    binary what it accepts is version-independent - it tests the capability rather than inferring
    it from a version string that a locally built or side-loaded binary may not report honestly.

    server.py had this same probe for --stream-to-disk and it fell out of use; it lives here now,
    beside the code that emits the flags, so the next new flag has one obvious place to gate on.
    """
    if not arnis_exe:
        return False
    key = str(arnis_exe)
    if key not in _HELP_CACHE:
        try:
            r = subprocess.run([key, "--help"], capture_output=True, text=True, timeout=20,
                               encoding="utf-8", errors="replace")
            _HELP_CACHE[key] = (r.stdout or "") + (r.stderr or "")
        except Exception:
            # Unreadable help means "assume old": passing a flag it may not have is the failure
            # mode that kills a whole run, while omitting one only loses the new behaviour.
            _HELP_CACHE[key] = ""
    return flag in _HELP_CACHE[key]


# NOTE: there is deliberately NO capability probe for the phase-marker protocol.
# ARNIS_PHASE_MARKERS is an ENV VAR, not a CLI flag: clap prints neither it nor a
# `--phase-markers` in --help, so any --help grep for it answers False forever and the
# env var never gets set. A binary that predates the protocol simply IGNORES an unknown
# env var and prints no marker lines, which run_arnis() already handles by falling back
# to the psutil sampler ("source": "sampler"). So the caller sets the var unconditionally
# and lets the presence of a `phase=done` line be the answer.
_VER_CACHE: dict[str, tuple] = {}


def forget_probe(arnis_exe: str) -> None:
    """Drop the cached --help and --version answers for one binary path.

    Both caches key on the PATH, and updating the generator replaces the file at a path that
    stays the same - so without this the process keeps answering from the binary it probed
    before. That is not just a stale version string in a message: arnis_supports() gates every
    new flag, so a freshly installed 3.0.8 would still be told it has no --overture, and the
    checkbox that update just enabled would go on doing nothing until Meld restarted.
    """
    key = str(arnis_exe)
    _VER_CACHE.pop(key, None)
    _HELP_CACHE.pop(key, None)


def arnis_version(arnis_exe: str) -> tuple[int, ...]:
    """(3, 0, 8) for this generator, or () if it will not say.

    `arnis --version` prints an ASCII banner and then a plain `arnis 3.0.8` line, so the last
    match wins rather than the first - the banner contains a version too, and picking that one
    would work today and break the moment the banner changes.

    Used to decide whether a downloaded generator is newer than the bundled one. Numeric per
    component, because "3.0.10" sorts before "3.0.9" as text.
    """
    if not arnis_exe:
        return ()
    key = str(arnis_exe)
    if key not in _VER_CACHE:
        ver: tuple[int, ...] = ()
        try:
            r = subprocess.run([key, "--version"], capture_output=True, text=True, timeout=20,
                               encoding="utf-8", errors="replace")
            found = re.findall(r"arnis\s+v?(\d+(?:\.\d+)*)", (r.stdout or "") + (r.stderr or ""),
                               re.IGNORECASE)
            if found:
                ver = tuple(int(p) for p in found[-1].split("."))
        except Exception:
            ver = ()
        _VER_CACHE[key] = ver
    return _VER_CACHE[key]


# Scale envelope the generator accepts. Mirrors MIN_SCALE/MAX_SCALE in the fork's
# src/args.rs, which since 3.1.0 rejects anything outside it at the clap parser - an
# out-of-range value used to reach the fetch stage and produce a hung or empty cell, and
# now fails the cell outright. Meld clamps instead of failing: these come from a settings
# POST or a preset, where the useful behaviour is to pull the value back into range and
# carry on, the same way job_size_regions and cpu_target_pct are handled.
#
# The floor is 0.01 (1:100) because Meld's planet renders live down there; upstream arnis
# floors at 0.05, which is why the fork deliberately widened it.
MIN_SCALE = 0.01
MAX_SCALE = 4.0


def clamp_scale(value) -> float:
    """Pull a scale into the generator's accepted range. Non-numeric falls back to 1.0."""
    try:
        scale = float(value)
    except (TypeError, ValueError):
        return 1.0
    # Every non-finite value falls back rather than clamping. NaN has to be special-cased
    # anyway (it fails both comparisons, so min/max would pass it straight through), and
    # clamping the infinities is worse than useless: +inf would land on MAX_SCALE and
    # quietly start a 4:1 render of whatever the selection is.
    if not math.isfinite(scale):
        return 1.0
    return max(MIN_SCALE, min(MAX_SCALE, scale))


def build_arnis_cmd(arnis_exe: str, bbox: dict, output_path: str,
                    settings: dict, origin: dict, elevation: dict | None,
                    seed: int, osm_file: str | None = None,
                    loot_table: str | None = None,
                    cell_key: str | None = None) -> list[str]:
    s, w, n, e = bbox["south"], bbox["west"], bbox["north"], bbox["east"]
    # Last line of defence. The settings API clamps on write, so a stored scale is already
    # in range; this catches a preset or project file written by an older Meld.
    scale = clamp_scale(settings.get("scale", 1.0) or 1.0)
    cmd = [
        str(arnis_exe),
        "--bbox", f"{s},{w},{n},{e}",
        "--output-dir", str(output_path),
        "--scale", str(scale),
        f"--ground-level={int(settings.get('ground_level', -62))}",
        "--rotation", str(settings.get("rotation", 0)),
    ]

    # EXPERIMENTAL native B_Linear: the fork writes Leaf's container directly instead of
    # Anvil, so no conversion pass runs afterwards. Server-only output (Leaf 1.21.11+/26.x).
    if str(settings.get("native_region_format", "mca")).lower() == "blinear":
        cmd += ["--region-format", "blinear",
                "--blinear-level", str(int(settings.get("native_blinear_level", 6)))]

    # EXPERIMENTAL GPU cave density. Host-specific (never travels in world metadata):
    # the same project on another machine may have no such adapter.
    gpu = str(settings.get("gpu_accel", "off") or "off").lower()
    if gpu in ("auto", "dgpu", "igpu"):
        cmd += ["--gpu", gpu]

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
    # Custom chest loot table (project loot_table.json). Absent = built-in default.
    # Chests only spawn where Buildings AND Interior are both on.
    if loot_table:
        cmd += ["--loot-table", str(loot_table)]
    # Skip OSM buildings (keeps roads, bridges, railways, land cover, water, terrain).
    if not settings.get("buildings", True):
        cmd.append("--no-buildings")
    # Additional Buildings: Overture Maps footprints for buildings missing from OSM. Detected
    # from satellite imagery, so a few can land where nothing exists - which is the whole reason
    # this is separable from `buildings`. Only emitted when switched OFF and only when the
    # generator has the flag: it landed in the fork after 3.0.7, and older binaries reject it.
    if not settings.get("overture", True) and arnis_supports(arnis_exe, "--overture"):
        cmd.append("--overture=false")
    # Image signage (street-name plates, transit signs, billboards) is drawn as map items in
    # item frames, and the map payloads live in the world's data/ directory. merge.py copies
    # region/, poi/, entities/, datapacks/ and level.dat - never data/ - so every cell's maps
    # are discarded at merge and the frames in the master world would point at map ids that
    # do not exist. Upstream's default is `basic`, i.e. ON, so this is emitted explicitly and
    # unconditionally rather than relying on the generator's default. The fork does not carry
    # the flag today, which is what arnis_supports() is for; the guard exists so pointing Meld
    # at a stock upstream 3.1.0 binary does not quietly fill a merged world with blank frames.
    if arnis_supports(arnis_exe, "--signage"):
        cmd += ["--signage", str(settings.get("signage", "none"))]
    # Schematic props (boats, cranes, tractors, wind turbines) are fixed-size builds, so the fork
    # skips them below --props-min-scale (default 0.35) - at 1:10 a parked crane is the size of a
    # district. Meld's own default scale is 0.1, so the default silently dropped every family
    # while the UI showed the checkboxes ticked. Passing it explicitly makes the gate the user's
    # decision instead of an invisible one.
    props_min = settings.get("props_min_scale")
    if props_min not in (None, "") and arnis_supports(arnis_exe, "--props-min-scale"):
        cmd += ["--props-min-scale", str(float(props_min))]
    if settings.get("fill_ground"):
        cmd.append("--fillground")
    if settings.get("caves"):
        # Vanilla-noise cave worldgen in the arnis fork; --caves auto-enables --fillground.
        # Themed biomes + formations come from the cave-pack/ directory that ships NEXT TO
        # arnis.exe (auto-discovered; no CLI flag). Without it caves still generate, un-themed.
        cmd.append("--caves")
        spec = cave_biomes_spec(settings)
        if spec:
            cmd += ["--cave-biomes", spec]
    # Vertical geometry. The fork derives the world's height from the terrain and refuses
    # a target version it has no VERIFIED constants for, so a bad mc_version fails the run
    # loudly instead of producing a world that loads and then quietly misbehaves.
    if settings.get("disable_height_limit"):
        cmd.append("--disable-height-limit")
        # Room reserved above the peak / below the floor when fitting. Only meaningful
        # with extended height, so they ride along with the flag.
        for key, flag in (("height_headroom", "--height-headroom"),
                          ("height_underroom", "--height-underroom")):
            val = settings.get(key)
            if val is not None:
                cmd += [flag, str(int(val))]
    if settings.get("mc_version"):
        cmd += ["--mc-version", str(settings["mc_version"]).strip()]
    # SEAM-CRITICAL, always sent. The fork otherwise MEASURES how much room to reserve
    # under the terrain for the deepest water carve from each cell's own land cover, so a
    # coastal cell puts its datum ~5 blocks above an inland neighbour and the shared
    # border becomes a Y-cliff (measured: inland Y-62 vs coast Y-57). 'max' reserves the
    # engine's bounded worst case in EVERY cell, so they all agree.
    cmd += ["--water-carve-clearance", "max"]
    # Explicit world floor/ceiling. Blank = derived from the terrain + headroom/underroom.
    # The fork refuses values that would cut into the terrain rather than clamping them.
    for key, flag in (("world_min_y", "--min-y"), ("world_max_y", "--max-y")):
        val = settings.get(key)
        if val not in (None, ""):
            cmd += [flag, str(int(val))]
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

    # B1: name the region rectangle this cell owns, so arnis never writes the seam halo
    # ring that the merge deletes moments later - measured at 20 of 36 files for a 4x4
    # cell, 12.3% of the cell's CPU. Placement is unaffected (the halo is still generated,
    # and the neighbouring cell renders that ground itself); only the write is skipped,
    # which is why the block_hash is identical with and without the flag.
    #
    # Gated on the setting AND on having a cell key: a bbox render with no owning cell has
    # no neighbour to generate the adjacent ground, so suppressing its edge would lose
    # real terrain. Requires an arnis that knows the flag - an older binary rejects an
    # unknown argument outright, hence the version gate at the call site.
    if settings.get("canonical_regions") and cell_key:
        from src.coords import canonical_region_bounds
        rect = canonical_region_bounds(cell_key)
        if rect:
            # --flag=VALUE, not --flag VALUE: a rectangle west or north of the origin starts
            # with a minus, and clap reads a bare "-4,-1,0,3" as an unknown flag. That failed
            # 36 of 81 cells on the first real run - exactly the cells with a negative rx.
            cmd.append("--canonical-regions=" + ",".join(str(v) for v in rect))

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
    # Regional-only elevation: the inverse — NEVER touch AWS terrarium data (it has
    # broken no-data tiles in some regions, e.g. z14/z15 gaps that carve terrain).
    # The fork errors instead of silently falling back, and the terrain warm-up
    # pre-fills the regional provider's tile cache so parallel cells read disk.
    elif settings.get("terrain", True) and settings.get("regional_elevation_only"):
        cmd.append("--regional-elevation-only")
    # Cached-elevation-only. Bake the region first, then this guarantees the render uses the
    # bake and nothing else: no cell waits on the tile server, none is rate-limited, and none
    # receives a truncated tile - which is the documented cause of flat terrain seams.
    #
    # It does NOT make a missing tile fail loudly, whatever the flag name suggests: both
    # providers return an error and arnis's existing fallback turns that into flat/NaN ground
    # (mapterhorn.rs:528 says so). The cell log does print "offline: ... not cached", so
    # _scan_cell_health picks it up and marks the cell suspect - that is what makes the
    # otherwise-silent case visible.
    if settings.get("terrain", True) and settings.get("offline_elevation") \
            and arnis_supports(arnis_exe, "--offline"):
        cmd.append("--offline")

    # Road detail — auto: compact below scale 0.7, clean at/above. max => omit.
    rd = (settings.get("road_detail_level") or "auto").strip().lower()
    if rd == "auto":
        rd = "compact" if scale < 0.7 else "clean"
    if rd in ("compact", "clean"):
        cmd += ["--road-detail", rd]

    # Generator quality passes. Both default OFF in arnis, so omitting them
    # reproduces 1.9.7 output exactly; both are deliberate output changes, so
    # they are opt-in per project rather than machine-tuned like the governor.
    if settings.get("river_bed_v1") and arnis_supports(arnis_exe, "--river-bed"):
        cmd += ["--river-bed", "v1"]
    if settings.get("road_grade") and arnis_supports(arnis_exe, "--road-grade"):
        cmd += ["--road-grade", "on"]

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

    # Bundled schematic props at OSM features. --props takes an allow-list; omit it
    # when every family is on (byte-identical default), else pass the enabled set
    # ("none" if all off). A family missing from the settings dict counts as on, so
    # families not yet exposed in the UI stay enabled.
    PROP_FAMILIES = ("boat", "car", "crane", "excavator", "fountain", "helicopter",
                     "lighthouse", "playground", "starship", "tombstone", "tractor",
                     "windturbine")
    pv = settings.get("props") or {}
    enabled = [f for f in PROP_FAMILIES if pv.get(f, True)]
    if not enabled:
        cmd += ["--props", "none"]
    elif len(enabled) != len(PROP_FAMILIES):
        cmd += ["--props", ",".join(enabled)]

    # World settings (game mode + initial time of day) written into level.dat.
    gm = str(settings.get("gamemode", "creative")).lower()
    if gm in ("survival", "creative", "spectator"):
        cmd += ["--gamemode", gm]
    try:
        wt = int(settings.get("world_time", 6000))
        cmd += ["--world-time", str(max(0, min(23999, wt)))]
    except (TypeError, ValueError):
        pass

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

        # Size-tier popularity sliders (relative weights). Emits --tree-size-weights only when a
        # tier differs from its default (small/medium/big/tall=100, giant=0), so default runs stay
        # byte-identical. Falls back to the legacy tree_sizes checkbox dict if that's all that's
        # saved. Giant only renders at 1:1 and tiny maps never place tall/giant (gated in the fork).
        spec = tree_size_weights_spec(settings)
        if spec:
            cmd += ["--tree-size-weights", spec]

    # Farmland texturing (--field-mix) + scattered rocks/bushes. The mix flag is sent
    # only when a non-farm share is set; --rocks/--bushes only when toggled on. All
    # three default off -> a default project stays byte-identical.
    fm = field_mix_spec(settings)
    if fm:
        cmd += ["--field-mix", fm]
    fc = farm_crops_spec(settings)
    if fc:
        cmd += ["--farm-crops", fc]
    # Rock/bush scatter: one mode selector (chunk-rolled 20% in the fork). Legacy
    # projects that stored separate rocks/bushes booleans are migrated on the fly.
    mode = str(settings.get("scatter_mode") or "").strip().lower()
    if mode not in ("none", "rocks", "bushes", "both"):
        r = bool(settings.get("rocks")); b = bool(settings.get("bushes"))
        mode = "both" if (r and b) else "rocks" if r else "bushes" if b else "none"
    if mode in ("rocks", "both"):
        cmd.append("--rocks")
    if mode in ("bushes", "both"):
        cmd.append("--bushes")
    # Field-pattern zoom (only when non-default).
    try:
        fs = max(25, min(400, int(settings.get("field_scale", 100))))
    except (TypeError, ValueError):
        fs = 100
    if fs != 100:
        cmd += ["--field-scale", str(fs)]
    # Extend the field pattern beyond OSM farmland: OSM grassland, and untagged
    # satellite land (both ON by default — most of the plain is untagged cropland).
    if settings.get("grass_texture"):
        cmd.append("--grass-texture")
        gm = _mix_spec(settings.get("grass_mix"))
        if gm:
            cmd += ["--grass-mix", gm]
    if settings.get("land_texture"):
        cmd.append("--land-texture")
        # Untagged cropland's own mix (plains-leaning default); omitting the flag
        # would fall back to the farmland sliders in the fork.
        um = _mix_spec(settings.get("untagged_mix"))
        if um:
            cmd += ["--land-mix", um]
    cmd += upstream_3_2_flags(settings, arnis_exe)
    return cmd


# Options that arrived with upstream arnis 3.2.0 and do not exist in the 3.1.8 fork.
#
#   (settings key, flag, kind, generator default, allowed values)
#
# "bool"  emits the bare flag when the setting is truthy.
# "value" emits flag + value only when the value is an allowed one that is NOT the
#         generator's own default - so a project that leaves these alone produces the
#         exact command line Meld 1.9.8 produced, and the byte-identical-output
#         guarantee survives the version bump.
#
# --mapillary-token is deliberately absent: it is a credential, and argv is readable
# by any other process on the machine (ps, Task Manager, a crash dumper). It goes to
# the child's environment as MAPILLARY_TOKEN instead, which upstream reads and which
# clap is told to keep out of --help.
_UPSTREAM_3_2_OPTIONS: tuple[tuple[str, str, str, str | None, tuple[str, ...] | None], ...] = (
    ("voxy_lod", "--voxy-lod", "bool", None, None),
    ("building_facades", "--building-facades", "bool", None, None),
    ("body", "--body", "value", "earth", ("earth", "moon", "mars")),
    ("overture_source", "--overture-source", "value", "auto", ("auto", "tiles", "parquet")),
    ("facade_detail", "--facade-detail", "value", "standard", ("standard", "high")),
    ("facade_px", "--facade-px", "value", "16", ("4", "8", "16", "32")),
    ("mapillary_facade_mode", "--mapillary-facade-mode", "value",
     "photos", ("blocks", "photos")),
)


# Options Classic mode still allows: the ones the user asks for by name, one control at a
# time, and that emit nothing at all when left alone.
#
#   voxy_lod - writes a renderer's cache beside the world; no block in the world differs.
#   body     - Moon and Mars are not a style applied to a render, they ARE the render, and
#              the default (earth) emits nothing.
#
# What the switch is actually for is the other direction: features that change how an
# ORDINARY Earth render looks without being asked each time - upstream turns the facades on
# by itself as soon as a token exists, and the Overture transport changes where buildings
# come from. Those stay gated.
_ALWAYS_AVAILABLE_3_2: frozenset[str] = frozenset({"voxy_lod", "body"})


def upstream_3_2_flags(settings: dict, arnis_exe: str) -> list[str]:
    """The arnis 3.2.0 options this binary actually advertises.

    Every flag is gated on arnis_supports(), so one Meld build drives both the 3.1.8
    fork and a 3.2.0 one: against 3.1.8 each probe answers False, this returns [], and
    nothing about the command line changes. That is the whole compatibility mechanism -
    no version parsing, because a locally built or side-loaded binary does not report a
    version honestly enough to branch on.

    Classic mode short-circuits everything the switch is actually about: the facades and
    the Overture transport, which rewrite how the world looks and call outside services.
    Those are the reason a one-click way back to the old command line exists.

    `_ALWAYS_AVAILABLE_3_2` is the exception: options the user picks explicitly, one control
    at a time, which emit nothing when left at their default. The Classic guarantee still
    holds for them - Voxy off and Earth selected produce no flag at all - and gating them
    only hid deliberate choices behind a look-of-the-world switch.
    """
    enhanced = str(settings.get("gen_mode_32") or "classic").strip().lower() == "enhanced"

    out: list[str] = []
    for key, flag, kind, default, allowed in _UPSTREAM_3_2_OPTIONS:
        if not enhanced and key not in _ALWAYS_AVAILABLE_3_2:
            continue
        if not arnis_supports(arnis_exe, flag):
            continue
        raw = settings.get(key)
        if kind == "bool":
            if raw:
                out.append(flag)
            continue
        if raw in (None, ""):
            continue
        val = str(raw).strip().lower()
        if allowed and val not in allowed:
            continue
        if val != default:
            out += [flag, val]
    # Tri-state. Upstream turns the facades on by default as soon as a token exists, so
    # there is nothing to emit to enable them - only an explicit "I have a token and I
    # do not want to spend the download on this run" needs saying. None means "follow
    # the token", which is upstream's own behaviour and the setting's default.
    if (enhanced
            and settings.get("mapillary_facades") is False
            and arnis_supports(arnis_exe, "--mapillary-facades")):
        out += ["--mapillary-facades", "false"]
    return out


# Tree size tiers in display order + their default weight (giant off = 0, like the old checkbox).
TREE_SIZE_TIERS = (("small", 100), ("medium", 100), ("big", 100), ("tall", 100), ("giant", 0))


def tree_size_weights_spec(settings: dict) -> str:
    """`--tree-size-weights` value from tree_size_weights (name=pct pairs), emitting only tiers
    that differ from their default so default runs stay byte-identical. Migrates a legacy
    tree_sizes checkbox dict (True->100 / False->0) when tree_size_weights is absent."""
    weights = settings.get("tree_size_weights")
    if not isinstance(weights, dict):
        legacy = settings.get("tree_sizes")
        weights = {}
        if isinstance(legacy, dict):
            for name, _ in TREE_SIZE_TIERS:
                weights[name] = 100 if legacy.get(name) else 0
        elif isinstance(legacy, (list, tuple)):
            low = {str(t).strip().lower() for t in legacy}
            for name, _ in TREE_SIZE_TIERS:
                weights[name] = 100 if name in low else 0
    parts = []
    for name, default in TREE_SIZE_TIERS:
        try:
            pct = int(weights.get(name, default))
        except (TypeError, ValueError):
            pct = default
        pct = max(0, min(200, pct))
        if pct != default:
            parts.append(f"{name}={pct}")
    return ",".join(parts)


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


# --- Arnis stdout protocol v1 -------------------------------------------------------
# The generator prints, ONLY when ARNIS_PHASE_MARKERS=1 is in its environment:
#
#   [meld] v=1 phase=<name> t=<ms_since_process_start>
#   [meld] v=1 phase=done wall_s=<f.3> cpu_s=<f.3> peak_mb=<f.1> gpu_ms=<u64>
#
# phase names: fetch parse overture elevation ground place post merge save (whichever the
# real pipeline has), then exactly one `done` line. These are MACHINE lines: run_arnis
# consumes them, so they never reach on_line() and can never disturb the keyword scraping
# in parse_progress() or the user-visible cell log.
_MARKER_RE = re.compile(r"^\s*\[meld\]\s+v=(\d+)(?:\s+(.*))?$")
_DONE_FLOAT_KEYS = ("wall_s", "cpu_s", "peak_mb")


def is_marker_line(line: str) -> bool:
    """Is this a `[meld] v=N ...` protocol line (any version)?

    Version-agnostic on purpose: a future v=2 generator paired with today's Meld must still
    have its machine lines swallowed rather than dumped into the user's log. Meld's own
    diagnostics ("[meld] arnis exited with code ...") carry no `v=` and are unaffected.
    """
    return _MARKER_RE.match(line or "") is not None


def parse_phase_marker(line: str) -> dict | None:
    """Parse one protocol v1 line into a dict, or None if it is not one we understand.

    Returns {"phase": name, "t_ms": int} for a phase line and
    {"phase": "done", "wall_s": float, "cpu_s": float, "peak_mb": float, "gpu_ms": int}
    for the terminal line - keys whose values are missing or unparseable are simply absent,
    so a generator that cannot measure (say) peak RSS just omits it and the caller falls
    back. Unknown extra key=value tokens are ignored, which is what lets the protocol grow
    without a version bump.
    """
    m = _MARKER_RE.match(line or "")
    if not m or m.group(1) != "1":
        return None
    fields: dict[str, str] = {}
    for tok in (m.group(2) or "").split():
        if "=" in tok:
            k, _, v = tok.partition("=")
            if k:
                fields[k] = v
    phase = fields.get("phase")
    if not phase:
        return None
    out: dict = {"phase": phase}
    if phase == "done":
        for key in _DONE_FLOAT_KEYS:
            if key in fields:
                try:
                    val = float(fields[key])
                except ValueError:
                    continue
                if math.isfinite(val):
                    out[key] = val
        if "gpu_ms" in fields:
            try:
                out["gpu_ms"] = int(float(fields["gpu_ms"]))
            except ValueError:
                pass
        return out
    try:
        out["t_ms"] = int(float(fields.get("t", "")))
    except ValueError:
        out["t_ms"] = 0
    return out


def _emit_stats(on_stats, cpu_seconds: float, wall_seconds: float, extras: dict) -> None:
    """Call on_stats(cpu, wall) with as much of `extras` as it will accept.

    server.py's current callback is `def on_stats(cpu_seconds, wall_seconds)`. Passing it
    peak_rss_mb= would be a TypeError, and catching that TypeError is not safe (the callback
    body raises TypeErrors of its own, and a retry would run half of it twice), so the
    signature is inspected instead: **kwargs takes everything, otherwise only the extra
    parameters the callback actually names are passed. Old callers keep working untouched.
    """
    kwargs = {}
    try:
        params = inspect.signature(on_stats).parameters
        if any(p.kind is inspect.Parameter.VAR_KEYWORD for p in params.values()):
            kwargs = dict(extras)
        else:
            named = {n for n, p in params.items()
                     if p.kind in (inspect.Parameter.POSITIONAL_OR_KEYWORD,
                                   inspect.Parameter.KEYWORD_ONLY)}
            kwargs = {k: v for k, v in extras.items() if k in named}
    except (TypeError, ValueError):  # builtins / C callables have no signature
        kwargs = {}
    on_stats(cpu_seconds, wall_seconds, **kwargs)


def run_arnis(cmd: list[str], cwd: str, on_line=None, on_proc=None,
              env: dict | None = None, on_stats=None, on_phase=None) -> bool:
    """Run Arnis, streaming stdout line-by-line to on_line(text). Returns ok.

    on_proc(proc) is called once with the Popen handle so the caller can publish
    it (e.g. to worker state) for termination via /api/stop. It's cleared with
    on_proc(None) before returning.

    env (optional) is overlaid on the inherited environment for THIS child only
    (used to pin RAYON_NUM_THREADS so N parallel cells don't oversubscribe cores,
    and ARNIS_STREAM_TO_DISK=1 for big cells). The post-merge Arnis reads both;
    an older binary harmlessly ignores them.

    on_phase(name, t_ms) (optional) fires for every `[meld] v=1 phase=<name> t=<ms>`
    line the generator prints under ARNIS_PHASE_MARKERS=1. Those lines are CONSUMED
    whether or not a callback is given: they are machine output, so they never reach
    on_line() and can never move parse_progress()'s keyword scraping.

    on_stats(cpu_seconds, wall_seconds, **extras) is called once at exit. `extras` is
    peak_rss_mb (float|None), source ("arnis" when the numbers came from the generator's
    own `phase=done` line, "sampler" when they came from the psutil poller) and gpu_ms
    (int, only when the generator reported it). A callback that names none of them - like
    server.py's two-argument one - is still called with two arguments, so nothing breaks."""
    child_env = {**os.environ, **(env or {})}
    # encoding is PINNED to UTF-8: Arnis writes UTF-8 (the startup banner alone is
    # solid-block characters), while `text=True` with no encoding decodes using the
    # machine's locale code page. On a Central-European Windows (cp1250) that page has
    # no mapping for 0x88 — the third byte of "█" — so the very first read raised
    # UnicodeDecodeError, the process was killed, and every cell "failed" in 0s on
    # locales outside cp1252. errors="replace" keeps any future odd byte from ever
    # taking a cell down again.
    proc = subprocess.Popen(
        cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        text=True, encoding="utf-8", errors="replace",
        bufsize=1, cwd=str(cwd), env=child_env,
    )
    # Sample the child's CPU time so the worker governor can size the pool from what a
    # cell ACTUALLY uses rather than from the threads it was handed. A thread is needed
    # because cpu_times() has to be read while the process is alive, and this function
    # is busy blocking on stdout. Entirely best-effort: if psutil is missing or the
    # process exits between calls, no sample is recorded and nothing else changes.
    #
    # KNOWN UNDERCOUNT (unchanged, and now measurable): the poll runs every 0.5 s, so
    # whatever the child burns after its last successful read - up to ~0.5 s of CPU, plus
    # any late memory spike - is missed. That is why the generator's own `phase=done` line
    # is preferred when it is there: it is measured inside the process at exit. The sampler
    # is the fallback for old binaries and for runs with ARNIS_PHASE_MARKERS unset.
    _cpu = {"seconds": 0.0}
    _rss = {"peak_mb": 0.0}
    _stats_stop = threading.Event()
    _stats_thread = None
    if on_stats is not None:
        def _sample_cpu() -> None:
            try:
                import psutil
                p = psutil.Process(proc.pid)
                while not _stats_stop.wait(0.5):
                    try:
                        t = p.cpu_times()
                        _cpu["seconds"] = float(t.user + t.system)
                        mi = p.memory_info()
                        # peak_wset is Windows' own high-water mark, so it survives the
                        # gaps between polls; elsewhere the running max of rss is the
                        # best available and undercounts a spike between two reads.
                        mb = float(getattr(mi, "peak_wset", 0) or mi.rss) / (1024 * 1024)
                        if mb > _rss["peak_mb"]:
                            _rss["peak_mb"] = mb
                    except Exception:  # noqa: BLE001 - process gone; keep the last read
                        return
            except Exception:  # noqa: BLE001 - psutil unavailable
                return
        _stats_thread = threading.Thread(target=_sample_cpu, daemon=True,
                                         name="arnis-cpu-sample")
        _stats_thread.start()
    _started = time.time()
    if on_proc:
        on_proc(proc)
    try:
        lines = 0
        done: dict = {}
        for raw in proc.stdout:                       # type: ignore[union-attr]
            lines += 1
            text = raw.rstrip()
            if is_marker_line(text):
                # Machine line: consumed here, never forwarded to on_line().
                marker = parse_phase_marker(text)
                if marker is not None:
                    if marker["phase"] == "done":
                        done = marker
                    elif on_phase is not None:
                        try:
                            on_phase(marker["phase"], marker["t_ms"])
                        except Exception:  # noqa: BLE001 - telemetry never fails a cell
                            pass
                continue
            if on_line:
                on_line(text)
        proc.wait()
        _stats_stop.set()
        if on_stats is not None:
            if _stats_thread is not None:
                _stats_thread.join(timeout=1.0)
            try:
                # The generator's own numbers win when it reported both of them; the
                # sampler's are a poll away from the truth (see the note above).
                if "cpu_s" in done and "wall_s" in done:
                    cpu_s, wall_s, source = done["cpu_s"], done["wall_s"], "arnis"
                else:
                    cpu_s = _cpu["seconds"]
                    wall_s = max(0.0, time.time() - _started)
                    source = "sampler"
                peak_mb = done.get("peak_mb")
                if peak_mb is None:
                    peak_mb = _rss["peak_mb"] or None
                extras = {"peak_rss_mb": peak_mb, "source": source}
                if "gpu_ms" in done:
                    extras["gpu_ms"] = done["gpu_ms"]
                _emit_stats(on_stats, cpu_s, wall_s, extras)
            except Exception:  # noqa: BLE001 - telemetry must never fail a cell
                pass
        if proc.returncode != 0 and on_line:
            # The exit code is the one fact every failure has. Without it a cell that clap
            # rejected, that panicked, and that was killed all looked identical downstream.
            on_line(f"[meld] arnis exited with code {proc.returncode} "
                    f"after {lines} line(s) of output")
        return proc.returncode == 0
    except Exception as e:                            # noqa: BLE001
        # Never swallow this silently: a reader-side failure looks exactly like an Arnis
        # failure from the outside, and with no line the log tail is guessed at instead
        # (that is how a decode crash got reported as a "network timeout").
        if on_line:
            try:
                on_line(f"[meld] arnis output could not be read: {type(e).__name__}: {e}")
            except Exception:
                pass
        try:
            proc.kill()
        except Exception:
            pass
        return False
    finally:
        if on_proc:
            on_proc(None)
