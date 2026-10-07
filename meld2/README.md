# Meld 2 (2.0.0-alpha.3, Phase 3)

Meld 2 is a Rust rewrite of Meld. It builds **projects**, which are saved sets of selections, through stock **Arnis at Scale** (Arnis 3.4+). Each selection has its own settings and builds into a One World. Selections that share a world extend it one after another. Selections in different worlds can build at the same time. A killed or stopped run resumes where it stopped.

The Python Meld 1.x app in the repository root keeps working until 2.0 replaces it. The plan is in [`docs/PLAN.md`](docs/PLAN.md): five phases, then 2.0.0-beta.1.

## Build

```sh
cd meld2
cargo build --release          # target/release/meld2(.exe)
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

## Run

```sh
meld2 run examples/e2e.toml          # finds or downloads Arnis, plans, builds
meld2 plan examples/e2e.toml         # the size and disk plan alone
meld2 status examples/e2e.toml       # or `meld2 status` for every project
meld2 stop examples/e2e.toml         # from another shell; resume with `run`
meld2 run examples/e2e.toml --rebuild=vaduz   # build that selection again from scratch
meld2 serve                          # the JSON API and a status page (below)
meld2 import ~/Meld/projects         # Meld 1 projects and presets to Meld 2 files
```

A run first prints the plan: for every selection, the pieces, region files, chunks, chunks still to build and the estimated size (3.84 MB per full region, the figure Arnis's GUI uses), from Arnis's `--plan-units` dry run. If the estimate plus 25 % and `run.min_free_mb` does not fit the free space of the saves volume, the run is refused; under twice the estimate it warns.

Only one `meld2 run` of a project runs at a time; a second is refused. The machine does not sleep while a run builds: Windows through the thread's execution state, Linux through `systemd-inhibit` and macOS through `caffeinate` when they are installed. All three end with Meld.

After the build, a final check runs `--plan-units` again on every built selection and reports the chunks still missing. Each run writes a JSON report, `reports/run-<unix time>.json` in the project's state folder: every step with its CPU and RAM share, start and end, result, Arnis's totals (wall, CPU seconds, peak RSS, chunks), every piece record, and the final check.

`--rebuild` (every selection) or `--rebuild=a,b` builds selections again from scratch: Meld forgets their state and removes Arnis's partial job folder (`arnis_one_world/jobs/<rect>_n<N>`), and Arnis writes every piece again over the world. Without it, a selection built with other settings is skipped, and a partial one with other settings is refused.

## Arnis

Meld 2 drives Arnis 3.4 (Arnis at Scale) or newer, the CLI build or the GUI build (which also takes CLI flags). It uses the first of:

1. `--arnis <path>`
2. `arnis = "..."` in the project file (relative to the project file)
3. the `MELD2_ARNIS` environment variable
4. `arnis.exe` / `arnis` next to the `meld2` binary (a bundle)
5. the pinned release downloaded earlier, in `<data>/arnis/3.4.0-beta.1/`
6. otherwise it downloads the pinned release, `Teddy563/arnis` v3.4.0-beta.1, and checks its SHA-256 (the hashes are in `core/src/install.rs`). On Linux and macOS it unpacks the `.tar.gz`.

Whatever it finds, Meld runs `--version` and `--capabilities` and refuses an Arnis older than 3.4.0-beta.1 or one that lacks `progress-json`, `unit-regions`, `one-world-workers`, `plan-units`, `threads` or `ram-budget`, and says how to fix it.

```sh
meld2 arnis status [--arnis P] [--project F]   # which Arnis, why, its version and capabilities
meld2 arnis install [--version 3.4.0-beta.1]   # download and verify the pinned release
meld2 arnis path                                # the path alone (never downloads)
meld2 caps [--arnis P]                          # version and capabilities
```

## Budget

`run.jobs` Arnis processes run at once, never two on one world. Each job gets a share of `run.cpu_target` % of the cores (`--threads`) and of `run.ram_budget_mb` (`--ram-budget-mb`; unset, 80 % of the free RAM on Windows and Linux), split by the jobs that can still run together. The share is worked out when a job starts, so when one ends the next gets the freed part, and the last job gets the whole budget. Each job runs `--one-world-workers auto`, so Arnis picks how many pieces build at once from the piece count and that share. A selection's own `threads`, `cpu_target`, `ram_budget_mb` or `workers` wins.

## Data steps: bakes and prewarms

```toml
[defaults]
osm_pbf = "geofabrik"     # read OSM from a Geofabrik extract...
osm_pbf_url = "https://download.geofabrik.de/europe/liechtenstein-latest.osm.pbf"   # ...this one
prewarm = true            # fill the caches before each build...
offline = true            # ...so the build never touches the network

