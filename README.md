<div align="center">

<img src="assets/banner.png" alt="Meld, turn the real world into one seamless Minecraft world" width="100%">

Turn an OpenStreetMap selection into one seamless Minecraft world. Meld tiles the area, builds
every tile in parallel, and melds them with no height cliffs and no seams. From a city block to a
whole continent.

&nbsp;![version](https://img.shields.io/badge/version-1.0.0-blue)
&nbsp;![Minecraft](https://img.shields.io/badge/Minecraft%20Java-1.21%2B-brightgreen)
&nbsp;![Python](https://img.shields.io/badge/Python-3.10%2B-yellow)
&nbsp;![built on](https://img.shields.io/badge/built%20on-Arnis%20fork-orange)

**Windows · macOS · Linux**

</div>

Meld is a real world Minecraft world generator. You draw an area on a map, pick a cell size, and
Meld splits the selection into region aligned tiles, generates each tile in parallel with a custom
[Arnis fork](https://github.com/Teddy563/arnis), and merges every tile into one master world. Every
seam lands on a Minecraft region boundary, so the join is exact and the surface is about 99 percent
seamless. Cities, regions, whole continents.

> Meld is an orchestrator, not a new generator. It drives [Arnis](https://github.com/louis-e/arnis)
> to build the blocks, then handles the hard part: tiling, a shared OSM fetch, one global elevation
> lock, and a region perfect merge. The win is scale on a single PC.

On one machine (Intel Core Ultra 9 275HX, 32 GB, NVMe SSD) Meld built a **24576 x 24576** block
world, 36 times the area of a single 4096 Arnis run, in **7 minutes 39 seconds**, about **23 times
the throughput**. See the numbers in [Meld vs Arnis](https://meldmc.com/vs-arnismc).

**New here?** Read the [docs](https://meldmc.com/docs) or try the
[live preview](https://meldmc.com/demo), an interactive, simulated copy of the app.

---

## What you get

| Feature | What it does |
|---|---|
| **Region perfect merge** | Every cell boundary is snapped to a Minecraft region edge, so tiles join exactly. About 99 percent seamless surface, no height cliffs. |
| **Custom Arnis fork** | Meld ships a fork of Arnis with a `--download-only` OSM mode and tile invariant rendering, so neighbouring cells agree on terrain and scatter. |
| **Shared OSM prefetch** | The selection's OpenStreetMap data is downloaded once and reused by every cell, so parallel runs never hit the Overpass rate limit. |
| **Parallel workers** | Builds many Arnis instances at once. Default 4, up to 16, with a one click **Recommend** that tunes cell size and workers to your CPU, RAM, and save disk. |
| **One elevation lock** | A single global elevation range plus a tile invariant seed, so terrain height and building or scatter choices match on both sides of every border. |
| **LOD ready** | Chunk lighting is baked in, so distant chunks render lit in Distant Horizons and Voxy without flying the whole world first. |
| **Resume and retry** | Re-run only unfinished cells after a stop, click one cell to regenerate it, and keep many worlds in your saves folder. |

---

## Quickstart

```bash
git clone https://github.com/Teddy563/meld
cd meld
pip install -r requirements.txt
python server.py        # then open http://127.0.0.1:5630
```

Get the **generator**: use the bundled `arnis.exe`, or download the latest from the
[Teddy563/arnis releases](https://github.com/Teddy563/arnis/releases) and drop the binary next to
`server.py`. On macOS or Linux, build the fork (`cargo build --release`) and place the `arnis`
binary there instead. Pillow is optional, only the automatic elevation survey needs it.

Then, in the app: draw an area, set the cell size in Settings, and hit **Generate and merge**.

> Windows: double click `start.bat`. macOS or Linux: run `./start.sh`. Both just launch
> `python server.py`. The port is `5630`, or set `PORT` to override it.

---

## How it works

1. **Origin.** Anchor a project origin, one lat/lon snapped to a region corner. Every cell is
   measured from it, so the whole world shares one coordinate convention.
2. **Survey.** Lock one global elevation range and seed for the area, so heights and choices are
   consistent across every tile.
3. **Plan.** Split the selection into a grid of region aligned cells at your chosen cell size.
4. **Prefetch.** Download the OSM data once for the whole selection, then feed it to every cell.
5. **Generate.** Build the cells in parallel with the Arnis fork, bounded by a worker pool.
6. **Merge.** Strip each cell to its canonical regions and write them into the master world, with a
   drift guard so nothing overlaps.

---

## Project layout

```
server.py            Flask orchestrator + the HTTP API
src/
  coords.py          the coordinate convention (origin anchored)
  grid.py            selection bbox to region aligned cell list
  prefetch.py        download the OSM once and share it to every cell
  arnis_cmd.py       build the Arnis argv, run it, find the world dir
  merge.py           canonical region strip + drift guard
  survey.py          elevation min/max (Pillow optional)
  workers.py         bounded parallel worker pool
  project.py         project.json + grid.json state
  level_dat.py       master level.dat handling
  constants.py       shared defaults
web/index.html       the Leaflet app UI served by Flask
tests/               coordinate round trip tests
```

---

## Caveats

- **The save phase is the bottleneck**, not the CPU. Each cell writes its region files in one
  burst, so very large cells or very high worker counts can saturate a slow disk. Meld defaults to
  cell size 4 and a low worker count, and **Recommend** tunes both to your machine.
- **One Arnis binary required.** Meld will not generate without `arnis.exe` (or `arnis`) next to
  `server.py`. The app says so on startup if it is missing.

---

## Releases

Versioned with [SemVer](https://semver.org). See [CHANGELOG.md](CHANGELOG.md) for the full history
and [RELEASE-NOTES.md](RELEASE-NOTES.md) for the highlights of each release. Tag a release as
`vX.Y.Z`; the matching CHANGELOG section is the release body.

---

## Credits

Built on the open source [Arnis](https://github.com/louis-e/arnis) generator by louis-e. Meld drives
a [custom Arnis fork](https://github.com/Teddy563/arnis) for the shared OSM prefetch and tile
invariant rendering that make the tiles line up. Respect the upstream Arnis license for the
generator.

Not affiliated with Mojang AB or Minecraft.
