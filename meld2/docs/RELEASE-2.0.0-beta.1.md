# Meld 2.0.0-beta.1

Meld 2 is a rewrite of Meld in Rust. It turns real places into Minecraft worlds through **Arnis at Scale** (Arnis 3.4), and it can host them on a Leaf or Paper server. This is the first beta: everything is in place, and it now needs testing on more machines.

## What Meld 2 does

- **Projects.** A project is a set of selections on the map, rectangles or polygons. Each selection has its own settings and builds into a named world. Selections that share a world extend it one after another. Selections in different worlds build at the same time.
- **Arnis at Scale's options.** Cave style, ores and biomes, Cell Size and Selection Snap, Bake CPU Usage, tree realm, sizes and packs, fields, grass and land texture, props, loot table and more, under the same names as in Arnis.
- **Builds that resume.** Arnis builds every world in pieces. If you stop a run, close Meld or the machine goes down, the next run skips what is already built and carries on.
- **A plan before each build.** Meld asks Arnis how many pieces and chunks the build has and estimates its size on disk. A run that would not fit on the disk is refused.
- **Your machine's budget.** Meld splits the CPU and RAM you allow between the jobs that run. The machine does not sleep while it builds.
- **Country-sized data.** A *bake* cuts a Geofabrik or local `.osm.pbf` extract once. *Prewarm* fills the caches first, so the build itself can run offline.
- **B_Linear.** Meld can convert a finished world to B_Linear for Leaf 1.21.11 or newer. It checks a sample of the converted regions against the source before it swaps the folder in.
- **A server for your worlds.** Meld sets up a Leaf or Paper server: it downloads and verifies the jar and the plugins, writes the config, adds Multiverse for a second world and draws WorldGuard regions for every selection. You can start and stop the server, read its console and send it commands. A world can also be backed up as a zip, after a disk check.
- **Meld 1 projects.** `meld2 import` converts Meld 1 projects and presets. Each imported project starts a new world, and your Meld 1 worlds stay playable.

## Three ways to use it

- **The desktop app (Meld).** The layout is Meld 1's, including its animated wordmark: build status and workers on the left, the map in the middle, and every setting on the right. The settings use the new Arnis's sections, labels, switches and preview pictures (World, Generation, Terrain & Nature, …, Extra Features with Meld Generation and Caves & Water, OSM Data Source), so the names match between the two apps. Draw selections on the map. Each setting applies to the whole project or to one selection. Then plan, generate, stop and resume. The progress bars follow Arnis's live percentages, piece by piece. Closing the window during a run or while the server runs hides Meld to the tray, and the run keeps going. Quit from the tray.
- **The command line (`meld2`).** Use `run`, `plan`, `status`, `stop`, `import`, `convert`, `export`, `server setup|start|stop|status|send`, `arnis status|install` and `caps`.
- **Headless (`meld2 serve`).** The same page and JSON API run on a server, with a token on every request.

## How it uses Arnis

Meld does not change Arnis. It drives the stock Arnis at Scale command line: one Arnis process per selection, with One World, pieces and JSON progress. On first use Meld downloads the pinned release, `Teddy563/arnis` v3.4.0-beta.1, and checks its SHA-256. Use the Arnis panel in the app, or run `meld2 arnis install`. Meld refuses an Arnis older than 3.4.0-beta.1, or one that lacks a capability Meld needs, and says what to do about it. The download is not bundled inside the app. That keeps one verified copy shared by the app, the CLI and the server, and keeps the installers small.

## Security

- **The token.** Every request to the API needs the token, on loopback too. The desktop app picks a fresh token at every start and serves only on 127.0.0.1.
- **Programs Meld will run.** A project edited through the app or the API may only name an Arnis or a Java that Meld installed or found by itself, such as `JAVA_HOME`, the Modrinth app's runtimes or `java` on PATH. To use another one, add its path to `trusted-executables.txt` in Meld's data folder. The API never writes that file.

## Downloads

| File | For |
|---|---|
| `Meld-2.0.0-beta.1-windows-x86_64-setup.exe` | the desktop app, Windows 10/11 |
| `Meld-2.0.0-beta.1-macos-universal.dmg` | the desktop app, macOS (Intel and Apple silicon) |
| `Meld-2.0.0-beta.1-linux-x86_64.AppImage`, `.deb` | the desktop app, Linux |
| `meld2-2.0.0-beta.1-<os>.zip` / `.tar.gz` | the `meld2` command line |

The builds are unsigned. On Windows, SmartScreen asks on first run: choose *More info* → *Run anyway*. On macOS, open *System Settings → Privacy & Security* and choose *Open Anyway*.

## Known limits

- **No Linux runtime tests yet.** Linux builds compile in CI, but nobody has run them yet. That includes process cleanup when Meld is killed, keep-awake and the desktop app on WebKitGTK. macOS is in the same position.
- **No client join test yet.** The server boots with the built worlds and loads them, and WorldGuard reads Meld's regions. Nobody has joined it with a Minecraft client yet, including on a B_Linear world.
- **The map needs the internet.** It loads OpenStreetMap tiles and Leaflet from public CDNs. So does Arnis, unless you bake or prewarm first.
- **First use needs the internet** too, to download Arnis.
- **The UI is a beta.** It follows Meld 1's layout with Arnis at Scale's look, and it will change before 2.0.0. Option pictures are Arnis's static ones, not live previews. There is no in-app updater yet, and no CPU/RAM graph.
- **Quitting from the tray** stops any run (it resumes next time) and kills a running Minecraft server without saving it. Stop the server first.
- **A world backup** taken while the server runs is only as fresh as the last save. Send `save-all flush` first.
- The Windows build was tested end to end. Paper and the Voxy server plugin have not been run.

## Coming later

The user's UI pass leads to 2.0.0-rc.1, together with Linux and macOS testing, a migration guide and a benchmark against 1.9.9. In 2.0.0 the Rust workspace moves to the repository root and the Python app is retired. Meld 1.9.9 (tag `v1.9.9`) stays available.
