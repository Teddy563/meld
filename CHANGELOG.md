# Changelog

All notable changes to Meld are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Meld follows
[Semantic Versioning](https://semver.org).

## [1.9.9] - 2026-09-12

Talks to the arnis 3.2.0 fork. Every option this release adds is gated on what the
generator itself advertises, so the same Meld build drives the 3.1.8 fork and a 3.2.0
one, and against 3.1.8 the command line is byte-identical to 1.9.8's.

### Added
- **Generator 3.2.0 options**, in their own settings drawer: Voxy LOD pregeneration,
  Moon and Mars worlds, the Overture vector-tile transport, preset building facades,
  and the Mapillary street-photograph facades with their mode, detail and resolution.
  Each row hides itself when the deployed generator does not advertise the matching
  flag, and the drawer hides entirely when none of them are available - a control that
  silently does nothing is worse than no control.
- **`meld --arnis-caps`**: which 3.2.0 options the resolved generator accepts, and the
  version it reports. The direct answer to "why does this toggle do nothing".
- **`meld --print-arnis-cmd`**: the exact command line one cell of the current project
  runs, built through the same builder the render uses, so it cannot disagree with what
  actually runs.
- **`/api/arnis-caps`**, the endpoint behind both the drawer and the CLI report.

### Security
- The Mapillary API token is passed to the generator in its environment, never on the
  command line, because argv is readable by other processes on the machine. It is also
  stripped from shared presets and from the `meld-world.json` sidecar written into the
  world folder - both are things people hand to someone else.

### Changed
- `server.py` carried a second, never-called copy of the generator capability probe with
  its own cache. Removed; there is now one probe and one cache, so the settings UI and
  the command line cannot disagree about what the binary supports after an update.

## [1.9.8] - 2026-08-27

Bundles arnis fork 3.1.8. Generation is much faster and tunes itself, a long
list of scheduling bugs is fixed, and two opt-in generator options reshape
river beds and road surfaces. Default output is unchanged from 1.9.7, verified
by hashing generated blocks on every build.

### Added
- **Meld picks its own worker count.** One generator process cannot fill a
  modern CPU - measured, 10 of 24 cores - so Meld starts low, times every
  finished cell, adds workers while that keeps improving throughput, backs off
  when it stops, and remembers the answer per project and scale. It never
  exceeds the worker ceiling, CPU percentage and free-memory floor you set.
  Modes: Off (your numbers, used as typed), Advise (logs what Auto would pick,
  changes nothing), Auto. `MELD_GOVERNOR=off` overrides everything.
- **OSM tile sidecars**, on by default: cached map tiles carry a pre-parsed
  companion so repeat renders skip most decoding, at roughly two thirds more
  OSM cache on disk. Turn off on tight disks; it only touches the cache.
- **Smooth river beds** and **road grading**, both off by default and both
  deliberate changes to the world, so a project regenerated with either one on
  will not match cells rendered earlier. Rivers get a rounded channel scaled to
  their width, with lakes and oceans left untouched; roads get one
  slope-limited height profile each, so a carriageway ramps instead of stepping
  on terrain contours.

### Fixed
- **Stop now actually stops.** The flag that halts a run was never set, so
  cancelled cells matched the retry filter and were resubmitted twice. Prefetch
  had the mirror problem: it accepted a stop signal that was never passed, so a
  stopped run could be revived by its own prefetcher.
- **A project switch mid-run could merge cells into the wrong world.** The
  destination path was re-resolved while the run was in flight; it is now frozen
  at run start. A second path through the export hook reopened half the same
  race and is closed too.
- **The first run of a new project was much slower than later ones**, sitting at
  about a third of the machine: startup stagger armed once per worker thread
  instead of once per run, and the worker search could not recognise a good step
  early. Cold runs went from 173.9 s to 144.8 s, CPU from 60% to 79%.
- **The worker search could stop for the wrong reason.** Throughput was measured
  over cells finished rather than over a window of time, so a burst read as a
  rate spike and a lull as a collapse; the idle guard could only ever hasten a
  stop, never keep the search alive.
- **The memory guard was four times too pessimistic** while streaming to disk,
  holding back workers there was room for.
- **The CPU budget had two different defaults** (100% and 90%) depending on the
  path taken; it is 90% everywhere.
- **GPU acceleration no longer travels in shared presets** - it describes a
  machine, not a world.
- **Generation no longer depends on machine load**: the generator's flood-fill
  limiter varied with system load, so identical input could differ on a busy
  machine. Meld now sets it explicitly on every child process.
- **Caves work at any world height** (generator fix). Worlds taller or deeper
  than vanilla broke at both limits: above roughly y=256 every underground block
  became air, and below y=-64 no caves generated at all. Both now follow the
  world's own floor and ceiling; standard-height worlds are unaffected, bit for
  bit.

### Changed
- Generation is **1.7x** a stock 1.9.7 install, **1.3x** a hand-tuned one, and
  **1.8x** what the generator manages alone on the same ground: 230.3 s to
  135.8 s on 81 cells of central Bucharest at 1:1, 338 to 573 region files per
  minute, with peak memory down from 82-93% to 52-56%.
- Cells no longer write the seam ring the merge deletes moments later (-12% per
  cell), and per-worker memory is capped and constant in area, so city-sized
  batches no longer climb toward swap.

## [1.9.7] - 2026-08-24

Bundles arnis fork 3.1.7. No Meld-side changes; everything here comes from the
generator.

### Fixed
- **Riverbeds and underwater slopes are deeper and properly shaped.** The depth
  curve was rounded down at every column, which shallowed every bed and widened
  each terrace: a 20-block-wide river came out as a flat shelf and two steps
  rather than a channel.
- **A cell no longer treats its own edge as a coastline**, which used to carve
  one bank of a channel crossing between cells shallower than the other.

### Changed
- **Land cover in multi-cell worlds is sampled in one shared frame**, so
  neighbouring cells cannot disagree about which block a boundary belongs to,
  and land cover no longer drifts slightly against the features sitting on it.

## [1.9.6] - 2026-08-24

Bundles arnis fork 3.1.6 with the building and shoreline upgrades: per type
height inference, Overture measured heights, era styled walls and windows,
straightened coasts, and the shore band land override that keeps quays and
promenades above water. Details in the arnis release notes.

### Added
- **Bake scope choice** in the OSM data pack card: bake only the selection's
  missing tiles (the old behaviour), re-bake all selection tiles, or bake the
  entire .pbf coverage. The whole file is read either way, so baking the whole
  country costs the same read as one selection and every later selection is
  already local. Whole file scope needs no drawn selection.

### Changed
- **The bake progress bar shows the source read.** Both .pbf passes run before
  any tile resolves, so the bar used to sit at 0/N looking hung for the whole
  read. It now reports the file, the pass, and the exact element count.
- **The bake time estimate charges both read passes.** A 20 tile Romania bake
  used to quote under a minute and take several; the fixed full file read
  dominates small bakes and is billed properly now.
- **No automatic world backup before the first server start.** The setting
  defaulted on for every project regardless of the toggle, so a 1 GB world
  zipped a 1 GB archive right when you pressed Start. Backups are the Backup
  world button now, on demand only. The project master world is always the
  untouched source either way.
- **Show cells on the map** moved to the bottom of the Generate card and
  collapses with it.

## [1.9.5] - 2026-08-23

The GPU/void/naming feature branch, merged on top of the triage release. The
generator side ships in fork 3.1.4; this release wires it into Meld and teaches
the worker pool to size itself from measurements instead of a stored number.
Full branch history and measurements in `docs/BRANCH-STATUS.md`.

### Added
- **A GPU acceleration toggle.** `gpu_accel` in the Region format drawer: off,
  auto, dedicated, integrated. Passed to the fork as `--gpu`; a missing or
  mismatched adapter falls back to CPU with a log line, and off stays the
  default. Measured on a 224-region 1:1 cell: 64.7 → 54.7 s wall (1.18×),
  852 → 623 core-seconds (1.37× fleet throughput), and the 5080 and the iGPU
  disagree on one block in 176 million.
- **The worker governor.** Each finished cell reports what it actually used —
  CPU cores from process time, RAM, and its share of the one shared GPU — and
  the pool is sized by three budgets, tightest wins: CPU at the existing target
  percentage, RAM at 95% of free, GPU at 95% of the adapter. Advisory in the
  log by default; the `worker_autoscale` toggle lets it act, stepping by at
  most 2 workers and re-measuring before the next move. Exists because a 1:20
  cell keeps ~1 core busy and a 1:1 cell ~12 — no stored `max_workers` is
  right for both.

### Changed
- The fork's flush pool scales with each worker's thread budget
  (`max(2, min(6, rayon_threads // 2))`) instead of a flat per-process
  default — 8 workers used to spawn 48 flush threads into 24 cores.

### Fixed
- World meta no longer stamps `meld_version` "1.7.0". The report builder was
  fixed in 1.9.4; the world-meta dict had the same hardcode.

## [1.9.4] - 2026-08-23

Community bug reports from 22–23 August, root-caused against 19,018 cell logs and
2,428 region files rather than by reading the code alone. Full write-up, including
what the measurements overturned, in `docs/TRIAGE-2026-08.md`.

### Fixed
- **RAM is reported the way Windows reports it.** A 2×32 GB kit read as "68.3 GB"
  total and "2×34 GB" per stick, because byte counts were divided by 1e9 and
  labelled GB while other places in the same file used 1024³. Every memory figure
  goes through one helper now. Disk stays decimal on purpose — drive makers really
  do sell decimal GB — and both stats say which convention they use.
- **The two size estimates no longer disagree.** The BUILD panel passed elevation
  to the size model and the run path did not, so the same project could be quoted
  22.4 GB and 2.2 GB in the same second, from the same function.
- **Build-time estimates are formatted as estimates again.** Two top-level
  `fmtDur` declarations in one script block meant the export-progress formatter
  silently won, rendering guesses as "13m 54s".
- **The yellow selection box can be removed.** There was no reachable way to clear
  it: the map's own trash is disabled, the Plan card's trash clears cells, and the
  one code path that did clear it sits behind a button that is `display:none`.
  There is now a labelled button, plus Esc and Delete, and a toggle to hide the
  cell overlay so you can see the ground you are selecting.
- **Quit no longer looks hung.** Quitting during a `.pbf` bake tore down the tray
  icon and window immediately while the interpreter stayed blocked for the rest of
  the bake — 346 s for one country file. People reasonably ended the task, and
  *that* is what left bake workers orphaned. Quit now runs with the icon still up
  and says what it is doing. The bake pool is also adopted into the job object it
  had been escaping, shutdown is serialised, and the self-update successor is no
  longer killed by the job of the instance that spawned it.
- **One cell no longer bakes two countries.** `.pbf` files were selected by header
  bounding box, and a country's bounding rectangle overlaps its neighbours' — so
  rendering one cell in Germany streamed France as well, for about 13.7 minutes.
  Selection now confirms against Geofabrik's real cutting polygon, with an exact
  test rather than a sampled one, and anything unrecognisable is still read.
- **Extending build height no longer inflates the estimate.** The size model
  scaled linearly with declared height, but the generator emits sections from the
  world floor up to the highest one holding content — so raising the ceiling costs
  nothing. Matched real builds differing only in the ceiling are byte-identical.
  The model is now additive in floor depth, with every constant measured.
- **A calibrated project keeps responding to its settings.** Once a project had
  built anything, the measured MB/region was returned unchanged for ever, so
  turning caves or baked lighting on moved nothing.
- **The benchmark report says which Meld actually ran.** `meld_version` was
  hardcoded to `"1.7.0"` in the report builder, so every report on disk claims a
  version six releases old — including the ones used to calibrate the estimates.
  It reads `src.__version__` now.

### Added
- A warning before Generate when the settings mean the world will look empty —
  buildings off, or a scale small enough that houses become single blocks. Neither
  default changed; both are deliberate, they were just invisible until the build
  finished.
- Overture failures are counted per run and surfaced in the log and the report.
  A cell that loses Overture still finishes, but the world is thinner and nothing
  said so.
- A warning below cell size 4, which discards most of the chunks it generates.
- Run reports record the settings the size and time models key on, so finished
  runs can calibrate them. None of the reports on disk recorded caves or the
  height flags, which is why those constants stayed guesses.

## [1.9.3] - 2026-08-21

Worlds can now be generated straight into Leaf's B_Linear region format, skipping
the conversion pass entirely. Off by default and marked experimental.

### Added
- **Region format, written during generation** (Settings, the drawer above Export
  & compression). Anvil `.mca` stays the default and is what everything reads.
  Choosing B_Linear has the fork write `r.X.Z.b_linear` itself, so there is no
  `.mca` to convert afterwards and only one world is ever written to disk -
  measured **3.7x smaller for a dense city** and 3.9x on terrain-with-caves
  content, with peak disk for the whole flow dropping about 4.7x because the two
  worlds never coexist. Needs **Arnis fork 3.1.2 or newer**.

  The cost is stated on the control itself: the world opens only in **Leaf
  1.21.11 (June 2026 builds) or newer, or 26.x**, never in Paper, older Leaf or
  the vanilla client; the map item is unavailable, since its renderer reads Anvil
  regions; and no `.mca` original is kept. Server setup already pins
  `region-format: B_LINEAR` for such a world, so the world folder and the server
  config still cannot disagree.

### Changed
- **The sidebar wordmark is the site's animated MELD title art.** The letters are
  placed one at a time like blocks, ARNIS WORLDS rises under them, and the
  finished composition takes over. Plain CSS keyframes, no animation library. It
  links to meldmc.com as it did before.

### Fixed
- **Merging a cell into a world of the other region format is refused up front.**
  The two containers use different file names, so a mismatch would never surface
  as a collision - it would quietly leave half a world no server could open. The
  same check rejects a world that somehow holds both.
- **The final missing-region scan understands `.b_linear`.** A 142-byte file is
  the chunkless equivalent of an 8192-byte `.mca`, so genuinely empty regions are
  still detected as holes to retry rather than counted as present.
- **Export and the map item are skipped for natively-generated B_Linear worlds**
  instead of running against a world that has nothing left to convert, and the
  export controls say so rather than sitting there enabled.
- **Region format now appears above Export & compression**, where it belongs. The
  Generate step is assembled at runtime and the drawer was being left behind by
  that pass, so it rendered below Export no matter its position in the markup.

## [1.9.2] - 2026-08-19

Windows bundling fix. The 1.8.5 through 1.9.1 Windows archives all carried
**Arnis fork 3.0.6** no matter what the fork's latest release was; every other
platform got the right binary. Reported on Discord - the version in the
generator's own footer is what gave it away.

### Fixed
- **The Windows archive bundles the fork's latest release again** (3.1.1 as of
  this release). The repo used to track a "live" `arnis.exe` for local
  development, and the build's offline shortcut - a local copy next to the repo
  wins - meant CI's checkout of that binary beat the download of the fork's
  latest release. That tracked copy was frozen at 3.0.6 on 2026-08-10, so every
  Windows zip since shipped it. The binary is untracked and ignored now, and a
  CI build refuses the local-copy shortcut altogether, so it can only bundle
  what the fork actually released (`MELD_ARNIS_LOCAL=1` forces the shortcut
  back on for a deliberate offline build).

