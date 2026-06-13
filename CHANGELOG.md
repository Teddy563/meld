# Changelog

All notable changes to Meld are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Meld follows
[Semantic Versioning](https://semver.org).

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
