# Meld 2.0 — a Rust orchestrator of stock Arnis at Scale

Date: 2026-10-07.

Releases: the work is cut into **Phases 1–5**. Each phase is a pre-release `2.0.0-alpha.N` (Phase 1 was `alpha.1`, Phase 2 `alpha.2`; Phase 3 is `alpha.3`). **2.0.0-beta.1 is the release with all five phases merged.** rc and 2.0.0 come after the user's UI pass. Arnis at Scale now covers most of Meld's engine features, so Meld 2 is mainly the orchestrator, server and projects layer.

Baselines:
- Meld 1.9.9: `Teddy563/meld` @ `4152dcb`, tag `v1.9.9`.
- Arnis at Scale 3.4.0-beta.1: `Teddy563/arnis` `arnis-scale-single` @ `47b4d8e7`, binary `work/release/arnis-3.4.0-beta.1.exe`.
- Meld 2: branch `meld-2.0`, worktree `work/wt-meld2`, folder `meld2/`. Phase branches: `meld-2.0-phase-1` = `52f8101`, `meld-2.0-phase-2` = `1ace4fe`, `meld-2.0-phase-3` (local).

Every Meld claim below cites a file and line that I read or grepped on 4152dcb. Arnis claims cite the `wt-single` tree.

How to read the tables:
- **A**: Arnis at Scale already does it (the flag is named).
- **O**: Meld 2 orchestrates Arnis to do it.
- **M**: Meld 2 implements it itself.
- **X**: dropped.
- **Arnis coverage** = A ÷ (rows − X).

---

## 1. What Meld 1.9.9 does, and where each part goes

### 1.0 Summary

| Group | Rows | A | O | M | X | Arnis coverage | A + O |
|---|---|---|---|---|---|---|---|
| Projects and selections | 11 | 4 | 2 | 4 | 1 | **40 %** | 60 % |
| Queue and jobs | 14 | 4 | 4 | 4 | 2 | **33 %** | 67 % |
| Server (Leaf/Paper, datapacks, borders, export) | 12 | 2 | 1 | 7 | 2 | **20 %** | 30 % |
| Previews and maps | 10 | 4 | 0 | 5 | 1 | **44 %** | 44 % |
| Updater, tray, app shell | 13 | 0 | 0 | 9 | 4 | **0 %** | 0 % |
| Conversion (B_Linear, region-convert) | 5 | 1 | 1 | 2 | 1 | **25 %** | 50 % |
| Benchmarking | 5 | 3 | 0 | 0 | 2 | **100 %** | 100 % |
| Data, caches, generation settings | 9 | 6 | 0 | 2 | 1 | **75 %** | 75 % |
| **Total** | **79** | **24** | **8** | **33** | **14** | **37 %** | **49 %** |

This inventory is at the level of the app. The 589-feature audit (`02-FEATURE-MATRIX.md`, `10-STATUS-REPORT.md` §2c) is mostly at the level of generation, and there ≈ 52 % of features are already in Arnis.

What Meld 2 must write itself is the shell, the server tooling and the multi-world layer. The world generation itself is in Arnis.

### 1.1 Projects and selections

In Meld 1, one project is one world, one selection and one locked origin (`src/project.py:1-5`). The selection is a bbox plus optional polygon rings (`project.py:505-520`), cut into a grid of cells.

| Feature | Meld 1.9.9 source | → | How in Meld 2 |
|---|---|---|---|
| Many projects: new, switch, clone, rename, delete; gallery order and folders | `server.py:5409-5558`, `_org.json` `:5381-5408` | M | Project files plus a dashboard (§2) |
| One world per project, with a locked origin | `project.py:523-531` | O | `--one-world --world-name --origin`. Meld 2 generalises this to N selections × N worlds |
| Polygon and multi-ring (country) selections | `grid.py:153-196`, `server.py:4494-4513` | M | Arnis takes only `--bbox`. **Done in Phase 3:** a `polygon` selection becomes one bbox per run of piece-sized cells in a row that overlap the rings (not snapped to the lattice) |
| Cell editing: add/remove mode, toggle, paint, grow rings, clear | `server.py:4489-4660` | M | Edit selections; the plan comes from `--plan-units` |
| Trim open-ocean cells | `server.py:4678-4731` | X | Pieces over sea are cheap in 3.4. Revisit if a measurement says otherwise |
| Guard against plans that are too large (`MAX_PLAN_CELLS`) | `grid.py:19-22` | A | Arnis refuses a map-id overflow ("too many pieces", `scale/mod.rs`). The disk plan is in §2 |
| Elevation survey and lock across cells | `survey.py`, `project.py:585-595` | A | One World pins the elevation affine in its manifest before the first piece (`scale/mod.rs:1-15`) |
| Project seed | `project.py:483,597` | A | `--seed` |
| Presets, with machine keys split from world keys | `presets.py:47-132` | M | TOML `[defaults]`. Machine keys live in `[run]` |
| `meld-world.json` provenance sidecar | `server.py:2203-2271` | A | `arnis_one_world.json` plus `metadata.json`; Meld 2 state keeps each selection's command |
| Per-cell status (`grid.json`) | `project.py:610-642` | O | Arnis keeps `jobs/<rect>_n<N>/done-<piece>.json`; Meld 2 keeps per-selection state (done) |