### Added
- **Stable download aliases.** Each release now also uploads its archives under
  fixed names (`Meld-win-x64.zip` and friends), so
  `releases/latest/download/Meld-win-x64.zip` always fetches the newest release
  without knowing its version number.

## [1.9.1] - 2026-08-18

Compatibility release for **Arnis fork 3.1.0**. No new features; this is the
Meld half of that upgrade.

### Fixed
- **Scale is now validated before a render starts.** The generator's 3.1.0
  rejects `--scale` outside 0.01 to 4.0 at the parser, so a value Meld would
  previously have passed through now fails the cell outright with a usage
  error - thousands of times over, on a country render. Meld clamps instead,
  on write in the settings API so the stored value is always usable and the UI
  shows what will actually run, and again when building the command line as a
  last line of defence for a project or preset written by an older Meld.

  Non-finite values fall back to 1.0 rather than clamping. NaN needs the
  special case anyway - it fails every comparison, so a plain min/max would
  pass it straight through - and clamping the infinities is actively harmful:
  positive infinity would land on the maximum and quietly start a 4:1 render of
  the entire selection.

- **The Scale box gains an upper bound and says when a value is out of range**,
  instead of accepting it silently and failing later.

### Changed
- **Image signage is explicitly held off.** Arnis 3.1.0 added `--signage`,
  defaulting to on. It draws street plates, transit signs and billboards as map
  items in item frames, and the map payloads live in the world's `data/`
  directory - which Meld's merge does not carry across cells, since it copies
  `region/`, `poi/`, `entities/`, `datapacks/` and `level.dat`. Every cell would
  restart map ids at 0 and the merge would drop them all, leaving blank frames
  throughout the world: strictly worse than the feature being off.

  Meld therefore states the value rather than inheriting a default it does not
  control. The flag is only emitted to a generator that advertises it, so
  nothing changes for the current fork build, which does not carry it.

