# Changelog

All notable changes to Meld are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Meld follows
[Semantic Versioning](https://semver.org).

## [1.3.0] - 2026-06-19

The "guided start, live tuning, and a benchmark report" release. Same engine, much easier to drive
and to understand. The right rail is now one numbered, guided flow instead of a wall of options, and a
one-click **Prepare and build** runs the prep steps in order. You can retune a run **while it runs**,
workers, threads and CPU budget all apply to the next cells, and every finished or stopped run writes
a **benchmark report** into the world folder: a themed page with the machine specs, CPU/RAM and
activity graphs, a per-worker timeline, and a Save-as-PDF. New projects start with defaults tuned for
a fast first build, the scale field reads out the real ratio, cell size is a free 1 to 64 fill-in, and
the live CPU/RAM gauges read accurately now.

> No engine change: this release ships the same Arnis fork (Teddy563/arnis 2.9.1) as 1.2.0. Every
> change here is in the Meld orchestrator, its UI, and the docs. Existing worlds and settings are
> untouched; the new defaults apply only to brand-new projects.

### Added

- **Benchmark report.** Every run that finishes (or is stopped midway) writes `meld-report.html` and
  `meld-report.json` into the world folder. The HTML is a themed, self-contained page: summary tiles
  (total time, cells merged, on disk, peak workers, median and slowest cell), the **machine** (CPU
  model, physical cores and logical threads, RAM type and speed, drive type) and the exact **run
  settings**, **CPU and RAM over the run**, **activity over the run**, a per-worker **cell timeline**
  (a Gantt with a merge playback), and a per-cell table. **Save as PDF** lays it out as two pages.
  Open it from **Benchmark report** in the Build card, or **View benchmark** when a run finishes; the
  full per-cell list is in the JSON. Backed by `/api/report`, `/api/report.json`, and a new
  `src/runreport.py`.
- **Live mid-run tuning.** Change **Workers**, **Threads per worker** or **CPU budget** during a run
  and the next cells a worker picks up use the new values, with no restart and no re-plan. The world
  invariants (origin, seed, elevation lock, scale, the cell grid) stay frozen, so a mid-run tweak can
  never desync the world.
- **One-click Prepare and build.** A button next to Generate runs the prep your settings need, in
  order, then generates: bake OSM if you pointed at a `.pbf` folder, warm the Overture building cache
  if buildings are on, wait for each, then build.
- **Readable scale.** The scale field shows the live ratio (for example `1:10`) and what it means: one
  block is N metres, and a 1 km city is X blocks wide. Editing the number updates the readout.
- **Explore mode (teleport lookup).** A 🗺️ **Explore mode** toggle in the Build card hides the cell
  preview, shows the world's border, and turns the map into a coordinate picker: click anywhere and
  Meld pops up the Minecraft teleport command for that spot (`/tp @s X ~ Z`), computed from the project
  origin and scale, with a Copy button. A search box at the top right of the map finds and zooms to
  any place while you explore. The world does not need to be built; it uses the same origin-anchored,
  scale-aware formula as the live coordinate readout.
- **Open world folder.** A button in the Build card opens the saved world in your file browser, and
  falls back to the first folder that exists if the save location was moved or disconnected.

### Changed

- **One guided rail (no mode toggle).** The right rail is a single numbered, top-to-bottom flow, steps
  1 to 6 (Settings, Project and world, Selection, Edit and retry, Prepare data, Generate), with the
  advanced cards (Elevation lock, Subregions) collapsed at the bottom. Every control is present; there
  is no Simplified/Advanced switch to think about.
- **New-project defaults tuned for a fast first build.** New projects start at **scale 1:10**,
  **buildings off**, **solid ground fill on**, and **4 threads per worker**. Existing projects keep
  their saved settings.
- **Cell size is a free 1 to 64 fill-in.** The cell-size dropdown is now a number field (presets 1, 2,
  4, 6, 8, 12, 16, 32, 64). Any integer aligns to its own region grid; 8 and up auto stream-to-disk so
  they do not run out of memory.
- **Recommend tunes workers and threads.** Recommend now suggests a worker count and a
  threads-per-worker so workers times threads fits your logical CPUs, and applies both. It counts the
  product as threads against your hardware threads, not a confusing "of N cores".
- **CPU-bound framing, everywhere.** The docs, Recommend, and the worker and thread tooltips lead with
  the real rule: generation is mostly CPU bound, keep workers times threads at or under your cores
  (logical CPUs / hardware threads). Stream to disk handles the big-tile memory burst; RAM and
  save-disk speed are secondary caps.
- **Browse-only folder pickers, flattened Prepare.** The OSM and elevation pickers are a single Browse
  button plus a Geofabrik link, and the Prepare data step no longer nests dropdowns.
- **Snappier, event-driven UI.** The rail no longer polls once a second when idle; polling kicks on
  your actions (Generate, Stop, Plan) and idles to a slow heartbeat, tightening only while a build
  runs. The worker list and log redraw only when they change. The live left-rail activity squares were
  dropped (they did not scale to big runs); the activity graph lives in the report instead.

### Fixed

- **CPU gauge stuck near 0%.** CPU was read with `cpu_percent(interval=None)` from the request path,
  which measured the sub-second gap between two polls under the threaded server. A dedicated background
  sampler (1-second rolling average) now feeds an accurate CPU%; the live gauge and the report both
  match Task Manager.
- **RAM read low.** RAM "in use" is now total minus available (what Task Manager shows), not psutil's
  `used`, which under-reports on Windows.
- **Open world folder did nothing** when the save location pointed at a moved or disconnected drive; it
  now climbs to the first existing folder and shows the path if it still cannot open.
- **Laggy buttons.** The per-second tick is gone; Stop responds instantly and the rail stops repainting
  needlessly.
- **Benchmark PDF stays two pages** with many workers: the cell timeline now compresses its lane height
  to fit instead of spilling onto a third page.

## [1.2.0] - 2026-06-18

The "offline, faster, cleaner" release. Meld can now build a whole region with **zero Overpass calls**:
bake OSM once from local Geofabrik `.pbf` files, and cache it on a fixed grid that overlapping
selections reuse. Generation got much faster: the supplementary Overture building fetch
(measured at about 93% of a cell's wall-clock) is skipped when you build roads-only, and each cell now
reads its OSM straight from the shared tile cache with no per-cell merge step. Drawn areas survive a
restart, per project. And the diagonal water and sand "wedges" that bled across correct terrain are
fixed at the source.

> Engine note: the Arnis fork (Teddy563/arnis 2.9.1, branch `merge-upstream-2026`) gained
> `--osm-tile-dir` (a cell reads its own grid tiles directly), an Overture gate plus on-disk cache, and
> a water ring-closure fix that drops the wedge artifact. The deployed `arnis.exe` is rebuilt from
> `arnis-283-src`. Meld's tile-invariant seam is unchanged.

> Heads up: OSM data packs need `pip install osmium` plus Geofabrik `.osm.pbf` files dropped in a
> folder. A region is fully offline once it's BOTH elevation-packed (1.1.0) and OSM-packed/cached.
> Restart the server after a bake so coverage reads it. Buildings are off by default (`--no-buildings`);
> turning them on triggers a one-time per-partition Overture download (slow first run, cached after).

### Added

- **OSM data packs (offline `.pbf` bake).** Bake OSM straight from local Geofabrik `.osm.pbf` files
  into the shared cache, so generation needs no Overpass at all. The **OSM data pack** card has Check
  coverage, Bake from .pbf, Scan folder, and live progress; baked tiles drop into the same grid the
  live fetch fills, so the two are interchangeable. New optional dependency `osmium` (pyosmium),
  lazy-imported only during a bake. Backed by the `/api/osmpack/*` routes.
- **Stable OSM grid cache (reuse across selections).** OSM is now cached on a fixed web-mercator
  slippy grid (z11) keyed only by `(z, x, y)`, independent of scale and selection. Two overlapping or
  near-identical selections share their interior tiles verbatim; only genuinely-new edge tiles fetch,
  and an identical re-run downloads nothing, fixing the old "re-downloads OSM every time the
  selection shifts" behaviour (the cache used to be keyed by the per-clump bbox).
- **Per-project selection persistence.** The drawn area, its polygon, and the planned cells now save
  into each project's `project.json` and redraw automatically on a server restart, per project,
  instead of a single global browser key shared across every world. Backed by `/api/selection` and
  returned in `/api/state`.
- **Road-detail modes.** A road-detail control (`auto` / `max` / `clean` / `compact`) on the Arnis fork and
  the Meld settings, to thin or simplify road rendering.
- **Overpass URL override.** Point OSM fetching at a custom or self-hosted Overpass endpoint
  (`--overpass-url`), for the live-fetch tail and gap fills.
- **Sub-world operations.** Carve / re-run sub-regions of an existing world.

### Changed

- **Overture buildings are gated on `--no-buildings`.** The fork's supplementary Overture Maps
  building fetch, a per-cell network round-trip measured at **~26.8s of a 28.8s cell (~93%)**, now
  runs only when buildings are enabled. A roads-only (`--no-buildings`) cell dropped **28.8s → 4.2s**.
- **Each cell reads its OSM tiles directly (no merge step).** Meld no longer assembles a per-cell or
  per-clump Overpass file. The Arnis fork takes `--osm-tile-dir <cache/osm>` and reads the cell's own
  z11 grid tiles straight from the shared cache, computing the covering tiles from `--bbox` and
  de-duplicating by (type, id). That removes the per-run "assembling" pass entirely and shrinks each
  cell's parse from the whole clump superset to just its own roughly 9 to 16 covering tiles. Verified
  identical to the old per-cell merge (same de-duplicated element set, same world output within the
  generator's own run-to-run variance).
- **Terrain warm is skipped when elevation is already cached.** The serial per-run terrain
  re-validation sweep is skipped entirely when elevation coverage is ≥99% at the build zoom, removing
  minutes from every run on a complete pack (the per-cell live fallback still covers any gap).
- **Elevation zoom is pinned through the terrain warm** (`ARNIS_ELEV_ZOOM`), so the warm and the cells
  agree on zoom and the warm actually populates the cache the cells read.

### Performance

- **Overture range cache + pre-warm.** When buildings ARE on, the STAC index and each GeoParquet
  byte-range (footer + row groups) cache to disk under `arnis-overture-cache/`, keyed by `(url,
  offset, length)` so every cell after the first reads its building data from local disk, only the
  few MB a cell uses are ever fetched, never the whole ~580 MB partition, and there's no lock to stall
  the build. A new **Buildings (Overture)** data-pack card + `arnis --prewarm-overture` flag bulk-
  download a region's ranges up front, in parallel, so a buildings-on build never stalls on a cold
  fetch (run it like the elevation/OSM packs; skip it for `--no-buildings`).
- **Empty sentinel tiles.** The bake writes a valid empty tile for sea / no-OSM / outside-`.pbf`
  areas, so coverage reads a truthful 100% and those tiles are never re-fetched live on each run.
- **Retry + backoff on transient tile fetches.** A rate-limited or timed-out grid-tile fetch now
  retries with backoff and caches on success, instead of falling to a per-run live fetch forever.

### Fixed

- **Triangular and rectangular water / sand wedges.** A water multipolygon whose member ways were not
  all loaded for a cell (a lake or river that extends beyond the cell's tiles) left its outer ring
  open. The bbox clip then closed that open ring with a straight chord, and the scanline fill flooded
  the whole side of it with a **triangle of water** plus a matching **sand shore band** along the same
  diagonal. The fork's `clip_water_ring_to_bbox` now rejects an open input ring (first node not
  matching the last by id or within one block) and drops it, instead of closing a broken outline with a
  fake straight edge. Rings that are properly closed still render, so legitimate water is preserved.
  Verified on real 1:10 data: a wedge cell went from 135,600 to 4,555 water blocks (the triangle gone,
  the real river kept) while a clean river cell came out byte identical. Both wedge colours are gone
  from one fix; the shore band recomputes from the corrected water.
- **Reconcile / patch recovery.** Orphaned pre-generated patches are recovered by scanning disk; a
  missing reconcile route now returns a clear "restart the server" error instead of failing opaquely.

[1.2.0]: https://github.com/Teddy563/meld/releases/tag/v1.2.0

## [1.1.0] - 2026-06-16

The "go bigger, see more, waste nothing" release. Meld's Arnis fork gained upstream's in-process
multi-core engine and stream-to-disk, so single cells can be huge (8x8, 16x16). The UI grew a live
status rail (CPU / RAM / disk, workers, build, log), a paint tool, a retry queue, and CPU controls.
All map caches moved into one shared, visible folder reused by every world. And a per-spawn cache
walk that was quietly adding seconds to every cell's startup is gone.

> Engine note: the fork (Teddy563/arnis, now version 2.9.0) carries a merge of 53
> upstream `louis-e/arnis` commits - spatial tile parallelization + stream-to-disk. The Meld seam
> (master-origin tile-invariant rendering) was preserved through the merge and verified: two
> independently generated overlapping cells agree block-for-block (0 of 1024 boundary chunks differ).

> Heads up: cell sizes 8 and 16, region data packs, and the elevation zoom chooser are new and best
> treated as power-user features. Build a small area first to confirm your scale, elevation detail,
> and save location before you commit a whole country. Big builds can be tens of gigabytes on disk,
> so keep an eye on the free-space bar. After you download or repair an elevation pack, restart the
> server and hard-refresh the browser so the new tiles show. Generation is offline-friendly once a
> region is packed, but the first download still needs a connection.

### Added

- **In-process multi-core generation.** A cell spanning >= 3 region-tiles now builds its tiles in
  parallel inside ONE Arnis process (rayon), on top of Meld's existing cross-process parallelism.
  Small production cells keep the unchanged sequential path; big test cells go wide.
- **Stream-to-disk.** Big cells evict finished regions to disk during generation instead of holding
  the whole world in RAM, so 8x8 / 16x16 test tiles complete without running out of memory
  (auto-enabled via the `ARNIS_STREAM_TO_DISK` env Meld sets for `size >= 8`).
- **No-buildings mode.** A `--no-buildings` flag (alias `--no-structures`) on the Arnis fork, a
  **Buildings** toggle in both the Arnis GUI and the Meld settings, for a roads + land-cover only
  world. Roads, bridges, railways, water, natural and terrain are all kept; building footprints are
  emptied too, so land cover fills in cleanly with no building-shaped holes. (Verified: same dense
  area generates a different world with the flag on vs off.)
- **Cell sizes 1 / 2 / 4 / 8 / 16** (powers of two, default 4; 8 and 16 marked as testing). Cap
  raised from 6 to 16.
- **Left status rail.** A second thin panel mirroring the right: Meld logo, a live **System** card
  (CPU %, RAM used/total, save-disk free + low-disk warning), the **Build** estimate, **Workers**,
  and the **Log** at the bottom. Settings stay on the right.
- **Cache card.** Shows where the shared cache lives + per-type size (OSM / terrain / land cover)
  with Clear buttons. Backed by `GET /api/cache` + a run-guarded `POST /api/cache/clear`.
- **Paint tool.** In Edit mode, click-drag across the map to add or remove cells (the first cell
  decides add vs remove); the whole drag persists in one atomic write (`/api/cell/toggle-bulk`).
- **Select-to-retry.** Drag to mark a clump of cells (distinct blue dashed ring), then re-run them
  with one button (`/api/cell/regenerate-cells`).
- **CPU controls.** A **CPU budget** slider (10-95%), a **Threads / task** floor (1-8), a stagger
  **toggle + step** slider, and **adaptive pacing** (spaces worker starts from the measured average
  cell time so cores stay busy without all 16 launching at once).
- **Spiral generation order.** Cells build center-out in concentric rings instead of edge-first.
- **Failed cells say why.** Hovering a red cell now shows the parsed cause (out of memory / disk
  full / Overpass rate limit / network timeout / crash) instead of nothing.
- **Auto-retry.** A cell that fails for a transient reason (network / rate-limit / timeout / OOM)
  is re-queued up to 2x; deterministic failures (drift / collision / disk-full / panic) are not.
- **Shared global cache in the Meld project.** OSM, AWS terrain, and ESA land-cover caches now live
  under `meld/cache/` (override with `MELD_CACHE_DIR`), reused by every project/world instead
  of being hidden in AppData and re-downloaded per project.
- **Region data packs.** Bulk-download a whole region's elevation once into the shared cache, so
  generation runs offline and is never rate-limited. The Data pack card has Check coverage,
  Download elevation, a re-fetch-this-view button, a packs list, and Import folder (drop in a folder
  of tiles to use them with no download). Backed by the `/api/datapack/*` routes.
- **Height preview.** A grayscale or hillshade overlay of the cached elevation on the map, so you can
  see the terrain before you build. Red means a tile is not cached yet. Zoom out to a regional view
  or in for full detail. Click a tile for a popup with its height range, size, and status.
- **No-data hole repair (overzoom).** The AWS terrarium set has real gaps at z14/z15 where it serves
  an all-black no-data tile. Those showed up as dark bands in the preview and flat dips in game. Meld
  now rebuilds each hole by upsampling the deepest zoom that does have data, baked into the cache so
  both the preview and Arnis read real terrain. Fix one tile from its popup, the drawn selection, or
  the whole cache in one pass; new downloads also self-heal.
- **Selectable elevation detail (zoom).** An Elevation detail dropdown picks the terrarium zoom used
  for download and generation. Auto matches the zoom to your scale (1:1 picks z15, 1:10 picks z13),
  so you get the right detail with no waste. A lower zoom is far fewer tiles, dodges the no-data
  holes, and stays lossless against the roughly 30 m source. Wired to the Arnis fork through the
  `ARNIS_ELEV_ZOOM` env var.

### Changed

- Build/size stats and the log moved into the left rail; the cell-size field is now a dropdown.
- **Build-time estimate recalibrated** against a measured run (adds a per-cell overhead term; ~30
  min @ 8 workers for a 9,408-region world now matches reality, was ~2x optimistic).
- **Prefetch now counts toward the elapsed timer** with a "prefetching OSM / terrain..." indicator,
  so the OSM + terrain warm-up time is visible instead of looking like a stall.
- **Per-child env tuning:** `RAYON_NUM_THREADS = max(threads-floor, floor(cores x cpu% ) / workers)`
  so N parallel cells don't oversubscribe; `ARNIS_STREAM_TO_DISK=1` for big cells.

### Performance

- **Per-spawn startup walk removed.** Arnis ran `cleanup_old_cached_tiles()` synchronously on every
  process spawn, walking the entire elevation cache (541k files): ~17.7s cold / ~1.1s warm per
  cell, deleting nothing. Now skipped in Meld tile-mode, throttled to once/day, and backgrounded.
- **AWS bilinear elevation resample parallelized** (`par_iter_mut`) - it was single-threaded and was
  the reason big 16x16 cells sat at ~15% CPU. Output byte-identical.
- **mimalloc** global allocator (~30% lower peak RSS at 1 thread) + an **i32 corner-sum overflow**
  crash fix that triggered on far-from-origin master-origin coordinates.

### Fixed

- **Floating vegetation over water/roads** on big/streamed exports - the cleanup ran post-merge,
  after stream-to-disk had already evicted regions; now runs per-tile.
- **Duplicate banners / signs / chests** on the parallel path (tiles overlap at edges) - block
  entities are now deduped by coordinate on the Java write path.
- **Worker-thread panic** on the parallel path: our fork has u16 block IDs (Meld blocks at 256+) but
  upstream's palette array assumed u8 (256 slots) - sized to a `BLOCK_ID_CEILING`.
- **Editing the plan during a running generation** desynced the worker pool (could re-add a deleted
  cell or strand planned cells) - all cell + plan-edit routes now refuse while a run/prefetch is
  active.
- Invalid cell keys (`NaN`, floats) rejected before they poison the grid; retry-ring ghosts pruned
  each poll; `MELD_CACHE_DIR` normalized (quotes / relative path); stale run-phase reset on switch.

[1.1.0]: https://github.com/Teddy563/meld/releases/tag/v1.1.0

## [1.0.0] - 2026-06-13

First public release. One origin, one coordinate convention, parallel cells, and a region perfect
merge, all driven from a single Flask plus Leaflet app.

### Added

- **Tiling engine.** Split any OpenStreetMap selection into region aligned cells anchored to one
  project origin, so every seam lands on a Minecraft region boundary.
- **Region perfect merge.** Strip each cell to its canonical regions and write them into a master
  world with a drift guard, for an about 99 percent seamless surface with no height cliffs.
- **Shared OSM prefetch.** Download the selection's OSM data once and feed it to every cell, so
  parallel runs never hit the Overpass rate limit. Adaptive top down chunking with quadrant splits
  on failure.
- **Custom Arnis fork.** Ship a fork of Arnis with a `--download-only` OSM mode and tile invariant
  rendering, so neighbouring cells agree on terrain height and scatter.
- **One global elevation lock.** A single elevation range plus a tile invariant seed across the
  whole world, surveyed automatically (Pillow) or set by hand.
- **Bounded parallel workers.** A worker pool with a hard cap of 16, default 4, that builds many
  Arnis instances at once.
- **Recommend.** One click probe of CPU, RAM, and save disk write speed that suggests a cell size
  and worker count for your machine.
- **Baked chunk lighting.** Distant chunks render lit in Distant Horizons and Voxy without flying
  the world first.
- **Resume, retry, multi world.** Re-run only unfinished cells, regenerate a single cell by
  clicking it, and keep many worlds in your saves folder.
- **Static site.** Marketing site with docs, changelog, a Meld vs Arnis benchmark page, and an
  interactive simulated demo of the app.

### Benchmark

- Built a 24576 x 24576 block world (2304 regions, 48 x 48) in 7 minutes 39 seconds on one PC
  (Intel Core Ultra 9 275HX, 32 GB, NVMe SSD), about 23 times the throughput of a single Arnis run
  over the same area.

[1.0.0]: https://github.com/Teddy563/meld/releases/tag/v1.0.0