### 1.2 Queue and jobs

| Feature | Meld 1.9.9 source | → | How in Meld 2 |
|---|---|---|---|
| Render queue across projects: pause, stop, kill | `server.py:5590-5740` | M | **Done in Phase 1:** `queue::run`, `meld2 stop` |
| Parallel cells (worker pool) | `src/workers.py` | A | `--one-world-workers auto\|N` |
| Threads × workers under a CPU target | `governor.py:580-610` | A | `--threads` / `--cpu-target`. Meld 2 splits one budget across the jobs that can run, rebalanced at each start (Phase 2 ✔) |
| RAM admission gate | `governor.py:633-700` | A | `--ram-budget-mb`, split by Meld (Phase 2 ✔); `--one-world-workers auto` sizes workers within it |
| Adaptive governor, occupancy, learned history | `governor.py` (1,242 lines), `occupancy.py` | X | T3: a few fat workers win (2–6), and Arnis auto picks 1–6 |
| CPU start stagger | `project.py:198-206` | X | Not needed with the Arnis coordinator |
| Resume after a crash | `server.py:5170` | O | Arnis resumes pieces; Meld 2 resumes selections (done) |
| Regenerate a cell, region or suspect | `server.py:5154-5245` | O | A selection over that bbox, plus `--rebuild` (Phase 3 ✔) |
| Final check for missing regions, with retry | `finalcheck.py:66` | O | After each run, `--plan-units` counts the chunks still missing per built selection (Phase 3 ✔); retry is `--rebuild` |
| Child containment (Job Object, no console) | `childproc.py:34,69` | M | **Done:** `arnis.rs` Job Object per run |
| Keep the machine awake | `power.py:9,39` | M | Windows `SetThreadExecutionState` (Phase 2 ✔); `systemd-inhibit` / `caffeinate` (Phase 3 ✔) |
| Prefetch OSM and terrain before parallel cells | `prefetch.py`, `server.py:4915-4971` | A | `--prewarm`, `--prewarm-first` |
| End-of-run report (HTML/JSON, Gantt, CPU/RAM) | `runreport.py` | M | JSON from the notes: steps, shares, times, `done` totals, every piece record (Phase 3 ✔). HTML/Gantt: GUI |
| Auto-export after a run | `server.py:3321-3354` | O | A post-build job kind |

### 1.3 Server features

| Feature | Meld 1.9.9 source | → | How in Meld 2 |
|---|---|---|---|
| One-click Leaf server: catalog, install, EULA, start scripts, JVM sizing | `mcserver.py:41-293` | M | Port it |
| Paper | Leaf only (`LEAF_API`, `mcserver.py:41`); plugins already resolved for paper loaders (`:117`) | M | New: PaperMC exposes the same downloads API shape |
| Multiverse sub-worlds | `mcserver.py:17,49-50` | M | A natural fit for multi-world projects |
| Voxy server-side plugin | `mcserver.py:56` | M | LOD pregeneration itself is Arnis `--voxy-lod` (stock) |
| Server start, stop, console, command, backup | `server.py:7165-7276` | M | Port it |
| World border (vanilla) | `level_dat.py:103-123` | A | `--world-border` |
| Level name | `level_dat.py:55-91` | A | `--world-name` |
| WorldGuard `regions.yml` and Skript border/zones | `border.py` (930 lines) | M | **Rewrite.** `border.py:172-176` projects equirectangularly; One World is Web Mercator |
| Datapacks carried through the cell merge | `merge.py:208-222` | X | Not needed: One World writes in place, so there is no merge |
| zip / tar.zst export | `export.py:767-812` | M | Port it |
| Linear v1 export | `export.py:231-348` | X | B_Linear supersedes it for Leaf ≥ 1.21.11 (`12-BLINEAR-COMPARISON.md`) |
| B_Linear export via region-convert | `export.py:941-998` | O | `--region-format blinear` is refused with `--one-world`. Meld 2 converts after the build (§2) |

There is no upload. A grep for sftp/ftp/upload finds only the preset import and the UI file upload (`server.py:3926,7469`). Upload is listed as new work in §2.

### 1.4 Previews and maps

| Feature | Meld 1.9.9 source | → | How in Meld 2 |
|---|---|---|---|
| Leaflet map: draw or search an area | `web/index.html` (7,174 lines) | M | GUI (§3.2) |
| Cave zone map | `server.py:6628` | A | `--cave-zone-map` |
| Climate map | `server.py:6721` | A | `--climate-map` |
| Elevation and height preview | `server.py:6796`, `datapack.py` | M | GUI |
| Terrain tile proxy | `server.py:1630` | X | The GUI reads tiles directly |
| Map item in the world | `project.py:153` | A | `--map-item`; `--map-item-only` redraws it |
| Area preview PNG | (merge) | A | Area previews, stitched per job (`scale/mod.rs` `stitch_previews`) |
| Client previews of the tree mix and field texture | `web/index.html` | M | Port the JS |
| Floating status-bar HUD | `statusbar.py` (806 lines) | M | A small Tauri window |
| Live System/Build/Log rail | `web/index.html:456-491` | M | Dashboard |