## [1.9.0] "Unbound" - 2026-08-16

The milestone release. Meld is a standalone desktop app: download, run,
generate. Unbound names the whole desktop arc, previewed and hardened across
1.8.4 to 1.8.9, now shipped as one release with the tuned presets in the box.

### Added

- **The three bundled presets are now the tuned ones**, authored by Teddy563
  in Meld 1.9.0: Default (the tuned look at real 1:1 scale), Scaled 1:10
  (city-sized builds, compact roads, props off) and Extended height (the same
  look with the extended build height on and no vertical stretch; the old
  1.5x terrain exaggeration is gone from the preset).

### Fixed

- **Presets can now carry the Overture-buildings and offline-elevation
  toggles.** Both settings were read by the generator launcher but missing
  from the defaults table, so applying a preset silently dropped them.
- **A machine-observed statistic (`mb_per_region_observed`) no longer rides
  along in saved presets.** It describes the sender's renders, not the world.

## [1.8.9] - 2026-08-16

The first-day report on the 1.8.8 build, fixed at the root, plus the polish
pass from using it.

### Fixed

- **Half the interface came up dead.** One hoisting bug in the slider styling
  threw before most of the page's script ran: no CPU/RAM gauges, empty
  settings, selections not splitting into cells, cards stuck open. One line;
  everything after it never wired. Headless-browser console checking is now a
  release gate, because every static check passed while this shipped.

### Added

- **Presets and the status bar are one click from the left rail.** Presets sits
  under Projects; the status bar can be shown or hidden without hunting for the
  tray icon.
- **The shipped preset starters are editable files** in the presets folder -
  tune them, share them, delete one to reset it to pristine.
- **The `.pbf` and presets folders exist from first launch**, the bake folder
  box shows its real default path, and PBF downloads appear in the cache card
  with everything else.
- **Smart data downloads price themselves.** The by-country set is recommended
  first with its combined size; a continent-sized file is offered last with its
  true download size, the memory baking it needs on this machine, and a warning
  when that will not fit.

### Changed

- Generate is the one gold button on the page. Test build and Trim ocean live
  with the other plan tools in Edit & retry. Drawer hover marks the whole row.
  Card titles breathe.
- **arnis 3.0.10**: the stadium model no longer lands on football fields, rail
  tunnels and building parts from the upstream ports, and three upstream
  scanline-fill correctness fixes (the other three upstream commits were
  verified as fixes this fork shipped first, or rejected with reasons).

## [1.8.8] - unreleased

The release-day reports, fixed at the root; a cleaner face; and settings that
travel.

### Fixed

- **Pressing Bake no longer opens extra Meld tabs.** The parallel bake spawns
  worker processes, and in a packaged build each worker accidentally booted as a
  full second Meld — which found the app already running, opened a browser tab at
  it and died. Two workers, two tabs, exactly as screenshotted. The same fault
  meant **every bake in every packaged build has secretly run single-file** since
  the first exe; parallel baking now actually works there, for the first time.
- **The bake's time estimate is restated when it loses its workers.** A predicted
  29 minutes against an actual 50 was the difference between "slower than hoped"
  and "looks stuck".
- **"Refresh sizes" no longer hangs the cache card.** Counting a 100 GB cache
  happened inside the request, while a bake was writing to it. It now counts in
  the background and the card says "counting…" instead of freezing on "…".

### Added

- **"Get the data for this region."** Meld reads the selection, asks Geofabrik's
  own index which extract fits it — the smallest one that contains it, or the
  set of neighbouring countries when the selection crosses a border — and
  downloads into the default `.pbf` folder with progress and a Stop. No more
  choosing between `europe-latest` and a country file by hand: that choice being
  gotten wrong is what produced the 75 GB / 190 GB-pagefile report.
- **The bake's disk estimate corrects itself** from the tiles it has actually
  written, and the scan warns when an extract is more than six months old.
- **Presets.** A preset is one JSON file that carries the render-defining
  settings — scale, terrain, trees, climate, caves, props, textures — and none
  of the machine-specific ones, so a file from a big desktop is safe on a
  laptop and never leaks a private Overpass mirror. Save yours, export it, send
  it to someone, import theirs. Selection travels only when both sides opt in.
  Three starters ship built in: Default, Scaled 1:10, Extended height.
- **Test build.** Renders the few cells at the centre of your selection so you
  can check the look before committing to the full run.

### Changed

- **One "Props & 3D models" drawer** replaces the two controls whose difference
  nobody could explain, with schematic props and downloaded 3D models as
  labelled groups inside it.
- **The football-field stadium is gone.** The 3D pipeline kept dropping a
  generated stadium model onto ordinary soccer grounds (arnis 3.0.10 removes the
  archetype outright). Pitch lines and goals are untouched.
- **Flat dark theme throughout** — gradients removed, drawer triangles full-size
  with a visible hover cue, the Generate area reordered to match the order you
  actually work in: selection first, then export, then retry tools.

## [1.8.7] - unreleased

Everything here came from the first day of people actually running the packaged
build. Four reports, four bugs, all of them invisible from a source install.

### Fixed

- **Browse was broken in every packaged build, and put the session token on
  screen.** Clicking Browse filled the folder box with
  `Meld is already running: http://127.0.0.1:5630/?t=…` instead of a path.
  Packaged, `sys.executable` is `Meld.exe` rather than a Python interpreter, so
  the picker was accidentally launching a *second Meld*, which reported the first
  one and exited — and its message was read back as the chosen folder. Affected
  the save location, the `.pbf` folder and the import folder. A path is now only
  ever read from a line explicitly marked as one, so no other output can be
  mistaken for it.
- **Offline `.pbf` baking did not exist in the packaged app.** The reader was
  left out of the build, so every file showed *"(no header bbox)"* and the only
  cure was a source install — `pip install` cannot help, because the bundled
  runtime is not the one pip writes to. It ships now. Where it genuinely is not
  available, Meld says so once, plainly, instead of blaming your files.
- **A bake read every `.pbf` in the folder, wherever the region was.** Eight
  continent extracts — 75 GB — to render one US state, when Meld had already
  read each file's bounds and thrown them away. Files that cannot contain your
  region are skipped. A file whose bounds will not parse is still read, because
  guessing wrong there would put silent holes in a world.
- **A bake could not tell it would not fit, and took the machine down finding
  out.** Measured: the OSM index costs about 2.2 GB of memory per GB of `.pbf`,
  per worker. Four workers on continent files asked for far more than the 68 GB
  available, and Windows grew a 190 GB page file trying to serve it. Worker count
  is now fitted to the memory actually free, and a bake that cannot fit is
  refused *with the fix in the message*: use the country or state extract. For
  New Hampshire that is 70 MB against 19 GB — the same world, 275× less memory,
  minutes instead of hours.

### Added

- **A bake tells you what it will cost before it starts.** Memory, disk, time and
  worker count, against what your machine actually has. Baked tiles run about
  15.7 MB each and the disk briefly holds roughly double the final size while
  merging — neither was knowable before. Because a city tile can be 88× a rural
  one, the first figure is a starting point that gets replaced by a real
  projection once enough tiles exist.

## [1.8.6] - unreleased

Three generator settings that existed but had no control, and updates that
actually complete.

### Added

- **Cached elevation only.** Bake a region, then guarantee the render uses the bake
  and nothing else: no cell waits on the tile server, none is rate-limited, and none
  receives a truncated tile — the documented cause of flat terrain seams.

  It does not conjure missing terrain. A tile that was never baked still produces flat
  ground for that cell, exactly as before; what changes is that Meld now flags those
  cells as suspect instead of leaving the failure silent. That visibility is the real
  fix, and the tooltip says so rather than implying the setting is a guarantee.
- **Overpass endpoints.** Comma-separated, tried in order — for pointing Meld at a
  private or self-hosted mirror. The public servers rate-limit hard during a
  country-sized batch, which is what turns a long render into a stalled one.
- **Cell timeout.** Blank keeps the generator's 600 s. For dense cells that keep
  failing at the flood-fill step.
- **Prop scale control**, with a warning. Props are fixed-size builds, so the
  generator skips them below 0.35 while Meld's usual scale is 0.1 — ticking the family
  checkboxes placed nothing at all, and said nothing. The warning now appears the
  moment the project scale falls below the gate.
- **Updates that finish.** A prepared version can be launched from inside Meld: the new
  build starts, this one quits, and the old folder is kept so you can go back to it.
  Removing an old build is a separate, explicit action, and it refuses any folder that
  contains your projects.
- **The generator updates on its own.** arnis is one binary in a folder Meld owns, so a
  generator fix no longer needs a whole new Meld — check and update it from the same
  dialog. Downloads are verified against the release checksum, and a release without
  one is refused rather than trusted.

