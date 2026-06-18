# Meld experimental tools

Power-user tools that run Meld work from the command line, without the web UI or a running
server. They are kept separate from the main app on purpose: they write straight to your
world folder on disk and assume you know what you are doing.

They call the exact same internals the server uses (the Arnis command builder and the world
merge), so the result is identical to a UI Generate, just scripted and headless.

## Before you run anything

- Close Minecraft. An open world locks the region files and a merge will fail.
- Make sure the save drive is connected. If it is a flaky external drive, a write can be lost
  mid-merge. Each cell owns its own region files, so a failure never corrupts other cells, but
  the failed cell may need a re-run (the tools print the failed list at the end).

## headless.py

Run it from the `light-meld` folder. Pass a project slug (the folder name under `projects/`).

```
python experimental/headless.py scan-water  --project meld-world-8-2 --sample 24
python experimental/headless.py repair-gaps --project meld-world-8-2
python experimental/headless.py rerun-all   --project meld-world-8-2 --workers 12
```

### scan-water (read-only)

Samples region files and prints the surface water fraction of each. A residual water "wedge"
shows up as a region that is mostly water; a real sea or river delta at the map edge also reads
high, so treat a high value as something to look at, not proof of a bug. Safe to run any time.

### repair-gaps

Finds cells that are marked `merged` in the plan but whose region files are missing from the
CURRENT world folder, and regenerates plus re-merges only those. This happens when the project
was renamed or the save location changed: the world folder is new, but the plan still says those
cells are done, so a normal Generate skips them and they show up as gray voids in game. This
fills them without re-running the whole world.

### rerun-all

Regenerates every cell with the deployed `arnis.exe` and re-merges it. Use this to roll a
generator fix across an existing world (for example the 2.9.1 water wedge fix). It overwrites
every region, so it takes longer, but it is guaranteed complete. OSM and elevation are already
cached, so each cell is fast.

## Notes

- These read the project's own settings, origin, elevation lock, and seed, so regenerated cells
  line up with the rest of the world exactly (same heights, same building palette).
- They never touch other projects or the shared cache; they only write the one project's world.
- `scan-water` needs `nbtlib` (`pip install nbtlib`). The other actions need only what Meld
  already uses.
