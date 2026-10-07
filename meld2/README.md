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

You need an Arnis 3.4 executable: either the CLI build or the GUI build, which also accepts CLI flags.

```sh
meld2 caps --arnis path/to/arnis.exe              # version and --capabilities
meld2 run examples/e2e.toml --arnis path/to/arnis.exe
meld2 status examples/e2e.toml                     # or `meld2 status` for every project
meld2 stop examples/e2e.toml                       # from another shell; resume with `run`
```

Meld looks for Arnis in this order: `--arnis`, then `arnis = "..."` in the project file, then `MELD2_ARNIS`.

State and logs go to `MELD2_HOME`. If that is not set, they go to `%LOCALAPPDATA%\Meld2` on Windows and to `$XDG_DATA_HOME/meld2` or `~/.local/share/meld2` elsewhere.

## Project file

```toml
format = 1
name = "Alps"
output = "saves"                  # saves folder, relative to this file

[run]
jobs = 2                          # Arnis processes at once
cpu_target = 90                   # % of cores, split evenly between jobs
# ram_budget_mb = 16000           # split evenly between jobs

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