[[bake]]
id = "liechtenstein"
osm_pbf = "geofabrik"     # the selections' osm_pbf and osm_pbf_url; optional bbox = [...]
osm_pbf_url = "https://download.geofabrik.de/europe/liechtenstein-latest.osm.pbf"
```

A `[[bake]]` makes Arnis cut (bake) an `.osm.pbf` extract once. The area is the bake's `bbox`, or else every selection that reads the same extract, plus Arnis's piece margin. Those selections then read the bake instead of the extract.

Arnis keeps bakes per extract. A Geofabrik bake therefore needs `osm_pbf_url`, and its selections need the same value, so that all of them read one extract. Without it, Arnis picks the smallest Geofabrik region that holds the area, and the region around the whole area can differ from each selection's own. In testing, the padded Vaduz + Schaan area reached across the Rhine, and Arnis picked `alps` (2.3 GB) instead of `liechtenstein` (3.5 MB). A local `.osm.pbf` path needs no URL. A bake is an Arnis `--prewarm` with the other sources off, so it also fetches land cover for the area, but no elevation. Bakes run first, one at a time, with the whole budget. A selection whose bake did not finish is refused.

`prewarm = true` runs the selection's own command with `--prewarm` before its build, on its world: Arnis downloads OSM, Overture, elevation, land cover, canopy and 3D models for every piece and writes nothing. `--offline` is left off that step, since Arnis refuses it with `--prewarm`. Bakes and prewarms report `transfer` progress and keep their own state as `bake:<id>` and `prewarm:<id>`. A step that finished with the same command is not run again.

Arnis has no CLI for preparing a local tile archive (arnis-tiles is a separate tool). A prepared archive is used through `osm_tiles_url = "<folder>"`.

## Polygon selections

```toml
[[selection]]
id = "li"
polygon = [[[47.05, 9.47], [47.27, 9.53], [47.06, 9.63]]]   # rings of [lat, lng]; their union
world = "Liechtenstein"
```

Arnis takes only `--bbox`. When the project loads, a polygon selection becomes `li-1`, `li-2`, and so on: one bbox for each run of piece-sized cells in a row that overlap the shape. A cell is `unit_regions × 512` blocks at the selection's scale. The parts share the selection's world and settings, so they meet without seams. The cells are piece-sized but not snapped to Arnis's piece lattice, and a polygon that crosses the antimeridian is not supported. `--rebuild=li` rebuilds every part.

## Server mode

```sh
meld2 serve [--bind 127.0.0.1:7878] [--dir <workspace>] [--arnis <path>]
```

`serve` prints its URL and token. Open `http://127.0.0.1:7878/?token=<token>` for a status page with projects, steps, live progress and the log. The page is minimal on purpose: the GUI (Phase 5) replaces it and uses the same API. Projects live in the workspace as `<name>/project.toml`. The default workspace is `<data>/workspace`; `meld2 import --out <workspace>` drops Meld 1 projects there.