### Fixed

- **A downloaded generator was always ignored.** The search order put the download
  location last, so a freshly fetched arnis lost to the bundled copy every time and the
  update silently did nothing. It now wins when it is strictly newer — and only then, so
  a binary you dropped in yourself still takes precedence.
- **An updated generator kept being described by the old one.** Both probe caches key on
  the file path, and an update replaces the file at the same path. Beyond a wrong version
  in a message, this gated new flags off the *previous* binary's capabilities: a freshly
  installed generator would keep being told it has no `--overture`, so the Additional
  Buildings checkbox would go on doing nothing until Meld restarted.

## [1.8.5] - unreleased

Fixes what 1.8.4 shipped broken on macOS and Linux, and gives Overture buildings the
off switch they never had.

### Added

- **Additional buildings toggle.** Overture Maps supplies footprints for buildings
  missing from OpenStreetMap, which is what makes a sparsely mapped area look mapped
  at all. They are detected from satellite imagery, so a few land where nothing
  exists. Until now the only way to avoid one was Buildings off, which also deletes
  every real building, wall, mast and pylon. The new checkbox drops only the satellite
  fill. On by default, and greyed out when Buildings is off, since the generator never
  fetches Overture in that case.

  Needs arnis 3.0.8. Meld asks the generator which flags it accepts before passing
  this one, so an older binary keeps working instead of failing every cell on an
  unknown argument.
- **Prop scale control.** Schematic props are fixed-size builds, so the fork skips
  them below `--props-min-scale` (0.35). Meld's default scale is 0.1, so every prop
  family was dropped at the default while the UI showed ten ticked checkboxes and no
  boat, crane or tractor ever appeared. The gate is now Meld's to set.

### Fixed

- **The macOS archives were not applications.** The tarball used a hardcoded member
  name, stripping the `.app` extension, so both 1.8.4 mac downloads extracted to a
  plain folder that Finder drew as a folder and macOS would not launch.
- **The macOS bundle signature was invalid.** The generator was copied in after
  PyInstaller had sealed the bundle, so the signature no longer matched its contents.
  The build now re-seals and verifies, and fails rather than shipping unsealed. The
  missing `.app` extension had been hiding this: fixing that alone would have produced
  *"Meld.app is damaged and can't be opened"*, which has no user-facing bypass.
- **The Windows archive was misnamed** — `Meld-1.8.zip` rather than
  `Meld-1.8.4-win-x64.zip`. `Path.with_suffix()` replaces everything after the last
  dot and the name is mostly dots, so the patch version, OS and architecture went at
  once. The release notes pointed at a filename the release did not contain.
- **B_Linear export failed on Linux.** The bundled `region_converter` binaries were
  committed without the execute bit, so the export died with a permission error on a
  binary that was plainly present.
- **The Linux generator could not start without WebKitGTK.** The published
  `arnis-linux` was built with the GUI feature, so the loader resolved webkit2gtk
  before `main()` and aborted on any headless or minimal system. Meld already passed
  `--no-default-features` when building from source; the released binary now matches.

## [1.8.4] - unreleased

**Meld is a desktop app.** Download a folder, run it, and it lives in the tray: no
Python to install, no repository to clone, no terminal. Closing the window stops
nothing, because the window was never doing the work.

Also groundwork for the Arnis fork's 3.0.7 upstream-port wave. Nothing about
generation changes in this release; it makes the generator binary Meld runs traceable,
and states where Meld has to move once the fork's CLI does.

### Added

- **Portable builds for Windows, macOS and Linux.** `packaging/build.py` produces a
  folder you extract anywhere, carrying its own Python runtime and its own `arnis`
  binary, so the target machine needs nothing installed. `--onefile` produces a single
  64 MB executable instead, with the generator embedded and unpacked on first launch.
  There is no installer, so there is nothing to uninstall and nothing that can delete
  a project folder on the way out.
- **Tray app.** `Meld.exe` starts with no console and no taskbar button. Clicking the
  tray icon shows or hides the status bar; the full UI is a menu item, so a stray click
  on a 16 px target never throws a 1360x880 window over your work. Quitting is the
  tray's Quit and nothing else.
- **Status bar.** A frameless strip that floats above other windows with one coloured
  block per worker (idle, queued, fetch, prepare, **build** in gold, save, merge,
  failed), plus the current task, ETA, CPU/RAM/disk and an optional two-minute
  CPU/RAM history graph. No title bar and no close box, so it is dismissed from its own
  right-click menu, so it cannot be shut by reflex six hours into a render. Position,
  opacity and hidden state are remembered.
- **Its own window.** The UI opens as an application window with no tabs and no address
  bar. `MELD_UI=browser` or the tray's *Open in browser* keeps the old behaviour, and
  an already-open window is raised rather than a second one opened.
- **Build stamp.** Every build records its version, UTC build time and commit; shown by
  `--check`, in the console banner and in the UI footer. The UI is baked into the
  bundle, so without this a stale binary serves a stale page with no outward sign.
- Desktop and Start-menu shortcuts (`packaging/Create-Shortcut.ps1`), a Linux `.desktop`
  entry and a macOS `/Applications` link (`packaging/install-shortcut.sh`), and a CI
  matrix that builds and smoke-tests all four targets.

### Fixed

- **The app could not survive being packaged.** `Path(__file__).parent` resolves inside
  the PyInstaller payload, which is read-only and temporary, so every write there works
  from source and fails for every packaged user. `src/paths.py` now separates read-only
  bundled files from the writable data directory. A source checkout resolves both to the
  repo root, so existing `projects/` and `cache/` do not move.
- **Orphaned generators.** Quitting, crashing or being killed left `arnis`
  children running: eight processes, every core pinned, no window to close them from.
  They now sit in a Windows Job Object that dies with the app however it dies, and in
  their own process group on POSIX.
- **A console window per cell.** A windowed process that starts a console program gets a
  new console for it, so a 3000-cell render flashed 3000 black windows. Children are
  spawned with `CREATE_NO_WINDOW`.
- **Renders died when the machine slept.** Hours of compute with no keypress looks idle
  to every power policy. Sleep is blocked while a run is active and released when it
  ends, is stopped, or the app quits.
- **The local API was open to any page you had open.** Binding to 127.0.0.1 stops the
  network, not the browser: any site could POST to it, and this API writes files and
  launches processes. Host and Origin checks are always on, plus a per-session token for
  the packaged app.
- **A second launch fought the first** for the port and the project folder. An OS file
  lock, not a PID file that a crash leaves behind, now makes the second launch open
  the running copy instead.
- Werkzeug's development server is replaced by waitress, with the channel timeout raised
  well past its 120 s default so a multi-minute export is not cut off mid-write.

### Changed

- The launcher starts the tray app and detaches once the port answers, instead of
  waiting on it, which is what pinned a console window to the taskbar for a whole
  session.
- The UI is squarer and darker to match the status bar: solid panels instead of
  translucent ones, no rounded corners, visible hairlines, and section headings that sit
  on the rail as rows rather than as a column of raised buttons.

### Fixed
- **The launcher could build and deploy a half-finished generator without saying so.**
  `build_arnis()`, the fallback that runs when no `arnis.exe` is present, ran a bare
  `cargo build --release` inside the fork checkout and copied `target/release/arnis.exe`
  over Meld's own tracked `arnis.exe`. In a checkout that is mid-development that silently
  ships an unreleased build, and it clobbers the `target/` directory of whoever is working
  on the fork. The build now goes to a launcher-owned directory outside the checkout, so
  the fork's `target/` and `git status` are left alone, and the log names the exact source
  it built: Cargo version plus `git describe --tags --always --dirty`. A dirty checkout
  additionally logs that the result is an unreleased build and how to fall back to a
  published release.

### Changed
- `src/__init__.py` carried `__version__ = "0.1.0"` while the project shipped 1.8.x. It now
  tracks the real version, so the tray/banner can report it instead of a literal.

### Notes
- The fork is at **3.0.7 (unreleased)**, see `arnis-283-src/CHANGELOG.md`. Everything landed
  there so far is gated on producing a byte-identical render, so no Meld change is required
  to run it.
- One Meld change *is* coming and is deliberately not in this release: upstream replaces
  `--terrain` with `--mode geo-terrain|geo-only|terrain-only`. When the fork lands that,
  `src/arnis_cmd.py` switches to `--mode` and the saved-project key stays `"terrain": bool`
  (no settings migration). The fork keeps a hidden `--terrain` alias, so the two sides can
  be updated in either order. Details: `docs/arnis-port-handoff.md`.

## [1.8.3] - 2026-08-10

