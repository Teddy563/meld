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
        # Image signage: none | basic | full. Held at "none" because the generator writes
        # signage map payloads into the world's data/ directory, which merge.py does not
        # carry across cells - see the note in arnis_cmd.build_arnis_cmd.
        "signage": "none",
        "job_size_regions": 4,   # sweet spot: small save bursts, safe on any disk (see Workers note)
        # 8 chunks = 128 blocks per side. NOT a tuning knob, and lowering it saves nothing.
        #
        # Cost: none on disk. tile.rs aligns arnis's output to 512-block regions, so ANY halo
        # from 1 block to 512 writes the same (job_size_regions+2)^2 region files - measured over
        # seven real renders, 9 files and 37,822,464 bytes at halo 1 and at halo 8 alike.
        #
        # Correctness: the 128 is spent exactly. road_bearings.rs budgets 96 (a field-texture
        # domain is 192 blocks and asks for the road bearing at its own centre) + 32 (the bearing
        # lattice half-cell) = 128, with zero margin, and at 1:1 the elevation built-up Gaussian
        # independently wants ~90. Two independently rendered adjacent cells agree on 100.000% of
        # shared surface columns at 64 blocks and on 85.3% at 16 - the parcel grid rotates at the
        # seam, which is the exact regression the 2026-08-04 seam fix removed.
        #
        # To cut wasted work, raise job_size_regions instead: the share of generated regions that
        # survives the merge is J^2/(J+2)^2 - 44.4% at J=4, 56.3% at J=6, 64.0% at J=8.
        "seam_buffer_chunks": 8,
        "ground_level": -56,
        "rotation": 0,
        "terrain": True,
        # Skip regional hi-res elevation (USGS/IGN/GSI) -> AWS-only. Fewer per-tile retries on
        # big parallel runs (the regional providers rate-limit under Meld's burst), at ~30m res.
        "aws_only_elevation": False,
        "regional_elevation_only": False,
        # Pass --offline to the generator: cached/baked elevation only, no tile-server
        # fetches. Reads like a machine knob but is a WORLD choice (a cell with no cached
        # tiles renders flat ground), so presets carry it; listed here so apply keeps it.
        "offline_elevation": False,
        # Terrarium zoom used for elevation (pack download + Arnis generation). "auto" matches the
        # zoom's pixel to the block size for this scale (the right detail with no waste): 1:1->z15,
        # 1:10->z13, etc. Lower zoom = far fewer tiles + dodges the z14/z15 no-data holes. 11..15.
        "elevation_zoom": "auto",
        # Off by default for the fastest first build (roads + land cover + water + terrain only).
        # Turning it on is remembered per project via update_settings. roads/bridges/rails/water kept.
        "buildings": False,
        "roof": True,
        "interior": False,
        # Overture's satellite-detected building fill on top of OSM's mapped ones. Read by
        # arnis_cmd (defaulting True there too); listed here so presets can carry the toggle -
        # apply drops any key absent from these defaults.
        "overture": True,
        "land_cover": True,
        "fill_ground": True,   # solid floor under the surface, no holes
        "caves": False,        # vanilla-like cave worldgen in arnis (opt-in; auto-enables fill_ground)
        # Per-biome cave theme amounts, percent of the default share (100 = default,
        # 0 = off, 200 ≈ double area). Passed to the fork as --cave-biomes only when
        # something differs from 100, so the default stays byte-identical.
        "cave_biome_amounts": {"lush": 100, "dripstone": 100, "deepdark": 100,
                               "mushroom": 100, "ice": 100, "amethyst": 100,
                               "volcanic": 100, "coral": 100},
        "osm_bake_workers": 4,  # offline .pbf bake parallelism; UI caps at 8, auto from CPU cores
        "disable_height_limit": False,
        # Target Minecraft version. "" = the fork's default. Only versions with VERIFIED
        # constants are accepted (the fork refuses anything else rather than guessing a
        # DataVersion, which would yield a world that loads and then misbehaves).
        "mc_version": "",
        # Blocks kept free above the highest terrain / below the lowest when the world's
        # height is fitted. Only used with disable_height_limit.
        "height_headroom": 32,
        "height_underroom": 16,
        # Explicit world floor / ceiling, "" = derive from the terrain + the room above.
        # Multiples of 16 within -2032..2031; the fork REFUSES a value that would cut into
        # the terrain rather than silently shearing it.
        "world_min_y": "",
        "world_max_y": "",
        # Pre-bake per-chunk lighting so LOD mods (Voxy, Distant Horizons) render
        # distant chunks lit without visiting them (Arnis issue #1071). On by default
        # because Meld builds areas too large to fly through; slower + bigger files.
        "bake_lighting": True,
        "road_detail_level": "auto",       # auto: compact <0.7, clean >=0.7
        "trees": True,                      # stamp bundled schematic trees (off = procedural)
        "tree_realm": "auto",               # auto: realm from selection latlon; or a realm code
        # Relative popularity per height tier (the sliders): 100 = default share, 0 = off, 200 =
        # ~double. small <=6, medium 7-12, big 13-20, tall 21-28, giant 29-40 blocks. Giant OFF by
        # default + only renders at 1:1; tiny maps never place tall/giant. 0 tiers fall back smaller.
        "tree_size_weights": {"small": 100, "medium": 100, "big": 100, "tall": 100, "giant": 0},
        # Farmland texturing: split OSM farmland into a weighted mix of five patch
        # styles (relative area shares). The default farm=100 (rest 0) = stock tilled
        # farmland; the fork gets --field-mix only when a non-farm share is set, so a
        # default project stays byte-identical.
        "field_mix": {"coarse": 0, "plains": 0, "flower": 0, "farm": 100, "moss": 0},
        # Scatter small schematic props on farmland (off by default). density = relative
        # amount 0..64; only sent to the fork when the toggle is on.
        # Rock/bush schematic scatter: one selector — none | rocks | bushes | both.
        # Each 16x16 chunk of farm/grass/untagged land rolls 20% for one piece.
        "scatter_mode": "both",
        # Field-pattern zoom percent (25-400): scales all parcel/plot sizes.
        "field_scale": 100,
        # Also texture OSM grassland (meadow/grass/orchard) and untagged satellite land
        # with the field pattern, not just OSM farmland. ON by default: most of the plain
        # is untagged ESA cropland and would otherwise render as endless stock wheat
        # (verified by block audit). Untick for stock-arnis surfaces.
        "grass_texture": True,
        "land_texture": True,
        # Per-profile mixes (relative shares). Grassland = OSM meadow/grass/orchard +
        # satellite grassland; Untagged = satellite cropland OSM never mapped. Defaults:
        # grassland is grassy, untagged is open-plains-leaning (not half crops).
        "grass_mix": {"coarse": 6, "plains": 64, "flower": 22, "farm": 0, "moss": 8},
        "untagged_mix": {"coarse": 15, "plains": 40, "flower": 10, "farm": 25, "moss": 10},
        # Farm-plot crop shares: each farm parcel grows ONE crop picked by these
        # weights (real monoculture plots). Defaults = the combined patchwork.
        "farm_crops": {"wheat": 40, "potato": 15, "carrot": 15, "beetroot": 8,
                       "sunflower": 12, "pumpkin": 5, "fallow": 5},
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
        # Bundled schematic props placed at OSM features. All on by default. The UI
        # currently exposes the wired families; the rest stay on until wired.
        "props": {
            "boat": True, "car": True, "crane": True, "excavator": True,
            "fountain": True, "helicopter": True, "lighthouse": True,
            "playground": True, "starship": True, "tombstone": True,
            "tractor": True, "windturbine": True,
        },
        # World settings written into the generated world's level.dat (Java).
        "gamemode": "creative",     # survival | creative | spectator
        "world_time": 6000,         # ticks: 0 dawn, 6000 noon, 18000 midnight
        "map_item": False,          # post-merge: add a locked world map to the player inventory
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
        # Resize the worker pool from MEASURED per-cell CPU occupancy instead of the
        # stored max_workers. Off by default: it is advisory until switched on, and the
        # log says what it would do. A 1:20 cell measures ~1.02 cores while being handed
        # ~5 threads, so the honest worker count there is ~21, not 4 - but changing the
        # pool mid-run is the user's call. See src/occupancy.py.
        "worker_autoscale": False,
        # ── governor: adaptive worker/thread scheduling (src/governor.py) ─────────────
        # "off"    legacy scheduling formulas — byte-identical to previous releases (DEFAULT)
        # "advise" measure + log the worker/thread target it would pick, change nothing
        # "auto"   actually move the pool size and the per-cell thread split mid-run
        # MELD_GOVERNOR=off in the environment force-overrides this (kill switch), so a
        # machine that misbehaves can be put back on the legacy path without editing a project.
        "governor_mode": "off",
        # Learned scheduling memory, keyed "<scale_bucket>/<cell_size>" ("1:1/4", "1:10+/8", …):
        # {"workers","threads","flush","cores_per_cell","rss_p95_mb","cells_per_min","ts"}. Lets a
        # second run of the SAME shape warm-start at the measured knee instead of re-climbing to it
        # (at 1:1 the knee is ~8-12 workers and every worker past it only inflates per-cell time).
        "governor_history": {},
        # RAM the governor refuses to spend: never admit another worker while available memory is
        # below (measured rss_p95 + this). 512..8192 MB. The measured 93%-RAM run at cell_size 8 is
        # what this exists to prevent — that build was one worker away from swapping.
        "ram_headroom_mb": 2048,
        # Ceiling for the region-save/flush thread pool handed to each cell. 1..24.
        "flush_threads_cap": 12,
        # Governor's own worker ceiling. 0 = use max_workers as the ceiling (the normal case);
        # >0 = an explicit ceiling independent of max_workers. 0..64.
        "governor_max_workers": 0,
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
        # Days before a cached OSM grid tile counts as stale and re-downloads (0 = never).
        # Long on purpose: a mass expiry re-fetches a whole country through the public
        # Overpass rate limit (hours of "Fetch OSM" on an area that was already local).
        "osm_cache_ttl_days": 365,
        # Region data pack: how many elevation tiles the bulk downloader pulls at once. 16 keeps
        # one controlled process well under any S3 throttle (vs the per-cell burst that flat-seams).
        "datapack_tile_concurrency": 16,
        # Pre-warm AWS terrain tiles once (serial, single process) before the parallel cells,
        # so the cells hit the cache instead of bursting S3 (which truncates tiles -> flat seams).
        "prefetch_terrain": True,
        # Stream regions to disk during generation (upstream Arnis --stream-to-disk). Lets a
        # single cell be 8x8/16x16 without OOM. Only used if the arnis binary supports the flag.
        "stream_to_disk": False,
        # ── phase 2 perf work: kill switches + measurement (docs/perf-phase2-plan.md) ──
        # The first two gate work that is NOT implemented yet. They exist now so the later
        # change lands flag-gated from its very first commit instead of being switched on by
        # the act of merging it — if you go looking for the feature behind them, it is not
        # missing, it has not been written. Both default to today's behaviour.
        #
        # Emit arnis's --canonical-regions (plan task B1): let a cell write only the regions it
        # actually owns instead of the full (job_size_regions+2)² halo. False = omit the flag
        # entirely, so arnis writes exactly the same region files it writes today, and an older
        # bundled arnis.exe never sees an unknown clap argument.
        "canonical_regions": False,
        # Generator quality passes (arnis --river-bed / --road-grade). Unlike every other
        # key here these deliberately CHANGE the generated world, so they stay off until
        # asked for: a project regenerated with one of them on will not match cells it
        # rendered before. Rivers get a smooth width-scaled U bed (lakes and oceans keep
        # their own beds untouched); roads get a slope-limited longitudinal profile so a
        # carriageway ramps instead of stepping on terrain contours.
        "river_bed_v1": False,
        "road_grade": False,
        # ── arnis 3.2.0 options ───────────────────────────────────────────────────────
        # Every one of these needs a generator that advertises the matching flag; against
        # the 3.1.8 fork arnis_cmd drops them all and the command line is 1.9.8's exactly.
        # Defaults chosen to match the generator's own, so a project that never touches
        # them renders the same world before and after the version bump.
        #
        # Voxy LOD pregeneration: the world renders to the horizon on first join instead
        # of needing /voxy import current. Java only, and it implies --bake-lighting
        # because unlit LOD terrain renders black.
        # Classic renders the way this fork always has: arnis_cmd emits none of the 3.2.0
        # flags below, whatever they are set to. Enhanced sends them. The switch exists
        # because everything under it is new and some of it calls an outside service, so
        # the old path stays one control away.
        "gen_mode_32": "classic",
        "voxy_lod": False,
        # Which celestial body to generate. moon/mars carry no OSM data and drive their
        # own scale, so every object option is ignored for them.
        "body": "earth",
        # Which transport Overture buildings are read through: auto (vector tiles falling
        # back to Parquet), tiles (no fallback, so a broken archive is visible rather than
        # merely slow), or parquet.
        "overture_source": "auto",
        # Premade facade photographs picked by building type. Needs no token, Java 1.21.4+
        # only, and it turns the Mapillary facades off - both hang panels on the same wall.
        "building_facades": False,
        # Mapillary street-level facades. The token alone turns them on, which is why the
        # switch is a tri-state: None follows the token, False keeps the token and skips
        # the download. The token itself is a credential - it is never written into a
        # preset or a world's metadata, and it reaches the generator through the child's
        # environment rather than on the command line.
        "mapillary_token": "",
        "mapillary_facades": None,
        "mapillary_facade_mode": "photos",
        "facade_detail": "standard",
        "facade_px": "16",
        # Pre-parsed OSM tile sidecars (.osmbin next to each cached tile .json). Cuts the
        # repeat-render decode cost (measured 946 -> 429 ms/cell) at the price of roughly
        # two thirds extra OSM cache size. On by default; turn off on tight disks. The
        # lifecycle reaper keeps pairs consistent either way.
        "osm_sidecars": True,
        # Ask the fork for its fast OSM tile parser (plan task A1: serde_json from_slice instead of
        # the from_reader IoRead path, plus a cheaper dedup key). False = the fork parses tiles
        # exactly as it does today. The switch exists so the replacement can be A/B'd against the
        # current parser and turned back off in one setting if the two ever disagree by a block.
        "parse_fast_json": False,
        # Emit the phase-2 per-phase timers (merge / prune / health / meta) into the run report:
        # summary.timers + cells[].timers, schema "meld-run-report/4". True = today's behaviour,
        # because the timers ship on by default — they only measure, they change no scheduling
        # decision. Off drops the new keys; a schema/3 reader ignores them either way.
        "phase2_timers": True,
        # World management
        "prune_cell_after_merge": True,   # delete per-cell subregion after merge (saves storage)
        "master_world_dir": "",            # where the merged world lives ("" = <project>/Meld World)
        "origin_corner": "nw",             # which selection corner the origin snaps to on Plan
        # Export / compression (see src/export.py + repo MELD_EXPORT_PLAN.md). DEFAULT = none
        # so an untouched build yields a working vanilla .mca world. Benchmarked picks:
        #   none   raw .mca (vanilla SP, largest)
        #   zip    universal archive, extract → vanilla SP (~1.85×)
        #   tarzst portable tar.zst, extract → vanilla SP, good for sharing (~1.85×)
        #   linear per-region .linear, SERVER ONLY (Leaf/Folia), smallest on disk (~4.85× @ L9)
        "export_format": "none",
        # Compression level: 0 = the format's sensible default (zip 6, tarzst/linear 9).
        "export_level": 0,
        # Compression workers. 0 = auto = logical cores − 1 (reserve one) when cores ≥ 4.
        # INDEPENDENT of max_workers (generation) by contract — do not couple them.
        "export_compression_workers": 0,
        # Keep both the raw .mca world AND the compressed copy. Safe default ON: the worst
        # tolerable failure is raw-present/compressed-missing (re-runnable).
        "export_keep_both": True,
        # Low-disk: delete each region's raw immediately after its compressed copy verifies
        # (linear only), so peak disk ≈ compressed size. Off by default. Implies keep_both off.
        "export_stream_and_free": False,
        # Overlap compression WITH generation instead of one post-pass at the end. linear =
        # parallel per-region streaming (real peak-disk win with delete-raw). zip/tarzst =
        # single-writer stream-add to one container (overlaps the build; raws kept till verified).
        # Off by default (post-pass is simplest + safest). See src/export.py.
        "export_overlap": False,
        # Where the compressed output lands (LINEAR only; archives are always a sibling file):
        #   in_place — write region/*.linear next to the .mca in the SAME world folder (current).
        #   separate — build a sibling "<name> [Linear]" world; the original stays untouched
        #              vanilla .mca. Safest mode (never mutates/deletes the source) + the cleanest
        #              for users: keep an MCA world to play AND a Linear world to serve, side by side.
        # Separate-folder forces post-pass (no overlap) and keep-both is implicit (source kept).
        "export_destination": "in_place",
        # B_Linear (.b_linear) variant when export_format = "blinear" (Rust region-convert):
        # v3 (default, bucketed, what Leaf B_LINEAR reads) or v2 (older). blinear always builds a
        # sibling "<name> [BLinear]" world; the source stays untouched.
        "export_blinear_variant": "v3",
        # What happens to the master .mca AFTER the [BLinear] world verifies:
        #   both         keep the .mca world too (play locally + serve)   — safe default, ~1.3× disk
        #   blinear_only delete the master .mca (server-only)             — ~0.3× disk
        #   archive_mca  zip the .mca then delete it (vanilla backup)     — ~0.85× disk
        # The .mca is only ever removed AFTER the [BLinear] world (and the zip, for archive_mca)
        # verify — never before. A failed/partial run is forced back to "both" (safeguard B).
        "export_blinear_keep": "both",
        # EXPERIMENTAL. Region container the fork GENERATES directly, skipping the
        # separate .mca -> .b_linear conversion pass entirely:
        #   mca     Anvil (default) — universal: vanilla client, Paper, every server
        #   blinear Leaf B_Linear v3 — SERVER ONLY, and only Leaf 1.21.11 (June 2026
        #           builds) or newer / 26.x. Not readable by Paper, older Leaf, or the
        #           vanilla client, and there is no .mca original to fall back on.
        # Worth it at scale: ~3.7x smaller on disk for a dense city, and the conversion
        # pass disappears. The map item is unavailable for these worlds (its renderer
        # only reads Anvil).
        "native_region_format": "mca",
        # zstd level for native blinear buckets, 1..22. Leaf's own default is 6.
        "native_blinear_level": 6,
        # EXPERIMENTAL. Evaluate cave density on a GPU: off | auto | dgpu | igpu.
        # Worth it on 1:1 renders with caves (measured 1.18x wall / 1.37x fleet on an
        # RTX 5080; the iGPU is nearly as good). Does nothing measurable at 1:20.
        # Approximate by contract: f32 shifts the odd cave wall vs the CPU (measured
        # 0.0005% of blocks). Falls back to CPU when no adapter matches.
        "gpu_accel": "off",
        # One-click Leaf server profile — the Server setup card remembers its choices
        # per project (see server.py /api/mcserver/*). server_dir "" = the default
        # <project>/server/leaf-<version>. Reachability is NOT stored here: staging
        # always writes the localhost/offline profile; going public is a deliberate
        # per-session switch.
        "server_version": "",
        "server_mode": "main",
        "server_dir": "",
        # Which world files feed the server: auto (follow Export settings) or an
        # explicit mca / linear / blinear pick. The Leaf region-format always
        # matches whichever files are staged.
        "server_world_src": "auto",
        "server_extras": False,
        "server_voxy": False,
        "server_auto_restart": True,
        # JVM resources for the Leaf server. server_ram_gb 0 = auto (a quarter of the
        # machine's RAM, 2..8 GB); the heap is always capped 2 GB below total so the OS
        # and Meld keep headroom. server_cpu_pct maps to -XX:ActiveProcessorCount.
        "server_ram_gb": 0,
        "server_cpu_pct": 100,
        # There is deliberately no server_backup_first here any more. It defaulted True,
        # which is why "default off since 1.7.0" never actually held: every project was
        # born with the stored True and the first start zipped a 1 GB world into a 1 GB
        # zip at the exact moment the user asked to play. Backups are the 💾 button now,
        # on demand only; the project's master world is always the untouched source.
    }