### 1.5 Updater, tray, app shell

| Feature | Meld 1.9.9 source | → | How in Meld 2 |
|---|---|---|---|
| Update check (GitHub, 24 h cache) | `update.py:37-151` | M | `tauri-plugin-updater` |
| Staged self-update with sha256 and a smoke test | `updater.py:107-305` | M | Same plugin (signed) |
| Generator update (`Teddy563/arnis`) | `update.py:212-260`, `updater.py:405` | M | Pinned Arnis download (§3.6) |
| Tray app | `tray.py` | M | Tauri tray |
| Chromium `--app` window | `preview.py` | X | Tauri window |
| Single instance plus hand-off | `single_instance.py` | M | `tauri-plugin-single-instance` |
| Localhost API guard (Host, Origin, token) | `appguard.py:44-114` | M | `meld2 serve` token on every request, loopback included (Phase 3 ✔) |
| App log | `applog.py` | M | Done for jobs (`logs/<id>.log`) |
| Data dir (env, pointer file, portable) | `paths.py:16-29` | M | Done: `MELD2_HOME`, else the OS dir. Pointer file later |
| Diagnostic CLI (`--check`, `--arnis-caps`, `--print-arnis-cmd`) | `meld_app.py:105-190` | M | Done: `meld2 caps`, `meld2 arnis status`, `meld2 plan`. `print-cmd` later |
| PyInstaller packaging | `packaging/` | X | cargo / Tauri bundler |
| Console banner | `banner.py` | X | — |
| One-click source launcher | `meld_launch.py` | X | — |

### 1.6 Conversion

| Feature | Meld 1.9.9 source | → | How in Meld 2 |
|---|---|---|---|
| region-convert (Rust; mca, linear, B_Linear v2/v3) | `region-convert/src` | O | Reuse it as a crate (it is already Rust) |
| `meldconvert.py` CLI | `meldconvert.py:418-446` | M | `meld2 convert` |
| Native B_Linear generation | `project.py:343-354` | A | `--region-format blinear`, without One World only |
| Compress while generating, stream-and-free | `export.py:538-765` | X | Not in 2.0 |
| Export safety: disk preflight, verify, resumable manifest | `export.py:88-176` | M | Part of disk planning |

### 1.7 Benchmarking

| Feature | Meld 1.9.9 source | → | How in Meld 2 |
|---|---|---|---|
| `bench_scheduler.py`: legacy vs governor, with a determinism gate | `bench/bench_scheduler.py` | X | The governor is dropped |
| Bucharest A/B, contention sweep, accept protocol | `bench/ab_bucharest.py`, `contention_sweep.sh`, `accept_protocol.md` | X | Arnis's split-vs-single harness owns this now |
| CLI-contract tests against the generator | `tests/test_upstream_3_2_flags.py` | A | `--capabilities` plus Meld 2's golden command-line test |
| Hardware probe and recommendation | `server.py:6338` | A | `--one-world-workers auto` |
| CPU seconds and peak RSS per run | (telemetry) | A | NDJSON `done` and `piece` records |

### 1.8 Data, caches, generation settings

| Feature | Meld 1.9.9 source | → | How in Meld 2 |
|---|---|---|---|
| Geofabrik finder and `.pbf` bake | `geofabrik.py`, `osm_pack.py` | A | `--osm-pbf geofabrik` |
| OSM grid prefetch with a TTL | `prefetch.py`, `osm_grid.py` | A | Tile archive, `--prewarm` |
| Overture prewarm | `server.py:2106` | A | `--prewarm` |
| Elevation data packs (bulk download, repair) | `datapack.py` | A | `--prewarm`, local tile archive |
| One shared cache root | `server.py:101-110` | A | `ARNIS_CACHE_ROOT` = `<data>/cache` unless set (Phase 3 ✔) |
| Cache view and clear | `server.py:1246-1267` | M | Dashboard |
| Loot editor and presets | `server.py:6461-6513` | M | The editor writes JSON; Arnis reads `--loot-table` |
| Generation settings (112 keys in `default_settings`) | `project.py:18-386` | A | Mapped by `meld2/core/src/args.rs` (`OPTS`, 49 keys) |
| GPU cave density | `project.py:355-360` | X | Rejected in the audit |

---

## 2. What Arnis lacks for "server and scale"