A single-cause release: Meld could not generate anything at all on a Windows whose
language is not Western European. Nothing about generation changed.

### Fixed
- **Every cell failed instantly on non-Western Windows locales, blamed on "network
  timeout".** Meld read Arnis's console output using the machine's locale code page
  instead of UTF-8. Arnis prints its logo in solid-block characters (`█` = `E2 96 88`
  in UTF-8), and code page 1250 — Polish, Czech, Slovak, Hungarian, Romanian and the
  rest of Central Europe — has no character at `0x88`. The first line Arnis printed
  therefore raised a decode error, Meld killed the process, and the cell "failed" in
  0 seconds; both retries died the same way, so a fully cached, fully offline build
  still produced an empty world. Reading Arnis is now pinned to UTF-8, and the whole
  Meld process runs in Python's UTF-8 mode so no unpinned read anywhere can repeat it.
  Affected cp1250, cp1251, cp932 and similar; cp1252 machines happened to survive
  because `0x88` is mapped there.
- **The failure reason was read off the command line, not the output.** The cause shown
  on a red cell came from scanning the cell log — including the echoed `RUN arnis.exe
  … --timeout 600 …` line, so the word "timeout" in Arnis's *own flags* labelled
  unrelated failures "network timeout". The scan now skips the command line, and a real
  timeout in the output is still reported as one.
- **A failure to read Arnis's output was reported as nothing at all.** That path caught
  the exception and returned silently, which is why the cause never reached the log. It
  now writes the exact error into the cell log and the Meld log.
- **A failed cell now shows what Arnis said.** All Arnis output already went to
  `logs/cell-<x>_<z>_<n>.log`, but a failure printed one guessed word and nothing else, so
  a rejected argument, a panic and a binary that never started were indistinguishable. The
  last lines of the log are now echoed into the Meld log, with the exit code, and "no
  output at all" is reported as its own diagnosis instead of being labelled a network
  problem.
- **`-X utf8` on the launcher command line was silently discarded.** The launcher re-execs
  itself inside `.venv` with a freshly built argument list, which drops any `-X` flag, so
  starting Meld that way put the server back on the locale code page. UTF-8 mode now
  travels as an environment variable, which survives the re-exec and every subprocess.
- **Other locale-sensitive reads hardened.** The Overture pre-warm (which surfaced the
  same crash as a raw traceback in the console), the map-item pass, the folder picker
  and the world-linking and Java probes can no longer raise on a byte their locale does
  not map.

## [1.8.2] - 2026-08-10 - "Field reports"

Everything in this release came from people hitting it: a selection that took the server
down, a dependency that failed to install, a Stop button that looked dead, estimates that
read low, and the same four questions asked over and over. No behaviour changes to what
gets generated.

### Fixed
- **Selecting the whole world at 1:1 killed Meld.** Planning built the cell list before
  anyone counted it — about 200 million cells for the planet at 4-region cells, tens of
  gigabytes of dicts — and the process died mid-allocation. The count is now arithmetic
  and checked first: past ~20,000 cells the plan is refused with what to change (bigger
  cells, smaller scale, smaller area) instead of dying. Raise it with
  `MELD_MAX_PLAN_CELLS` if you know what you are asking for.
- **`zstd` in `requirements.txt` was never used.** Meld imports `zstandard`; the similarly
  named `zstd` package was pinned by mistake at a version with no wheel on some platforms,
  which failed the install for anyone whose Python did not have one. Removed.
- **"Stop bake" did nothing for minutes.** The parallel OSM bake only checked the stop flag
  when a whole `.pbf` finished, and the workers only polled it every 2M elements — a
  country file runs for minutes either way. The pool now polls on a timer and writes the
  stop sentinel immediately, both bake passes check roughly every 260k elements, and the
  relation pre-pass (previously uninterruptible) honours it too. Stop now lands in about a
  second, and tiles already written are kept.
- **The worker recommendation was capped at 8 regardless of CPU.** A 16-core / 32-thread
  machine was told to use 8 workers and left half idle, which is why people found a manual
  setting roughly twice as fast. The ceiling now scales with the core count; RAM and
  save-disk speed remain the secondary caps.
- **Size and time estimates ignored build height and caves.** Both used a flat figure per
  region measured on a vanilla-height, cave-less world, so a tall or cave-carved build read
  several times low. Estimates now scale with the declared world height, caves and baked
  lighting — and once a run finishes, Meld measures what actually landed on disk per region
  and uses that measurement for the project from then on.

### Added
- **Troubleshooting guide** (`docs/troubleshooting.html`, linked from the docs hub): the
  flat-world-on-a-server sequence, Overpass rate limits and going fully offline, what bake
  lighting actually does, why an estimate read low, selections too big to plan, worker and
  thread tuning, install problems, and the world artefacts people ask about.
- **Server hand-off steps** in the finished-world dialog: open it in single-player once,
  stop the server, move the whole folder (`level.dat` and `datapacks/` included, not just
  `region/`), match the version. Pasting a world over a running server is the single most
  common support question.
- **Minecraft 1.21.10, 1.21.11, 26.1 and 26.1.1** in the version selector, and **extended
  build height on 26.2**, which the fork previously refused for want of a verified pack
  format. A note under the selector states the one-way rule: a newer client opens an older
  world, not the reverse — use chunker.app to go backwards.
- **Plain-language notes in the UI** where the questions keep coming: bake lighting is for
  LOD mods and map renderers only (Minecraft re-lights on load, so it is not a fix for a
  dark world), and the OSM panel now explains Overpass rate limits, what Meld does about
  them, and that a local `.pbf` bake removes them entirely.

## [1.8.1] - 2026-08-06 - "Height"

Pick the Minecraft version you are building for, and let a world's vertical range follow
its terrain instead of vanilla's -64..319. Defaults are unchanged: a project that does not
touch these settings emits no new flags and generates exactly as 1.8.0 did.

### Added
- **Minecraft version** selector (Settings -> Arnis). Decides the `DataVersion` written
  into every chunk, whether extended build height may be declared at all, and the chunk
  layout. Only versions the fork has VERIFIED constants for are offered — an unverified
  one would produce a world that loads and then quietly misbehaves. Choosing a pre-1.17
  version disables the extended-height toggle with the reason shown inline, rather than
  hiding it.
- **Extend build height**, reworked. The world now declares exactly the range its terrain
  needs via a generated datapack, instead of a fixed 4064-block one. If the terrain fits
  vanilla, no datapack is written at all — no experimental-features prompt, and no
  unremovable pack, for nothing.
- **Headroom above peak** and **Underroom below floor** sliders: the room reserved for
  trees and buildings above, and caves and water carving below. Underroom past 8 is what
  pushes a world below vanilla's floor.
- **World floor Y / World ceiling Y** boxes for setting the range exactly. Blank = fitted
  from the terrain. A value that would cut into the terrain is refused with a reason
  rather than clamped, so mountain tops are never silently lost.
- `tools/height_test/height_check.py`: generates real worlds and reads back what landed on
  disk — the dimension geometry, `pack.mcmeta`, the `level.dat` registration, the
  `DataVersion` in the chunks, that every section lies inside the declared world, that
  both refusals exit non-zero, that a deep world's basement is filled rock, and that two
  tiles with very different water content still choose the same datum.

### Fixed
- **The master world lost its datapack.** `merge_cell_into_master` copied region files and
  `level.dat` but not `datapacks/`, while arnis registers `file/arnis_tall` inside each
  cell's `level.dat`. A merged world therefore asked for a datapack that was never copied,
  and Minecraft loaded it at vanilla height — every block above y=319 of an extended world
  gone. Packs are now copied before `level.dat` and under the same lock, so an interrupted
  merge can leave a spare pack but never a `level.dat` referencing a missing one, and a
  world whose first merge predates this fix is backfilled.
- **A Y-cliff along coastlines.** Meld now sends `--water-carve-clearance max` to every
  cell. Without it the fork measures that clearance per cell, so a cell holding deep water
  put its datum ~5 blocks above an inland neighbour (measured: Y -62 vs Y -57) and the
  shared border became a step. This is the one flag on the command line that exists purely
  because Meld tiles.

### Notes
- Because the clearance is now fixed, Meld's datum moves from Y -62 to Y -56 for inland
  areas. A fresh world is unaffected; **re-rendering an existing world will place it 6
  blocks higher**, so do not mix old and new cells in one world.
- With the default underroom (16) and Meld's terrain datum at Y -56, extended height
  writes a datapack for essentially every project rather than only mountainous ones. Set
  underroom to 8 if you want "no datapack unless the terrain needs it".
