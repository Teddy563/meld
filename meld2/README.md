# Meld 2 (2.0.0-alpha.2, Phase 2)

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
```

A run first prints the plan: for every selection, the pieces, region files, chunks, chunks still to build and the estimated size (3.84 MB per full region, the figure Arnis's GUI uses), from Arnis's `--plan-units` dry run. If the estimate plus 25 % and `run.min_free_mb` does not fit the free space of the saves volume, the run is refused; under twice the estimate it warns.

Only one `meld2 run` of a project runs at a time; a second is refused. On Windows the machine does not sleep while a run builds.

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

## Process cleanup

Stopping a run, or Meld dying in any way, takes Arnis and its pieces with it. On Windows each run is in a Job Object that closes with Meld. On Linux and macOS Arnis runs in its own process group, watched by a small `sh` that kills the group when its pipe from Meld closes.

## Data

State, logs and downloaded Arnis builds go to `MELD2_HOME`. If that is not set, they go to `%LOCALAPPDATA%\Meld2` on Windows and to `$XDG_DATA_HOME/meld2` or `~/.local/share/meld2` elsewhere.

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