| Need | Arnis 3.4 today (evidence) | Meld 2 provides | Phase |
|---|---|---|---|
| **Projects** | One bbox per command. The GUI keeps no project file | `project.toml` format 1: selections, each with its settings and its world | Phase 1 ✔ |
| **Multi-selection queue** | One job per process. One writer per One World (`arnis_one_world/owner.pid`) | A queue with `run.jobs` at once; selections on the same world are serialised | Phase 1 ✔ |
| **Resumable jobs across restarts** | A job resumes only if it is rerun with the same bbox and N (`scale/mod.rs:13-15`). Nothing records which jobs are pending | `state.json` per project: pending, running, stopped, failed or done, plus the command | Phase 1 ✔ |
| **Scheduling pieces across selections** | `--threads` and `--one-world-workers` are per process; nothing is shared across processes | An even split of `cpu_target` and `ram_budget_mb` between jobs (done). Rebalanced at each job start; workers sized by `--one-world-workers auto` from the piece count within the share | Phase 1 ✔ / Phase 2 ✔ |
| **Server integration** | None | Leaf and Paper staging, Multiverse world per selection world, Voxy, console and backups | Phase 4 |
| **Country bakes as jobs** | `--osm-pbf geofabrik` bakes inline inside the first job; `--prewarm` works per bbox | `[[bake]]` (a `--prewarm --osm-pbf` over the selections that read one extract) and `prewarm = true` (the selection's command with `--prewarm`), run ahead of the builds | Phase 3 ✔ |
| **Disk and size planning** | Prints nothing before it builds. `--plan-units` gives chunks per piece | `meld2 plan` and the start of each run: chunks still to build × 3.84 MB per region against free disk, +25 % and `run.min_free_mb`; refuse when short, warn when tight | Phase 2 ✔ |
| **Project dashboard** | None | `meld2 serve` (JSON API, SSE, a minimal page) and the GUI: worlds, selections, pieces, logs, disk | Phase 3 ✔ / Phase 5 |
| Polygon selections | bbox only | Cover the rings with piece-sized bboxes | Phase 3 ✔ |
| B_Linear for One Worlds | Refused with `--one-world` (`--region-format` help; `REMAINING.md`) | Convert after the build with region-convert | Phase 4 |
| Progress across pieces | Piece mode reports `progress` but it does not follow pieces: e2e showed **44.3 % at 2/16 pieces**. `done.chunks` counts only that run | Meld shows pieces done/of (done); totals summed per world | Phase 1 ✔ |
| Rebuilding with new settings | A rerun resumes by rect and N, whatever the settings | A partial job with changed settings is refused (done). `--rebuild` clears Arnis's job folder | Phase 1 ✔ / Phase 3 ✔ |
| Headless remote control | No daemon | `meld2 stop` (done); `meld2 serve` with a token | Phase 1 ✔ / Phase 3 ✔ |
| Parent death on Unix | The coordinator does not watch stdin; only pieces do (`scale/child.rs:75-82`) | Arnis in its own process group plus a `sh` pipe watchdog that kills the group when Meld's end of the pipe closes (Linux and macOS) | Phase 2 ✔ |
| Upload a built world | None (none in Meld 1 either) | Optional: rsync/SFTP of a finished world | rc / later |

---

## 3. Architecture

### 3.1 Shape

```
meld2/                      Cargo workspace, version 2.0.0-alpha.N (N = phase)
  core/   meld-core (lib)   project.rs   format-1 TOML model + validation
                            args.rs      settings → Arnis argv + required capabilities (one table)
                            arnis.rs     --version / --capabilities probe, spawn, Job Object / Unix watchdog kill
                            install.rs   Arnis lookup order, pinned download + sha256, version/caps gate
                            plan.rs      --plan-units parser, size estimate, free disk, verdict
                            progress.rs  NDJSON v1 parser
                            queue.rs     scheduler: jobs budget, one writer per world, stop, resume
                            state.rs     state.json in the data dir (atomic writes)
                            report.rs    JSON run report from the notes          (Phase 3)
                            import.rs    Meld 1 project.json / preset → TOML     (Phase 3)
  cli/    meld2 (bin)       run · status · stop · caps      (Phase 1)
                            plan · arnis status|install|path (Phase 2)
                            serve · import · run --rebuild  (Phase 3)
                            convert                         (Phase 4)
  gui/    (Phase 5)          Tauri 2 shell over meld-core, same web UI that `serve` hosts
```

- **Layout.** A top-level `meld2/` is right while the Python app has to keep working on this branch. At 2.0.0, move the workspace to the repo root and delete the Python tree; tag `v1.9.9` keeps it.
- **Headless first.** The CLI and `meld2 serve` are the product for servers. The GUI is a client of the same core. The GUI makes in-process calls; the browser goes over `serve` HTTP.

### 3.2 GUI toolkit: Tauri 2

| Option | For | Against |
|---|---|---|
| **Tauri 2 (recommended)** | Arnis already ships Tauri 2 (`Cargo.toml`: `tauri = "2"`), so the same stack and know-how. Meld's UI is 7,174 lines of HTML/JS with Leaflet, and a webview reuses it. The same pages can be served by `meld2 serve` to a browser on a headless server: one UI, two hosts. Official tray, single-instance and updater plugins replace `tray.py`, `single_instance.py` and `updater.py` | WebKitGTK on Linux (Arnis already handles EGL quirks); a JS layer stays |
| egui / eframe | Pure Rust, small | No Leaflet-class map widget; the whole UI is a rewrite; it does not serve a browser for headless use |
| Slint / iced | Native look, pure Rust | Same map and rewrite problem; no browser path |
| `serve` + browser only | Least code | No tray, no notifications, no single window: a regression from 1.x on desktops |

### 3.3 How Meld 2 drives Arnis (implemented in Phase 1)

| Concern | Mechanism |
|---|---|
| Spawn | `Command` with argv built by `args::build`. stdout is piped; stderr and non-record lines go to `logs/<selection>.log` |
| Capability gate | `arnis --capabilities` (one JSON line) is probed at start. Each setting names its capability in the `OPTS` table, and `args::require` refuses the job, naming what is missing. A missing line means "predates 3.4" |
| Always passed | `--bbox --output-dir <saves> --one-world --world-name <world> --progress json --no-update-check --unit-regions N` (default 4: pieces are what resumes) |
| Progress | `progress::parse` keeps lines starting `{"v":1,`, then reads phase, progress, piece, transfer, error and done; unknown types become `Other`; other versions are skipped |
| Budget across selections | Up to `run.jobs` processes; never two on one world. When a job starts, `slots` = the distinct worlds among running and queued jobs, at most `run.jobs`; the job gets `--threads = cores × cpu_target ÷ 100 ÷ slots`, `--ram-budget-mb = (run.ram_budget_mb or 80 % of free RAM) ÷ slots` and `--one-world-workers auto`, unless the selection sets its own. Arnis splits that again across its pieces. A running Arnis keeps its share (no CLI to change it) |
| Clean process tree | Windows: one Job Object per run with `KILL_ON_JOB_CLOSE`, the same pattern as Arnis's `scale/child.rs`. Stop calls `TerminateJobObject`; if Meld dies, the kernel closes the handle. Arnis's pieces are in its nested job. Unix (Phase 2): Arnis starts with `process_group(0)` beside `sh -c 'cat >/dev/null; kill -KILL -- -$0' <pgid>`, which has its own group and reads a pipe Meld holds. Meld's death (kill -9 included) or a stop closes it; the coordinator's death closes its pieces' stdin |
| Stop | `meld2 stop <project>` drops a `stop` file. The run polls it every 0.5 s, kills its jobs and marks them `stopped` |
| Resume | Done with the same command: skip. Partial with the same command: rerun, and Arnis skips the finished pieces. Partial with a changed command: refuse |

### 3.4 Project file and persistence

- **Project:** TOML, because server admins hand-edit it and keep comments. `format = 1` is required. A higher number is refused with "update Meld"; future migrations are functions from format N to N+1, run on load. Unknown keys are errors (`deny_unknown_fields`).
- **Settings:** Arnis flag names in snake case. Arnis validates values; Meld checks keys, types and capabilities. `extra_args` passes unmodelled flags through unchecked.
- **Settings Arnis refuses with One World** (`min_y`, `max_y`, `region_format`) are refused when the project is parsed (`y_bounds.rs:31`).
- **State:** JSON (machine-written) in the data dir. The data dir is `MELD2_HOME`, else `%LOCALAPPDATA%\Meld2`, else `$XDG_DATA_HOME/meld2` or `~/.local/share/meld2`.

  | Path in the data dir | Contents |
  |---|---|
  | `projects/<slug>-<fnv32(path)>/state.json` | per-selection state, written atomically |
  | `projects/<slug>-<fnv32(path)>/logs/` | one log per selection |
  | `projects/<slug>-<fnv32(path)>/stop` | the stop request file |
  | `projects/<slug>-<fnv32(path)>/run.lock` | held by the one running `meld2 run` (Phase 2) |
  | `projects/<slug>-<fnv32(path)>/reports/run-<unix>.json` | one report per run (Phase 3) |
  | `arnis/<version>/` | downloaded Arnis builds (Phase 2 ✔) |
  | `cache/` | the Arnis cache, via `ARNIS_CACHE_ROOT` (Phase 3 ✔) |
  | `workspace/<name>/project.toml` | projects `meld2 serve` serves, unless `--dir` (Phase 3) |

### 3.5 Bundling and updating Arnis (Phase 2 ✔)

1. The pin is in `meld2/core/src/install.rs`: `Teddy563/arnis` v3.4.0-beta.1 (becomes `louis-e/arnis` once upstream ships at-Scale), one asset per OS, SHA-256 recorded on 2026-10-07:

   | Asset | SHA-256 | Used on |
   |---|---|---|
   | `arnis-windows.exe` | `4218e2394707ff360b9743e44a567f6f0364bb2a1c9d76508cc2293011c68207` | Windows x86_64 |
   | `arnis-linux.tar.gz` (member `arnis-linux`) | `b457f88e0efd55d05808ed9f0ecb4bd6b90f030e383e8c5208385298ebbdc058` | Linux x86_64 |
   | `arnis-mac-universal.tar.gz` (member `arnis-mac-universal`) | `5d35ccde76c17d9803c2f51aca522410cd0f92794aeb0374bd4920d1987cdc64` | macOS |
   | `arnis-linux-appimage.tar.gz` | `29a02b22706ee03c61b632008f37f7e0bc804d9d998de38958ca8011be609dd2` | not used (needs FUSE) |

2. `meld2 arnis install`, or the first `run` that finds nothing, downloads to `<data>/arnis/<version>/`, verifies the hash, unpacks the tar.gz on Linux and macOS, then probes the result.
3. Resolution order: `--arnis`, the project's `arnis`, `MELD2_ARNIS`, `arnis(.exe)` next to the meld2 binary, the cached download, a fresh download. Every Arnis goes through the probe: version >= 3.4.0-beta.1 and the capabilities `progress-json unit-regions one-world-workers plan-units threads ram-budget`.
4. The release's Windows asset is a CI build whose hash differs from the local `work/release/arnis-3.4.0-beta.1.exe` (`6f94d3d1…`); both report 3.4.0-beta.1 with the same capabilities.

### 3.6 Migration from Meld 1.x

Meld 1 does have a saved format (verified):

| File | Contents | Source |
|---|---|---|
| `projects/<slug>/project.json` | `name`; `origin{lat,lon,locked}`; `settings` (112 keys); `elevation{min_m,max_m,seed,locked}`; `selection{bbox{south,west,north,east}, polygons}` | `project.py:479-520`, `paths.py:152` |
| `grid.json` | cell → status | |
| `projects/_org.json` | gallery order and folders | |
| Presets | JSON, `PRESET_SCHEMA = 1` | `presets.py:47` |
| `meld-world.json` | sidecar in each world | |

`meld2 import <meld1-data-dir>` (Phase 3 ✔) writes one `project.toml` per Meld 1 project; it prints every key as mapped, dropped on purpose or NOT MAPPED:

| Meld 1 key | Meld 2 |
|---|---|
| `selection.bbox` (or `polygons`) | one selection (polygons → covering bboxes) |
| `origin.lat/lon` | `origin = "lat,lon"` |
| `elevation.seed` | `seed` |
| `scale`, `ground_level`, `interior`, `overture`, `caves`, `bake_lighting`, `map_item`, `gamemode`, `snow_mode/y`, `field_scale`, `grass_texture`, `land_texture` | same name. `snow_mode = peaks` → `manual` from `snow_y`: One World refuses peaks (found in the Phase 3 e2e), so `snow_percent` is dropped |
| `fill_ground` | `fillground` |
| `terrain = false` | `mode = "geo-only"` |
| `buildings` | `buildings` (`false` → `--no-buildings`) |
| `road_detail_level` | `road_detail`; `auto` resolves to compact below 0.7, else clean (`arnis_cmd.py:466-470`) |
| `river_bed_v1` | `river_bed = "v1"` |
| `scatter_mode` | `rocks` / `bushes` |
| `field_mix`, `farm_crops`, `tree_size_weights` (dicts) | `"k=v,…"` strings |
| `job_size_regions` | `unit_regions` |
| `max_workers` | `workers` |
| `cpu_target_pct` | `[run] cpu_target` |
| `offline_elevation` | `offline`, plus `prewarm = true` so the caches are filled first |
| `overpass_url` | `overpass_url` |
| `native_region_format` | refused under One World; becomes a post-convert |
| governor, stagger, prefetch, sidecar, timer, `canonical_regions`, `seam_buffer_chunks`, `gpu_accel`, `mc_version`, height room keys | dropped, and the import lists them |

Meld 1 worlds use an equirectangular frame, and One World cannot extend them (`01-TECHNICAL-INTEGRATION.md`, risk table). An import therefore starts new worlds; the old ones stay playable.

---

## 4. Roadmap: Phases 1–5 → 2.0.0-beta.1 → 2.0.0

| Phase | Scope | Acceptance tests | h | Risks |
|---|---|---|---|---|
| **Phase 1** (`2.0.0-alpha.1`, done) | core + CLI: format-1 project model; settings→argv table with capability gate; spawn, NDJSON, Job Object kill; queue with job budget and one writer per world; `state.json` resume; `run / status / stop / caps` | fmt, clippy `-D warnings` and 12 unit tests green (parser, golden argv, progress on recorded NDJSON, resume decisions, fake-Arnis loop, tree kill); e2e against `arnis-3.4.0-beta.1.exe` (§5) | 30 | Arnis `progress` is not proportional in piece mode (worked around); the Unix coordinator does not die with Meld |
| **Phase 2** (`2.0.0-alpha.2`) scale and safety | `meld2 plan` (size and disk from `--plan-units`) ✔; budget rebalancing when a job ends ✔; pinned Arnis download and verify ✔; Unix parent-death ✔; one `run` per project (lock) ✔; keep the machine awake (Windows ✔). Moved to Phase 3: `--rebuild`, JSON run report, keep-awake on Linux/macOS | plan within ±25 % of actual bytes on 3 areas and 2 scales; a run refused when the disk is short; tampered download rejected; `kill -9 meld2` on Linux leaves 0 arnis; a second `run` refused; report lists every piece | 45 | bytes per chunk swings with caves and scale; GitHub rate limits |
| **Phase 3** (`2.0.0-alpha.3`, done) server mode and data jobs | `meld2 serve` (JSON API, token, localhost by default); job kinds `bake`/`prewarm` (country bakes; a `--prewarm-first` step before a big selection); from Phase 2: `--rebuild`, JSON run report, keep-awake on Linux/macOS; polygon selections; `meld2 import` from Meld 1; final check via `existing_chunks`; regenerate an area | Liechtenstein bake job, then the builds pass with `--offline`; import of a real 1.9.9 project and of 3 bundled presets; a curl-driven run over `serve`; country polygon with 0 missing chunks | 70 | Bake time on large countries (Austria ≈ 103 s single-threaded in arnis-tiles); polygon edge cases |
| **Phase 4** server integration and output | Leaf and Paper staging; Multiverse world per project world; Voxy plugin; start, stop, console, backup; WorldGuard/Skript borders recomputed in Web Mercator; B_Linear post-convert (region-convert crate); zip/tar.zst with preflight | Leaf boots a 2-world project (`Done (` marker); a B_Linear world loads in Leaf 1.21.11; WorldGuard ring within 1 block of `--world-border`; an export refused when the disk is short | 80 | In-game checks need a person (`REMAINING.md` › Needs the user); Leaf/Paper API drift; check the region-convert fork's licence |
| **Phase 5** desktop GUI | Tauri 2: dashboard, map editing of selections (port the Leaflet UI), per-piece live progress, tray, single instance, updater | GUI e2e on Windows; Playwright smoke test on the `serve` UI; tray stop and resume | 110 | Porting a 7k-line UI; WebKitGTK |
| **2.0.0-beta.1** | All five phases merged | everything above green together | — | — |
| **rc.1** (after the user's UI pass) | Linux and macOS CI; docs; migration guide; benchmark vs 1.9.9 on the same area | green on 3 OSes; wall time ≤ 1.9.9 on the gate area | 30 | macOS signing |
| **2.0.0** | Move `meld2/` to the root, retire Python, release | release assets install and run `caps` | 10 | — |
| **Total** | | | **≈ 375** | Arnis at Scale is not merged upstream yet: pin the fork release; the interface is CLI only, so swapping in upstream later is just a change to the pin |

---

## 5. Phase 1 as delivered

**Commits on `meld-2.0`** (not pushed):

| Commit | Contents |
|---|---|
| `5e5a449` | Workspace, versioned project file, command builder |
| `34c07e7` | NDJSON parser and fixtures |
| `78475ef` | Driver: probe and tree kill |
| `bbfb35a` | Queue and resume state |
| `daa456e` | CLI and README |
| `52f8101` | Refuse `min_y`/`max_y`/`region_format` up front |

**Gates:** `cargo fmt --check` passes. `cargo clippy --all-targets -- -D warnings` passes. `cargo test`: 12 passed.

**e2e** (`work/meld2-e2e/e2e.sh`, log `logs-e2e-full.txt`) runs two selections at once: Vaduz (4 pieces) and Schaan (16 pieces), with jobs = 2 and one-region pieces.

| Run | What happened |
|---|---|
| 1 | Progress streamed. `meld2 stop` after Schaan finished 2 pieces: both jobs `Stopped`, 0 arnis processes left |
| 2 | Resumed (Arnis "piece … skipped"). Then `taskkill /F /IM meld2.exe` (meld2 only, not `/T`): 2 arnis before the kill, 0 after; Vaduz `done`, Schaan left `running` 4/16 |
| 3 | Vaduz skipped; Schaan resumed at piece 5 and finished 16/16 |
| 4 | Both skipped |

Each world records exactly one area.

## 6. Phase 2 as delivered (`2.0.0-alpha.2`, local branch `meld-2.0-phase-2`)

**Commits on `meld-2.0`** (not pushed): see `git log meld-2.0-phase-1..meld-2.0-phase-2`.

| Commit | Contents |
|---|---|
| docs | This plan in the branch, Phases 1–5, version 2.0.0-alpha.2 |
| provisioning | Arnis lookup, pinned download + sha256, version/capability gate, `meld2 arnis status/install/path` |
| plan | `meld2 plan`, size and disk check before each run |
| budget | CPU/RAM split across the jobs that can run, rebalanced at each start, `--one-world-workers auto` |
| unix | Pipe watchdog: Arnis dies with Meld on Linux and macOS |
| lock | One run per project, Windows keep-awake |

**Gates:** `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` (Windows; also for `x86_64-unknown-linux-gnu` with ureq's TLS turned off in a scratch copy, since `ring` needs a Linux C compiler), `cargo test`: 19 passed on Windows. The Unix watchdog test compiles but has not run: no Linux host here (WSL and Docker are down).

**Plan accuracy** (chunks ÷ 1024 × 3.84 MB vs region files on disk): Schaan 18.9 MB est. / 20.8 MB actual (−9 %), Vaduz 0.84 / 0.97 MB (−13 %). Scale 1 only; the ±25 % acceptance on 3 areas × 2 scales is still open.

**e2e** (`work/meld2-e2e/e2e-phase2.sh`, log `logs-e2e-phase2.txt`): fresh `MELD2_HOME`, no `--arnis`, no `MELD2_ARNIS`, no bundled exe.

| Step | Result |
|---|---|
| `arnis status` before | "no Arnis found; `meld2 run` will download …" |
| `run` | downloaded `arnis-windows.exe`, verified `4218e239…`, printed the plan (Vaduz 4 pieces / 0.8 MB, Schaan 16 / 18.9 MB, disk ok), started both at 9 threads + 6109 MB each, built 2/2 |
| a second `run` during the first | refused: "already running in another meld2" |
| `arnis status` after | source "downloaded earlier", 3.4.0-beta.1 (ok), capabilities listed |
| `run` with `min_free_mb = 999999999` | refused: "not enough disk: ~20 MB to write …", exit 1 |
| 3 selections, 2 in world Schaan, jobs = 2 | Vaduz and Schaan at 9 threads / 6096 MB; once both ended, schaan-north started alone at 19 threads / 12193 MB |

**Left for Phase 3** besides its own scope: `--rebuild`; the JSON run report; keep-awake on Linux/macOS; running the watchdog test and `kill -9 meld2` on a Linux host; plan accuracy at a second scale and on 3 areas; a `--prewarm-first` step for big selections.

## 7. Phase 3 as delivered (`2.0.0-alpha.3`, local branch `meld-2.0-phase-3`)

**Commits on `meld-2.0`** (not pushed): see `git log meld-2.0-phase-2..meld-2.0-phase-3`.

| Commit | Contents |
|---|---|
| cache, awake | `ARNIS_CACHE_ROOT` = `<data>/cache`; `systemd-inhibit` / `caffeinate` during a run |
| data steps | `[[bake]]` and `prewarm = true`, run ahead of the builds as `bake:<id>` / `prewarm:<id>` |
| polygons | `polygon` selections → piece-sized covering bboxes `<id>-N` |
| rebuild, report | `run --rebuild[=a,b]`, `reports/run-<unix>.json`, final check via `--plan-units` |
| import | `meld2 import` of Meld 1 projects and presets, every key reported |
| serve | `meld2 serve`: JSON API, SSE, token, status page |
| fixes from e2e | snow `peaks` → `manual` on import (One World refuses peaks); a Geofabrik bake pins its extract with `osm_pbf_url` |
| docs | README, this plan, version 2.0.0-alpha.3 |

**Gates:** `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (26 passed on Windows: 24 core, 2 cli).

**e2e** (`work/meld2-e2e/e2e-phase3.sh` and `e2e-phase3-stop.sh`, output in `p3/`): fresh `MELD2_HOME`, all through `meld2 serve` and curl.

| Step | Result |
|---|---|
| no token / wrong token | 401, 401 |
| PUT a broken project / the Liechtenstein project | 400 `missing field name` / 200 |
| run, and run again | 202 / 409 `already running in this server` |
| run 1: bake, 2 prewarms, 2 builds with `--offline` | the bake downloaded `liechtenstein-latest.osm.pbf` and baked 57,509 elements in 0.1 s; both prewarms and both offline builds read that bake (one `Baked` line, six `Reading the bake`); schaan 16/16, vaduz 4/4; final check 0 chunks missing |
| first attempt, without `osm_pbf_url` | Geofabrik picked `alps` (2.3 GB) for the padded union. Led to the `osm_pbf_url` rule |
| `run?rebuild=vaduz` | "224 existing chunk(s) are replaced"; region files rewritten; schaan skipped |
| import the real 1.9.x project (Bucharest, scale 0.1) and run it over the API | 31 keys mapped, 57 dropped on purpose, 29 NOT MAPPED; 4 pieces, 64 regions, 61,732 chunks in 66 s, 242 MB of region files (plan said 232 MB), 0 missing |
| PUT it back with `workers = 1`, `run?rebuild=all`, `stop` after 8 s, `run` | stopped at 3/4 pieces, 0 Arnis left; the resume skipped 3 pieces and built 1 |
| SSE | 4,938 lines in the main run: notes plus Arnis's `piece`, `transfer`, `progress`, `phase` and `done` records |
| status page | opened in a browser: projects, steps, pieces, live log |

**Moves to Phase 4** besides its own scope:
- Meld 1 keys that are NOT MAPPED and have an Arnis flag: `body`, `voxy_lod`, `props`, `world_time`, `rotation`, `tree_realm`, `disable_height_limit`, `overture_source`, the facade keys.
- Polygon cells snapped to Arnis's piece lattice (needs the world origin).
- Country polygon acceptance on a real country.
- The Linux run of the watchdog, keep-awake and `kill -9` tests.
- Plan accuracy at a second scale.
- State only saves progress at piece events, so the page's bar moves per piece; live percentages are in the SSE stream (Phase 5 UI).
