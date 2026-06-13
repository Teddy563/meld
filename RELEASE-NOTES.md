# Release notes

Short, human highlights for each Meld release. Full detail lives in [CHANGELOG.md](CHANGELOG.md).

## v1.0.0

**Meld turns the real world into one seamless Minecraft world, at scale, on a single PC.**

Draw an area on a map, pick a cell size, and Meld tiles the selection, builds every tile in
parallel with a custom Arnis fork, and merges them into one master world. Every seam lands on a
Minecraft region boundary, so the join is exact and the surface is about 99 percent seamless. No
height cliffs, no Overpass rate limits, no fly the whole world to light it.

Highlights:

- **23 times the throughput.** A 24576 x 24576 block world (36 times the area of a single Arnis run)
  built in 7 minutes 39 seconds on one PC, because cells run in parallel instead of one step at a
  time.
- **Shared OSM prefetch.** The area is downloaded once and reused by every cell, so parallel runs
  never trip the Overpass rate limit.
- **One elevation lock and a tile invariant seed**, so terrain height and building or scatter
  choices match on both sides of every border.
- **Recommend** tunes cell size and workers to your CPU, RAM, and save disk in one click.
- **Resume, retry, and multi world saves**, plus baked chunk lighting for Distant Horizons and Voxy.

Get it: clone [Teddy563/meld](https://github.com/Teddy563/meld), `pip install -r requirements.txt`,
`python server.py`, and open `http://127.0.0.1:5630`. The generator is the custom
[Arnis fork](https://github.com/Teddy563/arnis), shipped as `arnis.exe`.

Built on the open source [Arnis](https://github.com/louis-e/arnis) generator by louis-e. Not
affiliated with Mojang AB or Minecraft.