- **Confirmed in Minecraft 26.1.2.** A 4096x4096-block Yosemite build at 1:4, generated as
  four cells and merged: they agreed on geometry, datum and DataVersion, kept every section
  inside the declared world, and reached Y 559 — 240 blocks above vanilla's ceiling. The
  merged world opens in game with the generated height datapack active. The two-cell seam
  probe still reports 100.000% agreement on ground blocks and terrain Y.
- Two datapack bugs that only the game could find were fixed on the way there: the pack was
  written in the 1.21.x metadata schema, which 26.x rejects outright ("Errors in currently
  selected data packs prevented the world from loading"), and an intermediate change
  restamped level.dat's version, which made Minecraft skip the DataFixer and fail even in
  Safe Mode. Both are fixed in the bundled fork. Neither was visible to any file-level
  check — the files were internally consistent and said exactly what was intended — so an
  extended-height change is not done until a world has actually been opened.
- Extended height on **26.2** is currently refused: it uses the 26.x datapack schema but no
  verified pack format for it has been read out of a real pack yet, and guessing one
  produces a world that will not open.
- Requires the bundled Arnis Meld fork **3.0.5**.

## [1.8.0] - 2026-07-26 - "Farmlands"

The countryside comes alive. Farmland, grassland, and the plains OSM never mapped turn
into real agricultural land: rotated field parcels that follow the roads, monoculture
crop plots at different growth stages, wildflower meadows, dirt tracks, hay bales, and
scattered rocks and bushes — with live pattern previews and per-land-kind controls.
Bundles arnis fork 3.0.4.

### Added

- **Farmland texture drawer.** Five sliders (Coarse dirt / Plains / Flowers / Farmland /
  Moss) set the relative shares of farmland; each plot becomes one style, so
  `Plains 60%` makes ~60% of the farmland open grass. A **Profile** picker (Farmland /
  Grassland / Untagged land) gives each land kind its own five shares — grassland
  defaults grassy, untagged satellite land defaults to open plains instead of endless
  crops. Every value is click-to-type.
- **Real farm plots.** A "Farm plots" slider group weights what fields grow — wheat,
  potato, carrot, beetroot, sunflower rows, pumpkin patches, fallow. Each field grows
  one crop at one stage; neighbouring fields ripen differently, stray bird-sown patches
  and hay-bale bundles break up the carpet, and sunflower fields cluster in the low
  open plains.
- **Fields that follow the land.** Parcels sit in orientation domains at multiple
  angles — long strips and blocky plots — **aligned to the nearby road network**, with
  organically wandering domain borders and farmland that feathers into the surrounding
  grassland. Parcel sizes track the map scale, with a **Pattern size** slider (25-400%).
- **Texture more than farmland.** "Also texture open grassland" and "Also texture
  untagged land" (both on by default) extend the pattern to OSM meadows and to the huge
  satellite-classified plains OSM leaves blank; villages get grassy ground instead of
  wheat. Everything carries vanilla-density cover: short grass, ferns, large ferns,
  tall grass, ten wildflower species, sunflowers, dead bushes, moss.
- **Rocks & bushes.** One selector (Rocks + bushes / Rocks / Bushes / None): 8 rock
  formations and 60 bush schematics scatter at random rotations across farm, grass and
  untagged land — bushes in ~5% and rocks in ~2% of chunks, never on rivers, lakes,
  roads, or tilled fields.
- **Live pattern preview.** The "🌾 Preview pattern" canvas mirrors the generator —
  angled parcels, strips, domain borders, per-crop plot tints (farmland golden),
  vegetation speckle, scatter dots — and follows the selected profile, with
  scroll-zoom and a wide area range.

### Fixed

- **No stray snow on lowland farmland.** Snow now needs real mountain relief; genuine
  peaks still cap.
- **Nothing floats over water.** Sunflowers, flowers, moss and bushes no longer hover
  over rivers and lakes, and schematic pieces never stamp onto water.

## [1.7.0] - 2026-07-19 - "Meld Compass"

A big usability pass focused on guiding people to run and build everything easily: a guided three-step rail, a settings search, a darker glass skin, one Generate button, project folders and a render queue that builds worlds one after another, borders from any drawn shape, a slimmer in-place server setup, and a one-click cross-platform launcher. Bundles arnis fork 3.0.3 (generator unchanged).

### Added

- **Settings search.** A search bar at the top of the right rail finds any setting, toggle, or button. Enter cycles through matches (Shift+Enter for previous); the first match opens its section and scrolls into view, and every match is tinted. Esc or the gold clear button resets it.
- **Project folders and drag reorder.** In the Projects gallery, drag any project card (its ⠿ handle) to reorder it, or drop it on a folder to group it. 📁 creates a folder; each folder collapses, renames, deletes (its projects fall back to Ungrouped), and shows a count. Click a folder to FOCUS the map on only its areas; press its ▶️ to QUEUE only its projects. Saved in `projects/_org.json` (presentation only, never touches project files).
- **Render queue: pause, kill, and size estimates.** Queue several projects to render one after another. Pause holds between projects, Stop halts after the current one, and Kill aborts the running render immediately. Each queue row shows its area and estimated output size, with a combined total that reacts to each project's export format (Raw / Zip / Linear / B_Linear). While a queue runs, every queued project's area stays drawn on the map with the current one highlighted.
- **Borders and zones from a drawn rectangle or polygon.** A zone can use the area you drew on the map (the 📐 toggle) as its boundary instead of a country, so the concentric rings, `regions.yml` and `border.sk` build for any custom shape, not just admin boundaries.
- **One-click cross-platform launcher.** `meld.bat` / `meld.command` / `meld.sh` plus `meld_launch.py` create a private venv, install the dependencies, fetch or build the matching `arnis` binary, and start the app in one double-click on Windows, macOS, and Linux. New `python meld_launch.py --check` reports what is installed or missing without changing anything.
- **Run the server against the world in place.** Server setup links the world's real files into the server (no copy, no doubled disk) by default; a "Copy world into the server" option keeps the isolated behaviour.
- **Build-size estimate.** The Export drawer shows an estimated output size that changes with the chosen format.

### Changed

- **Three numbered steps.** The rail is now 1 Settings, 2 Prepare data, 3 Generate. Project & world, Selection / search, and Edit & retry live inside the Generate step. Every section starts collapsed and opens one at a time.
- **One Generate button.** Prepare & build, Generate & merge, and Resume unfinished collapse into a single ▶ Generate world that does the right thing for the current stage, including resuming after a stop or crash.
- **Projects everywhere.** A Projects gallery button sits in the left rail and directly under the Generate controls; the old project switcher, New project, New world and "show all project areas" controls fold into the gallery. Cloning a project now copies its settings AND pre-plans its cells (so the preview shows on open) and drops the copy right next to its source.
- **Darker glass skin.** Every semi-transparent surface is more opaque, so cards, rails, the projects overlay, and dialogs read as solid glass. Confirmation dialogs open above the projects window instead of behind it.
- **Tidier drawers.** Chest loot, tree sizes, cave biomes, climate, and export are collapsible drawers that look like a button when closed and open flush (no boxed panel), with a smoother open animation.
- **Cleaner Data pack and Border sections.** The height preview is hillshade-only with one button and a clear ✕, and the hillshade relief is much stronger. Border & zones flows without boxed rows, its ring / margin / script settings fold into one drawer, and Export border files is a full-width button.
- **Server setup does not back up on first start by default.** The manual Backup button and the opt-in toggle remain.
- **The page is served no-store**, so UI updates always appear on a plain refresh.

### Removed

- **Redundant `start.bat` / `start.sh`.** The `meld.*` one-click launchers replace them.

## [1.6.2] - 2026-07-15 - "Clear Waters"

Preview zoom and framing, a tree-size preview, and a worker-count fix. Bundles arnis fork 3.0.3, which removes the water colour bands, the thin land and road lines across water, and the cave lava seams, and renders climate boundaries as organic blobs.

### Added

- **Zoom and pan on every preview.** The climate, cave, and tree previews support scroll to zoom, drag to pan, and double-click to reset, so you can inspect the detail up close.
- **Tree-size preview.** The tree-size sliders now have their own live preview showing the spatial mix of sizes the sliders and the project scale produce, with a percentage legend, updating as you drag.

### Changed

- **Previews open on a centered 3x3-region window.** All three previews now start on a centered patch of at most 3x3 Minecraft regions in the middle of the selection, so you begin on local detail instead of the whole area downsampled to nothing, with a stepper to widen or tighten the window.

### Fixed

- **Generation starting with only 2 workers.** The low-scale burst clamp measured the legacy AWS tile cache, which the default Mapterhorn source never fills, so it wrongly held every run at 2 workers even when more were configured. The clamp now only applies when a run is pinned to legacy AWS, and the configured worker count is re-asserted at the start of every run so no run inherits a stale value.

## [1.6.1] - 2026-07-14 - "Climate & Contours"

A focused follow-up on climate, heightmaps, trees, and finishing a world cleanly. Bundles the Arnis fork **3.0.2**. Everything here is additive.

### Added

- **Climate preview.** A "Climate (Koppen)" card under the terrain toggles renders the exact grouped climate your area will get (biome tint plus arid/polar surface blocks) to a sidebar canvas with a colour-swatch legend and measured percentages. Backed by the fork's new per-block climate sampling, so a large region now varies smoothly across the map instead of snapping to one climate per cell.
- **Tree size popularity sliders.** The five tree-size on/off checkboxes become five relative-popularity sliders (0 to 200 percent, 100 = default, cave-slider UX with exact-value typing and reset), so you can make rarer big trees or a denser giant-heavy old-growth look. Emits the fork's new `--tree-size-weights`; a saved legacy checkbox setting is migrated automatically.
- **Bake Mapterhorn elevation.** A new Data pack button pre-downloads the tiles the new default heightmap (Mapterhorn, plus regional high-res providers) actually uses for your selection, so generation runs offline and is never rate-limited. Runs at the project scale so the cached zoom matches what the cells request.
- **Mapterhorn heightmap preview overlay.** A "Preview Mapterhorn" button renders the new heightmap for your selection using the real provider stack and overlays it on the map (hillshade or grayscale), distinct from the legacy AWS tile height preview, so you see the terrain the world will actually get.
- **Missing-region final check.** After a run finishes, Meld scans the merged world for regions that should exist but are missing or empty and marks each on the map with a warning emoji and a black square. A "Retry missing" button re-runs exactly those cells through the normal retry path so the holes fill in and merge back seamlessly; a "Check for gaps" button runs the scan on demand. The detector only flags interior holes and dropped-out cells surrounded by finished neighbours, so it never false-positives on the normal ragged coast or ocean edges.

### Changed

- The heightmap source toggle is confirmed and documented: the "Legacy AWS elevation" checkbox (off by default) is the switch, with Mapterhorn plus regional providers as the default modern source.

## [1.6.0] - 2026-07-12 - "Above & Beyond"

An above-ground release to match the underground one. The bundled Arnis fork jumps to
**3.0.1**, reaching feature parity with upstream louis-e/arnis 3.0.0, and Meld surfaces
the new world-shaping options: bundled 3D props at real map features, a visual chest-loot
editor, per-world game mode and time-of-day settings, an in-world map item, richer building
facades, and a global Mapterhorn elevation source that replaces the legacy AWS tiles'
broken-tile cliffs. Everything stays seam-safe: the master-origin elevation grid and
tile-invariant rendering are unchanged, so multi-cell worlds still merge seamlessly.

> Engine note: the bundled Arnis fork moves Teddy563/arnis 2.9.3 -> 3.0.1. Every upstream
> 3.0.0 feature was ported faithfully and audited for cross-tile seam-safety. Existing
> worlds are untouched; new behaviour applies to newly generated cells only.

### Added

- **3D props at map features.** A Props menu (Settings) toggles families of bundled props
  stamped where they belong: boats on water, parked cars in car parks, cranes and
  excavators on construction sites, tractors on farmland, wind turbines at wind
  generators, lighthouses, fountains, playgrounds, and cemetery tombstones. Off by
  family = no extra time or RAM.
- **Visual chest-loot editor.** Edit interior-chest loot per theme with item sprites, a
  searchable picker, 56 vanilla-structure presets (trial chambers, stronghold, buried
  treasure and more), and a raw-JSON escape hatch. Registry-validated so typos cannot
  reach generation.
- **World settings.** Game mode (survival / creative / spectator) and time of day written
  into `level.dat`, plus a **World map item** toggle that drops a locked filled-map of the
  whole world into the player's inventory, added in one post-merge pass so a multi-cell
  region gets a single correct map.
- **Richer building facades.** Downtowns render six distinct skyscraper styles (glass,
  glass-corner, grid, contemporary, modern, masonry) chosen from real OSM material and
  historic tags, plus base-course plinths, full-glass shop storefronts, string-course
  cornices, coherent window frames on commercial and historic buildings, and a
  per-building window offset so neighbouring buildings stop sharing one citywide window
  grid.
- **Sport pitch markings**, **furnished bedrooms** (real beds), **electrified rail
  catenary**, **street lamps**, **highway tunnels**, **modular bridges**, **helipad pads**,
  **more surface materials**, **climate-driven biomes** with warm / cold / frozen oceans
  and rivers, **street trees on plazas**, trees that no longer grow through bridge decks,
  and canopies that drape over low roofs. All from the 3.0.1 engine, driven automatically
  per cell.

### Changed

- **Elevation now defaults to Mapterhorn** (global terrarium tiles with pyramid
  hole-proofing). Regional high-res providers still win where they cover the area; the old
  AWS source is now a legacy escape. The elevation toggle was relabelled "Legacy AWS
  elevation", and "Regional elevation only" now falls through to Mapterhorn instead of AWS.
  `--offline` is honoured (cache hit served, cache miss degrades to flat rather than
  hitting the network).

### Fixed

- A **stream-to-disk corruption guard** (large exports could truncate saved chunk data), a
  **CLI spawn-Y** fix (players no longer spawn buried or in the air on nested output
  paths), and **basin-gated farmland irrigation** (no more crops washed out on slopes).

## [1.5.0] - 2026-07-04 — "Meld Depths"

The long-awaited underground release. Worlds are no longer solid rock below the surface: one
**Caves** toggle carves a full vanilla-style cave system into every cell at generation time —
caverns, tunnels, rivers, lakes, ores, geodes and eight themed cave biomes — deterministic and
seam-safe across tiles. Around it, this release makes the finished world go further: an
**Export & compression** suite (zip / tar.zst / Linear / B_Linear, up to ~4.9× smaller), a
**one-click Leaf server** builder that turns the world into a running localhost server with the
border plugins pre-staged, and the Border & zones Skript completing its **real particle wall**.

> Engine note: the bundled Arnis fork moves Teddy563/arnis 2.9.2 -> 2.9.3. The entire cave
> system lives in the fork (`--caves`, +6,178 lines); Meld drives it with one toggle. Existing
> worlds are untouched — caves appear in newly generated cells only.

### Added

- **Caves.** One Settings toggle. The fork ports Minecraft 1.21.8 cave worldgen to Rust and
  carves it into the filled ground during generation: cheese caverns, spaghetti tunnels and
  noodle worms from the vanilla noise density field (vanilla's own 4x8x4 cell interpolation),
  random-walk tunnel and ravine carvers, pool caves and long snake rivers that breach into
  caves only while descending, a contained deep lava sea below y=-54, the vanilla ore table
  plus stone-variety patches, amethyst geodes, and **8 cave biome themes** (lush, dripstone,
  deep dark, mushroom, ice under mountains, amethyst, volcanic at the world floor, coral pools)
  covering about half the underground. Every pass is a pure function of (seed, position), so
  adjacent cells carve the same caves at their shared seam. A bundled `cave-pack/` of 143
  `.schem` formations (ice spikes, dripstone columns, crystal clusters, clay basins) ships
  with Meld — like the tree packs — and decorates cave floors and ceilings automatically;
  caves still generate fully without it.
  - **Configurable biome mix + zone preview.** With Caves on, a **Cave biomes** panel gives
    every theme its own slider (drag in 10% steps, click the % to type any exact value,
    double-click a slider to reset it; 0 = off, defaults untouched when you don't touch
    them). **🗺 Preview** renders the REAL zone layout for your world's seed right in the
    panel: a zoomed-in window with the patches in their noise shapes over gray rock, one
    canvas for the upper caves and one for the deep, with the measured share of every theme
    — and hovering a biome in the list lights up just its patches. Backed by the fork's
    `--cave-biomes` / `--cave-zone-map`; a changed mix stays deterministic and seam-safe,
    and untouched sliders keep the world byte-identical to the default.
  - **Cave polish (bundled fork 2.9.3):** deep dark is now deep-only (below the deepslate
    line, like vanilla — no more sculk in shallow caves), and the diorite/granite/andesite/
    dirt patches are placed relative to the local surface instead of absolute Y, so they
    fill the rock under every column at any terrain height (a valley region went from 4
    diorite blocks to ~173,000 — the proper vanilla stone-variety look).
- **Export & compression.** An Export format dropdown on the Project & world card:
  - **Zip / tar.zst** — universal archives, extract back to a vanilla single-player world
    (~1.85× smaller; Anvil chunks are already compressed, which is the honest ceiling for
    archives).
  - **Linear** — per-region `.linear` for Leaf/Folia servers, ~4.85× smaller at level 9. Write
    it next to the `.mca` or as a separate `<name> [Linear]` sibling world (the original stays
    untouched). Optional stream-and-free keeps peak disk near the compressed size, and overlap
    mode compresses while generation still runs.
  - **B_Linear** — builds a `<name> [BLinear]` sibling world for Leaf's `B_LINEAR` through a
    bundled cross-platform Rust converter (`region-convert/`, based on LuminolMC's
    region_converter, MIT, ~1.2-2.5× faster than the Python codec). Keep-modes after the new
    world verifies: keep both (default), B_Linear only, or archive the `.mca` as a zip first.
    The `.mca` is only ever removed after everything verifies.
  - **`meldconvert.py`** — a standalone CLI for `mca <-> linear <-> blinear` conversion using
    the same verified codecs, plus an `info` inspector.
- **Server setup (one-click Leaf server).** A new card next to Border & zones turns the
  finished world into a ready-to-run local server:
  - Pick any Leaf version (1.21.4 to latest, fetched live), mount the world as the server's
    main world or as a Multiverse sub-world, and choose the **world files** (auto-follow the
    export settings, or explicitly `.mca` / `[Linear]` / `[BLinear]`) — the server's
    region-format config always matches the files actually staged.
  - Five explicit steps: **Plan** (dry run listing the exact jar build and every plugin version
    with hashes), **Stage** (configs + a copy of the world), **Download** (confirmation
    required; every file hash-verified — sha256 from Leaf, sha512 from Modrinth), **Accept
    EULA** (its own deliberate step), **Start / Stop** — with a live console and command input.
    The border plugins (WorldGuard, WorldEdit, Skript, skWorldGuard, SkBee) install
    automatically and the exported border files are pushed and loaded on first start.
  - **Localhost only, by design**: servers are set up for `127.0.0.1:25565`, offline mode.
    Meld never touches reachability; going further is your own `server.properties` edit.
  - **Voxy option**: installs Voxy Server Side (exact-version matched) and writes its config,
    so players with the Voxy client see the whole map as distant terrain the moment they join.
    Works with MCA, Linear and B_Linear alike (it requests chunks through the server, never
    parses region files).
  - **Crash watchdog**: if the server dies without a Stop, Meld restarts it (max 3 per 10
    minutes), and the first Start zips a world backup automatically — **optional**: a
    "backup world before first start" toggle (default on) skips it for big worlds, where a
    1 GB world means a ~1 GB zip (region data barely recompresses); the manual 💾 button
    stays available either way. Version, mode, folder and extras persist per project.
  - **RAM + CPU sliders, adapted to your machine**: set the server's heap (slider at 0 =
    auto — a quarter of your RAM, 2-8 GB, always capped 2 GB below total) and a CPU % that
    maps to the JVM's `ActiveProcessorCount` (the lever that actually sizes its GC and
    worker thread pools). Live readouts show the resolved result ("50% = 12/24 cores");
    values persist per project, apply on the next Start, and the written
    `start.bat`/`start.sh` stay in sync with every launch.
  - **Pop-out console**: the live server console opens in its own window (like the Log
    pop-out) with a command input, backed by a lightweight console feed endpoint.
  - Voxy / extras checkboxes persist the moment they change, not only when a Plan runs.