| Method | Path | |
|---|---|---|
| GET | `/api/projects` | the workspace's projects with each step's state |
| GET / PUT | `/api/projects/<name>` | `{name, path, toml, state, running}`; PUT takes the TOML and checks it before it saves |
| POST | `/api/projects/<name>/run` | starts a run in the server (`?rebuild=a,b` or `?rebuild=all`); 409 if one runs |
| POST | `/api/projects/<name>/stop` | asks its run to stop, whether it was started here or by `meld2 run` |
| GET | `/api/projects/<name>/plan` | pieces, chunks and MB per selection, and the disk verdict |
| GET | `/api/status` | the saved state of every project Meld knows |
| GET | `/api/events` | Server-Sent Events, one JSON object per line: `{project, id, note, ...}` with `note` = `log`, `skipped`, `refused`, `started`, `event` (Arnis's NDJSON record), `finished`, `stopping` or `end` |

**Security.** Every request needs the token, on loopback as well, so other web pages in the browser cannot drive Meld either. Send it as `X-Meld-Token: <token>` or `Authorization: Bearer <token>`. The page and `EventSource` cannot set headers, so they use `?token=`. The token is 128 random bits from the OS, new at each start, unless `MELD2_TOKEN` (at least 16 characters) sets it. It is compared in constant time. Project names in paths are letters, digits, `-` and `_` only, and bodies are capped at 1 MB. `--bind 0.0.0.0:7878` opens it to the network. It is still token-only, but plain HTTP, so use a trusted LAN, an SSH tunnel or a TLS reverse proxy.

```sh
T=<token>; U=http://127.0.0.1:7878
curl -X PUT -H "X-Meld-Token: $T" --data-binary @project.toml $U/api/projects/alps
curl -X POST -H "X-Meld-Token: $T" $U/api/projects/alps/run
curl -N "$U/api/events?token=$T"
```

## Importing Meld 1

```sh
meld2 import <Meld 1 project folder | its projects/ folder | its data folder | preset.json> [--out .]
```

Each Meld 1 `projects/<slug>/project.json` becomes `<out>/<slug>/project.toml`, and each preset becomes `<out>/presets/<name>.toml`, a `[defaults]` fragment. Existing files are never overwritten. The keys map as `docs/PLAN.md` §3.6 lists them, following Meld 1's own rules. For example, `road_detail_level = auto` becomes compact below scale 0.7, and the mixes are written only where Meld 1 sent the flag. Two cases differ:

- `offline_elevation` also turns on `prewarm`.
- `snow_mode = peaks` becomes `manual` from Meld 1's `snow_y`, because Arnis refuses peaks in a One World.

The import prints every key as mapped, dropped on purpose (the governor, cells and merge, export and server keys) or **NOT MAPPED**, and writes the unmapped ones at the top of the file. A Meld 1 world cannot be extended, because its frame is equirectangular. The import therefore starts a new One World named after the project, and the old world stays playable. `grid.json` and the gallery folder in `_org.json` are reported and not carried.

## Process cleanup

Stopping a run, or Meld dying in any way, takes Arnis and its pieces with it. On Windows each run is in a Job Object that closes with Meld. On Linux and macOS Arnis runs in its own process group, watched by a small `sh` that kills the group when its pipe from Meld closes.

## Data

State, logs, run reports, downloaded Arnis builds and Arnis's caches (`<data>/cache`, passed as `ARNIS_CACHE_ROOT` unless you set it yourself) go to `MELD2_HOME`. If that is not set, they go to `%LOCALAPPDATA%\Meld2` on Windows and to `$XDG_DATA_HOME/meld2` or `~/.local/share/meld2` elsewhere.

## Project file

```toml
format = 1
name = "Alps"
output = "saves"                  # saves folder, relative to this file

[run]
jobs = 2                          # Arnis processes at once
cpu_target = 90                   # % of cores, split between the jobs running
# ram_budget_mb = 16000           # split the same way; unset: 80 % of free RAM
# min_free_mb = 1024              # disk to keep free after the estimated build

[defaults]                        # every selection starts from these
scale = 1.0
unit_regions = 4                  # piece size; pieces are what resumes

[[selection]]
id = "vaduz"
bbox = [47.139, 9.520, 47.141, 9.523]   # min_lat, min_lng, max_lat, max_lng
world = "Alps"
settings = { caves = true, snow_mode = "peaks" }
```

Settings use the names of the Arnis flags in snake case, e.g. `snow_mode` for `--snow-mode` and `buildings = false` for `--no-buildings`. The full list is the `OPTS` table in `core/src/args.rs`.

Arnis validates the values. Meld checks the key names and value types, and checks that the Arnis build lists the capability each setting needs. `extra_args = ["--flag", "value"]` passes flags that Meld does not model, unchecked.
