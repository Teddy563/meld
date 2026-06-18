# Meld docs

Meld turns one map selection into one seamless Minecraft world. It tiles a big area into a grid, builds the tiles with many generator runs at once, and melds them into a single world with the same terrain height and building style everywhere.

This hub links to the how-it-works guides. For what changed in the latest release, see [What's new in 1.2.0](./whats-new-1.2.0.mdx); the full per-release history lives in [CHANGELOG.md](../CHANGELOG.md).

## How a build flows, end to end

You draw an area on the map. Meld does the rest in a few clear steps:

1. It picks an **origin**, one fixed anchor that every block coordinate is measured from.
2. It runs a quick **elevation survey** and locks one height range, so the land lines up everywhere.
3. It splits the area into **cells** that fall on region file boundaries, so seams land in clean spots.
4. It downloads the area's **map data once** and shares it to every cell, so nothing gets rate limited.
5. It builds many cells **in parallel** with the Arnis fork, all using the same origin, seed, and height lock.
6. It **merges** each finished cell into one master world, keeping only the part each cell owns.
7. A **drift guard** refuses any cell that does not line up, so a built world is never corrupted.

The result is one Minecraft world with no cliffs and no seams.

## The guides

Each page explains the why, the mechanism, and how to use it in the app.

- [How it works](./how-it-works.mdx). The full pipeline: origin, the elevation lock, region aligned cells, map prefetch, parallel generation, the region perfect merge, and the drift guard. The core mental model.
- [Elevation](./elevation.mdx). The whole elevation system: the global height lock, region data packs you download once, selectable detail with Auto, the no-data hole repair, and the height preview.
- [Parallel generation](./parallel-generation.mdx). Cells and the worker pool, the CPU budget and staggered starts, stream to disk for huge cells, auto retry, spiral build order, and why small cells can finish faster.
- [The Arnis fork](./the-arnis-fork.mdx). What the custom Arnis fork changes and the knobs you get: no buildings, road detail modes, automatic flat bridges, seam free rendering, offline elevation, a custom Overpass URL, and how to run map data offline or self-hosted.

## Power tools (experimental)

- [Headless tools](../experimental/README.md). Run Meld work from the command line with no web UI: repair missing regions in a world, re-run a whole world to roll out a generator fix, or scan for residual water artifacts. They write straight to disk, so close Minecraft first.

## What changed between releases

- [What's new in Meld 1.2.0](./whats-new-1.2.0.mdx). Offline OSM packs, an OSM cache that reuses tiles, much faster cells, the water-wedge fix, and a remembered selection.
- [What's new in Meld 1.1.0](./whats-new-1.1.0.mdx). The changelog-style summary of what 1.1.0 added and why.