- **Border & zones: the particle wall is real.** The generated `border.sk` now embeds its wall
  geometry (bucketed segments built on load) and draws per-player dust curtains with SkBee —
  hard wall yellow, safe edge orange, country borders aqua — scanning only the cells around
  each player. No point-file loading, no extra Skript addons beyond SkBee.

### Changed

- **Flat cards.** Border & zones and Server setup no longer nest a dropdown inside the card;
  the card header is the one collapse and opens straight onto the controls.
- **Export defaults stay raw.** `none` remains the default format, so an untouched build still
  yields a working vanilla `.mca` world; compression is always opt-in.

### Fixed

- **Region-format mismatches are impossible by construction.** The staged world folder and
  Leaf's `region-format` are decided together and validated against the files on disk, so a
  Linear world is never served as MCA or vice versa. Verified against the Leaf 26.1.2 binary
  itself (disassembled constants + on-disk files): our `.b_linear` output is byte-identical to
  Leaf's B_LINEAR bucketed v3 layout (same superblock, version byte, header fields, hash
  seed), and our classic Linear v1 files load under Leaf's LINEAR_V2 (it reads v1/v2/v3).
- **Server-written Linear worlds convert back cleanly.** A Leaf server rewrites `.linear`
  regions as version 3, which Meld's native v1 codec cannot read; `meldconvert.py` now
  detects v2/v3 sources and routes the conversion through the bundled region_converter
  (which reads all versions) instead of failing, and the codec's error message says exactly
  what to do if hit directly.
