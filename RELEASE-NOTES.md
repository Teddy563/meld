# Release notes

Short, human highlights for each Meld release. Full detail lives in [CHANGELOG.md](CHANGELOG.md).

## v1.2.0

**Offline, faster, cleaner.**

1.1.0 made builds bigger; 1.2.0 makes them offline and fast. You can now bake a whole region's OSM
once from a local Geofabrik `.pbf` and generate with zero Overpass calls, and the OSM cache is keyed
to a fixed map grid so two overlapping selections reuse the same tiles instead of re-downloading. On
the generation side, the biggest per-cell cost turned out to be a supplementary building fetch that
ran on every cell even when you'd turned buildings off, that's gone for roads-only builds (a measured
cell dropped from ~29s to ~4s), and each cell now reads its OSM straight from the shared tile cache
with no merge step at all. Your drawn area is finally remembered across a restart, per world. And the diagonal
water-and-sand "wedges" that sometimes slashed across otherwise-perfect terrain are fixed.

Highlights:

- **Build offline.** Drop Geofabrik `.osm.pbf` files in a folder, bake them once, and generate with
  no Overpass, pair it with the elevation packs from 1.1.0 for a fully local region. New **OSM data
  pack** card: check coverage, bake, scan folder, watch progress.
- **OSM cache that reuses.** Map data is cached on a fixed grid, so a 90%-overlapping selection
  downloads only the new edge, and an identical re-run downloads nothing.
- **Much faster cells.** The Overture building fetch (about 93% of a cell's time) is skipped on
  roads-only builds; with buildings on it now caches to disk and downloads once instead of per cell.
  Each cell reads its OSM tiles directly with no merge step, the terrain warm is skipped when elevation
  is already cached, and rate-limited tiles retry-and-cache instead of re-fetching every run.
- **Your area is remembered.** The selection and cells save into each project and redraw on restart,
  so there is no re-drawing the country after a server restart.
- **No more water wedges.** Triangular water/sand bleed across terrain (from water polygons that
  cross a cell edge) is fixed by clipping every water ring to the cell before it's filled.
- **Detail + reliability.** Road-detail clean/compact modes, a custom Overpass URL, sub-world
  operations, and disk-recovery of orphaned patches.

> Setup: `pip install osmium` for `.pbf` baking. A region is fully offline once it's both elevation-
> and OSM-packed. Restart the server after a bake. Buildings are off by default; turning them on
> downloads Overture once per partition (slow the first time, cached after).

## v1.1.0

**Go bigger, see more, waste nothing.**

Meld turns the real world into one seamless Minecraft world at scale, on a single PC. On the same
area it runs about 2x faster than a single Arnis pass, because it builds the tiles in parallel
instead of one after another. The ceiling on that speed is how fast your disk can save the regions,
not your CPU. The real win is scale: build a whole city, country, or continent as one world, with no
seams and no height cliffs at the joins. 1.1.0 adds the reliability to match. It repairs the
elevation no-data holes that caused dark bands and in-game dips, smooths water artifacts, removes
duplicate block entities on the parallel path, and fixes the crashes big parallel runs could hit.

Highlights:

- **Bigger builds.** The Arnis fork gained an in-process multi-core engine and stream-to-disk, so a
  single cell can now be huge (8x8 or 16x16 regions, up from a cap of 6). Big cells build their
  tiles in parallel inside one process and write finished regions to disk as they go, so they finish
  without running out of memory.
- **Region data packs.** Pull a whole region's elevation once into the shared cache, then generate
  offline and never rate limited. Check coverage as a percent, see exactly which tiles are missing,
  or import a folder of tiles from another machine with no download.
- **Height preview.** A grayscale or hillshade overlay of the cached elevation, right on the map, so
  you can see the terrain before you build. Red means a tile is not cached yet. Click a tile for its
  height range, size, and status.
- **No-data hole repair.** The source data has real gaps at its highest zooms that showed up as dark
  bands in the preview and flat dips in game. Meld now rebuilds each hole from the deepest zoom that
  does have data, for one tile, a drawn selection, or the whole cache. New downloads also self-heal.
- **Selectable elevation detail.** An Elevation detail dropdown picks the terrain zoom, or Auto
  matches it to your scale (1:1 picks the finest level, 1:10 picks a lighter one). A lower zoom is
  far fewer tiles, dodges the no-data holes, and stays lossless against the roughly 30 metre source.
- **One shared cache.** OSM, terrain, and land cover now live in one visible folder reused by every
  project and world, instead of being hidden away and re-downloaded each time. The Cache card shows
  where it lives and the size of each type, with Clear buttons.
- **A live status rail.** A panel down the left side mirrors your machine and your run: live CPU,
  RAM, and disk gauges with a low-disk warning, the build estimate and timer, a row per worker, and
  the log. Failed cells now say why on hover (out of memory, disk full, rate limit, timeout, crash).
- **No-buildings mode, road detail, and flat bridges.** A Buildings toggle for a roads and
  land-cover only world. A Road detail mode that keeps roads legible at small scales. And flat
  one-block bridges below scale 0.3 so tall arches do not collapse into noise.
- **The fixes.** Floating vegetation over water and roads on big or streamed exports, duplicate
  banners, signs, and chests on the parallel path, a worker-thread crash on far-from-origin
  coordinates, and a desync when the plan was edited mid-run, are all fixed.

Upgrade:

- After you pull this release, restart the server and hard-refresh the browser so the new tiles and
  the new UI show.
- The new `arnis.exe` (the Arnis fork at version 2.9.1) is bundled. If you keep your own binary, drop
  the matching build next to `server.py`.
- A region data pack you already downloaded keeps working. Run Check coverage to confirm it before a
  big build.

Heads up:

- Cell sizes 8 and 16, region data packs, and the elevation zoom chooser are power-user features.
  Build a small area first to confirm your scale, elevation detail, and save location before you
  commit a whole country.
- Big builds can be tens of gigabytes on disk, so keep an eye on the free-space bar.
- Generation is offline-friendly once a region is packed, but the first pack download still needs a
  connection.

## v1.0.0

**Meld turns the real world into one seamless Minecraft world, at scale, on a single PC.** Draw an
area, pick a cell size, and Meld tiles the selection, builds every tile in parallel with a custom
Arnis fork, and merges them into one master world. Every seam lands on a Minecraft region boundary,
so the join is exact and the surface is about 99 percent seamless, with no height cliffs and no
Overpass rate limits. A shared OSM prefetch, one global elevation lock and a tile invariant seed,
Recommend to tune cell size and workers to your machine, resume and retry, multi world saves, and
baked chunk lighting for Distant Horizons and Voxy.

Get it: clone [Teddy563/meld](https://github.com/Teddy563/meld), `pip install -r requirements.txt`,
`python server.py`, and open `http://127.0.0.1:5630`. The generator is the custom
[Arnis fork](https://github.com/Teddy563/arnis), shipped as `arnis.exe`.

Built on the open source [Arnis](https://github.com/louis-e/arnis) generator by louis-e. Not
affiliated with Mojang AB or Minecraft.