# ── legacy → governor migration ──────────────────────────────────────────────────────────
# worker_autoscale is the pre-governor opt-in: "resize the pool from measured CPU occupancy".
# The governor is that idea finished, so a stored True means the user already asked for it and
# must land on governor_mode="auto" rather than silently reverting to the legacy formulas.
#
# PURE: takes a settings blob, returns the PATCH to apply (possibly empty). It reads no files,
# writes no files and mutates nothing — server.py calls it at load and feeds the result to
# Project.update_settings, so the migration happens exactly where every other write happens.
_GOVERNOR_MODES = ("off", "advise", "auto")


def _governor_mode_was_written(settings) -> bool:
    """Has the user ever chosen a governor_mode for this project?

    Project.settings() merges default_settings() UNDER the stored blob, so the merged dict
    always carries a governor_mode — "off" from the defaults reads exactly like an "off" the
    user picked. Provenance is the only way to tell them apart, so Project.settings() returns
    a _SettingsView that remembers which keys were actually persisted.

    Without that provenance (a plain dict — a hand-built blob, or a copy that dropped the
    attribute) fall back to "a recognised mode string means written". That errs toward
    LEAVING THE MODE ALONE, which is the safe direction: the cost is a legacy autoscale
    project staying on the legacy formulas, versus overriding a mode the user set."""
    stored = getattr(settings, "stored_keys", None)
    if stored is not None:
        return "governor_mode" in stored
    mode = settings.get("governor_mode")
    mode = mode.strip().lower() if isinstance(mode, str) else ""
    return mode in _GOVERNOR_MODES


