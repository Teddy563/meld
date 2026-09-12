# Meld 1.9.9

*(ships with the arnis fork 3.2.0)*

Meld drives the generator's 3.2.0 options — Moon and Mars, Voxy LOD pregeneration, and the
facade and Overture options for whenever a generator that has them is installed.

Nothing changes unless you ask for it. The new options live behind a **Classic / Enhanced**
switch that starts on Classic, and in Classic none of them are sent no matter what they are
set to, so a project renders the command line it rendered before.

---

## Options that only appear when they work

Every new option is gated on what the deployed generator advertises in its own `--help`,
not on a version number — a locally built or side-loaded binary does not report a version
honestly enough to branch on. A row hides itself when the generator does not know the flag,
and the whole drawer hides when it knows none of them.

So one Meld build drives both generations: against the 3.1.8 fork every probe answers no,
nothing is emitted, and the command line is byte-identical to 1.9.8's.

Against the 3.2.0 fork as released, two of the ten light up — **Voxy LOD** and
**celestial body**. The other eight (Overture transport, preset and Mapillary facades and
their mode, detail and resolution settings) are ready for a generator that has them; the
fork deferred those to 3.3.0, so their rows stay hidden until one is installed.

**`meld --arnis-caps`** prints exactly this: the generator Meld resolved, the version it
reports, and which of the ten options it accepts. It is the direct answer to "why does this
toggle do nothing", and it works while a render is running.

```
$ meld --arnis-caps
arnis       ...\arnis.exe
version     3.2.0
  yes  --voxy-lod
  yes  --body
   no  --overture-source
   ...
2/10 of the 3.2.0 options are available.
```

## Moon and Mars

Pick Moon or Mars under **Generator options · 3.2.0** and the cell renders NASA's own
elevation at that body's fixed scale. They carry no map data, so every object option —
buildings, roads, trees, caves, props — is ignored for them, by the generator, deliberately.

## Voxy LOD pregeneration

Builds the [Voxy](https://modrinth.com/mod/voxy) mod's LOD cache while the world is
generated, so it renders to the horizon the first time you join instead of needing
`/voxy import current`. It forces lighting to be baked (unlit LOD terrain renders black)
and costs extra time and disk.

**One cache per world, so Meld builds it only for a single-cell render.** A multi-cell
render generates each cell into its own world and merges the region files into the master;
the cache is a single database keyed on the world seed, not a set of per-region files, so
N cells produce N caches that cannot be combined. Asked for on a multi-cell run it would
cost time and disk on every cell for something that is thrown away with the cell folder, so
Meld withholds the flag there and says so once in the log. On a single-cell render the
cache is carried into the master world with the regions.

*(If you have been running 1.9.9 pre-release builds: this is the one real bug found in
flight-checking the release. Before it, the cache was generated and then deleted with the
cell folder, and the toggle did nothing but cost time.)*

## The Mapillary token is treated as a credential

Facades from street-level photography need a free Mapillary API token. It is passed to the
generator **through its environment**, never on the command line, because argv is readable
by other processes on the machine. It is also stripped from shared presets and from the
`meld-world.json` sidecar written into world folders — both things people hand to others.

## Smaller things

- **`meld --print-arnis-cmd`** prints the exact command line one cell of the current
  project will run, built through the same builder the render uses, so it cannot disagree
  with what actually runs.
- **"Show cells on the map" is now an eye on the map**, under the paint triangle: open when
  the cells are drawn, struck through when they are hidden. It was a checkbox under the
  Generate buttons, where an open dropdown drew straight over it.
- A duplicate capability probe on the server was removed; the drawer and the CLI report now
  read the same one.

## Upgrading

Drop the new generator next to Meld and nothing else changes. Keep the old one and nothing
changes either — the new rows simply do not appear.

Projects from 1.9.8 open unchanged: every new setting defaults to off, and the generation
mode defaults to Classic.

## Verification

- 591 tests pass, including 46 that pin exactly which flags are emitted for a given
  settings/binary pair, and 3 new ones pinning that the LOD cache survives a merge.
- `--arnis-caps` verified against both a 3.1.8 binary (0/10) and the 3.2.0 fork (2/10).