- **The server card survives a Meld restart.** Backup, EULA and Start fall back to the
  per-project server profile when Meld was restarted after staging, so an already-built
  server works with just a fresh Plan — no re-Stage (which would re-copy the world)
  required. Live-verified: restart → Plan → Start = 12 s warm boot on a B_LINEAR world.

## [1.4.0] - 2026-06-22

The "real trees, shape the land, and bound a world to a country" release. Meld now places a
region-aware schematic tree pack instead of the simpler procedural trees: 1,959 hand-made models
across 448 species and 10 world regions, chosen by location, scattered into natural groves, in five
size tiers, anchored reliably on slopes and at water edges. Two new terrain knobs let you make
mountains as tall as you want without widening the map and put snow exactly where you want it. And a
new Border & zones server tool turns one or more countries into ready WorldGuard regions, point
files, and a generated Skript that fences a server to that border.

> Engine note: the bundled Arnis fork moves Teddy563/arnis 2.9.1 -> 2.9.2 (version bump for the tree
> pack and terrain flags). Existing worlds and settings are untouched; the new tree placement applies
> when the Schematic trees toggle is on.

### Added

- **Region trees.** A bundled region-aware schematic tree pack: **1,959 tree models**, **448
  species**, across **10 world regions**, **172 communities**. The region is picked from your
  selection (Africa -> acacia/baobab, Europe -> oak/spruce, the tropics -> jungle/palms), then a
  community by terrain, then a species, with Vanilla+ familiar trees mixed in everywhere. Models are
  created by the artist [paleozoey](https://www.planetminecraft.com/member/paleozoey/); Meld bundles
  and places them. Drive it from **Settings -> Schematic trees / Tree biome / Tree sizes**. Guide:
  [meldmc.com/docs/tree-packs](https://meldmc.com/docs/tree-packs).
- **Five size tiers with toggles.** small (<=6) / medium (7-12) / big (13-20) / tall (21-28) / giant
  (29-40), each on/off in Settings. Giant is off by default and only at 1:1; tall is rare. A disabled
  tier falls back to a smaller one, never a gap.
- **Terrain height x (vertical exaggeration).** Multiplies terrain height only, not the map
  footprint, so mountains get taller at the same map size and auto-compress to the build height. At
  small ratios real relief flattens, so x2-2.5 brings the mountains back. Default 1.0.
- **Snow modes.** off / realistic (real latitude snow line) / peaks (snow on the top N% of the
  world's height) / manual (above a Y you pick), with a percent for peaks. Pairs with the height
  multiplier. Guide: [meldmc.com/docs/terrain-and-snow](https://meldmc.com/docs/terrain-and-snow).
- **Border & zones (server tool).** Pick one country or several (they combine into one landmass),
  set the band sizes, preview the rings on the map, and export to `<project>/border/`: per-ring point
  files, a WorldGuard `regions.yml`, and a generated `border.sk` (country titles, an escalating
  soft->hard kill-zone, a fling-back wall, per-player packet-particle walls). A points control
  (50-1000) makes the border hug the real coast; an optional trim limits generation to the border so
  the void hides behind the wall. Guide:
  [meldmc.com/docs/border-zones](https://meldmc.com/docs/border-zones).
- **Docs quick-menu.** A 📖 Docs button in the app header opens the full guide index without leaving
  Meld.

### Changed

- **Trees default to the schematic pack.** With the Schematic trees toggle on, Meld places the
  region pack instead of the procedural trees; turning it off restores the legacy procedural trees.
- **Habitat remaps.** High elevation pulls conifers onto mountains; wetlands and coasts pull
  mangroves, willows and cypress, so montane reads montane and swamps read swampy anywhere.

### Fixed

- **Float fix.** Trees anchor on slopes and at water edges; no floating trunks on hills or over
  rivers.

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