def migrate_governor_settings(settings: dict) -> dict:
    """Patch that carries a legacy worker_autoscale=True project onto governor_mode="auto".

    Migrates ONLY when governor_mode was never written. Any mode the user actually chose —
    "off" included — is theirs and is left alone; the patch then just retires the legacy flag.
    This runs at boot AND on every project switch, so the earlier "mode not in (advise, auto)"
    rule re-imposed "auto" on every load of a project whose owner had deliberately turned the
    governor off (the stored default "off" is indistinguishable from a chosen "off" without
    provenance — see _governor_mode_was_written).

    Idempotent by CONSUMING the legacy flag: the patch always clears worker_autoscale, so
    applying it twice yields {} the second time. The key itself stays in default_settings for
    one release (read-only, so old projects still round-trip)."""
    if not isinstance(settings, dict):
        return {}
    if not bool(settings.get("worker_autoscale", False)):
        return {}
    if _governor_mode_was_written(settings):
        return {"worker_autoscale": False}       # user owns the mode — just retire the old flag
    return {"governor_mode": "auto", "worker_autoscale": False}


class _SettingsView(dict):
    """Merged settings that remember WHICH keys came from disk (`stored_keys`).

    A plain dict everywhere it matters — json.dumps, {**s}, .get, ==, iteration — so every
    existing reader is unaffected. Only migrations that must distinguish "the user chose this"
    from "a default filled the hole" look at stored_keys. Any copy (dict(s), {**s}) drops the
    attribute and readers fall back to their conservative path."""

    __slots__ = ("stored_keys",)

    def __init__(self, merged: dict, stored_keys=()):
        super().__init__(merged)
        self.stored_keys = frozenset(stored_keys)


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
        """Defaults with the stored blob on top. The result is a _SettingsView: a dict that
        also remembers which keys were actually persisted, so migrate_governor_settings can
        tell a chosen "off" from the default "off"."""
        stored = self.load().get("settings")
        if not isinstance(stored, dict):
            stored = {}
        return _SettingsView({**default_settings(), **stored}, stored.keys())

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
