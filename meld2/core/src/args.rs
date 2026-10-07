//! Selection settings to an Arnis command line, and the `--capabilities`
//! each one needs. Values are passed through as given: Arnis validates them,
//! Meld only checks the key and its type.

use crate::project::{Selection, Settings};
use anyhow::{bail, Result};
use std::path::Path;
use toml::Value;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// `true` adds the flag, `false` leaves it out.
    Switch,
    /// `false` adds the flag (`buildings = false` -> `--no-buildings`).
    Not,
    /// The flag followed by the value.
    Value,
}

struct Opt {
    key: &'static str,
    flag: &'static str,
    /// The `--capabilities` name it needs, `None` for flags stock Arnis 3.3 has.
    cap: Option<&'static str>,
    kind: Kind,
}

const fn opt(key: &'static str, flag: &'static str, cap: Option<&'static str>, kind: Kind) -> Opt {
    Opt {
        key,
        flag,
        cap,
        kind,
    }
}

use Kind::{Not, Switch, Value as Val};

/// Every setting a project may use, in the order they go on the command line.
#[rustfmt::skip]
const OPTS: &[Opt] = &[
    // world and terrain
    opt("scale", "--scale", None, Val),
    opt("ground_level", "--ground-level", None, Val),
    opt("mode", "--mode", None, Val),
    opt("height_multiplier", "--height-multiplier", None, Val),
    opt("origin", "--origin", Some("origin"), Val),
    opt("disable_height_limit", "--disable-height-limit", None, Switch),
    opt("aws_only_elevation", "--aws-only-elevation", None, Switch),
    opt("seed", "--seed", Some("seed"), Val),
    opt("climate_mode", "--climate-mode", Some("climate-mode"), Val),
    // objects
    opt("buildings", "--no-buildings", Some("no-buildings"), Not),
    opt("interior", "--interior", None, Val),
    opt("overture", "--overture", None, Val),
    opt("road_detail", "--road-detail", Some("road-detail"), Val),
    opt("legacy_trees", "--legacy-trees", None, Switch),
    opt("tree_realm", "--tree-realm", Some("tree-realm"), Val),
    opt("tree_size_weights", "--tree-size-weights", Some("tree-size-weights"), Val),
    opt("rocks", "--rocks", Some("rocks"), Switch),
    opt("rock_density", "--rock-density", Some("rocks"), Val),
    opt("bushes", "--bushes", Some("bushes"), Switch),
    opt("bush_density", "--bush-density", Some("bushes"), Val),
    opt("props", "--props", Some("props"), Val),
    opt("use_3d", "--no-3d", None, Not),
    opt("overture_source", "--overture-source", None, Val),
    opt("signage", "--signage", None, Val),
    opt("building_facades", "--building-facades", None, Switch),
    opt("facade_detail", "--facade-detail", None, Val),
    opt("facade_px", "--facade-px", None, Val),
    // The token is a credential: Arnis reads MAPILLARY_TOKEN from Meld's environment.
    opt("mapillary_facades", "--mapillary-facades", None, Val),
    opt("mapillary_facade_mode", "--mapillary-facade-mode", None, Val),
    // ground
    opt("fillground", "--fillground", None, Switch),
    opt("caves", "--caves", None, Switch),
    opt("cave_style", "--cave-style", Some("cave-style"), Val),
    opt("cave_ores", "--cave-ores", Some("cave-ores"), Val),
    opt("cave_seed", "--cave-seed", Some("cave-seed"), Val),
    opt("snow_mode", "--snow-mode", Some("snow-mode"), Val),
    opt("snow_percent", "--snow-percent", Some("snow-mode"), Val),
    opt("snow_y", "--snow-y", Some("snow-mode"), Val),
    opt("river_bed", "--river-bed", Some("river-bed"), Val),
    opt("water_detail", "--water-detail", Some("water-detail"), Val),
    opt("field_mix", "--field-mix", Some("field-mix"), Val),
    opt("farm_crops", "--farm-crops", Some("field-mix"), Val),
    opt("field_scale", "--field-scale", Some("field-mix"), Val),
    opt("grass_texture", "--grass-texture", Some("grass-texture"), Switch),
    opt("land_texture", "--land-texture", Some("land-texture"), Switch),
    // output
    opt("bake_lighting", "--bake-lighting", None, Switch),
    opt("voxy_lod", "--voxy-lod", None, Switch),
    opt("map_item", "--map-item", None, Val),
    opt("world_border", "--world-border", Some("world-border"), Switch),
    opt("gamemode", "--gamemode", None, Val),
    opt("world_time", "--world-time", None, Val),
    // data
    opt("offline", "--offline", Some("offline"), Switch),
    opt("prewarm_first", "--prewarm-first", Some("prewarm"), Switch),
    opt("osm_pbf", "--osm-pbf", Some("osm-pbf"), Val),
    opt("osm_pbf_url", "--osm-pbf-url", Some("osm-pbf"), Val),
    opt("osm_tiles_url", "--osm-tiles-url", Some("local-tile-archive"), Val),
    opt("overpass_url", "--overpass-url", Some("overpass-url"), Val),
    // process
    opt("workers", "--one-world-workers", Some("one-world-workers"), Val),
    opt("threads", "--threads", Some("threads"), Val),
    opt("cpu_target", "--cpu-target", Some("cpu-target"), Val),
    opt("ram_budget_mb", "--ram-budget-mb", Some("ram-budget"), Val),
    opt("max_downloads", "--max-downloads", Some("max-downloads"), Val),
];

/// Keys handled outside `OPTS`.
const UNIT_REGIONS: &str = "unit_regions";
/// `prewarm = true`: before the build, a step runs the same command with
/// `--prewarm`, filling the caches (and the `.osm.pbf` bake) so the build reads disk.
pub const PREWARM: &str = "prewarm";
/// Raw arguments appended last, unchecked: the way to reach a flag Meld does not model.
const EXTRA_ARGS: &str = "extra_args";

/// Piece size when a project does not set one. Pieces are what makes a job resumable.
pub const DEFAULT_UNIT_REGIONS: i64 = 4;

/// Every key a selection may set, with whether it is a switch, for forms.
pub fn keys() -> Vec<(&'static str, bool)> {
    let mut keys: Vec<_> = OPTS.iter().map(|o| (o.key, o.kind != Val)).collect();
    keys.extend([(UNIT_REGIONS, false), (PREWARM, true)]);
    keys
}

/// Piece size, in regions per side.
pub fn unit_regions(settings: &Settings) -> i64 {
    settings
        .get(UNIT_REGIONS)
        .and_then(Value::as_integer)
        .unwrap_or(DEFAULT_UNIT_REGIONS)
}

/// Checks keys and value types.
pub fn check(settings: &Settings) -> Result<()> {
    for (key, value) in settings {
        let ok = match key.as_str() {
            UNIT_REGIONS => value.as_integer().is_some(),
            PREWARM => value.is_bool(),
            EXTRA_ARGS => value
                .as_array()
                .is_some_and(|a| a.iter().all(Value::is_str)),
            // Every Meld 2 selection builds a One World, which fixes its own
            // height and region format (Arnis y_bounds.rs, --region-format help)
            // and is Earth at rotation 0 (validate_args). B_Linear comes from a
            // conversion after the build (`[server] format = "blinear"`).
            "min_y" | "max_y" | "region_format" | "rotation" | "body" => {
                bail!("{key}: Arnis 3.4 refuses it with --one-world, which Meld 2 builds")
            }
            _ => match OPTS.iter().find(|o| o.key == key) {
                Some(o) if o.kind == Val => scalar(value).is_some(),
                Some(_) => value.is_bool(),
                None => bail!("unknown setting {key:?}"),
            },
        };
        if !ok {
            bail!("setting {key:?} has the wrong type ({value})");
        }
    }
    Ok(())
}

fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Integer(i) => Some(i.to_string()),
        Value::Float(f) => Some(f.to_string()),
        Value::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

/// What the scheduler hands one job of a shared budget. A selection's own
/// `threads`, `cpu_target`, `ram_budget_mb` or `workers` wins over it.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
pub struct Share {
    pub threads: Option<u32>,
    pub ram_budget_mb: Option<u64>,
    /// `--one-world-workers auto`: Arnis sizes its pieces-at-once from the
    /// piece count and these threads and memory.
    pub workers_auto: bool,
}

/// One Arnis run: its arguments and the capabilities they need.
#[derive(Debug, PartialEq)]
pub struct Invocation {
    pub args: Vec<String>,
    pub caps: Vec<&'static str>,
}

/// The command line that builds `sel` into its One World under `saves`.
pub fn build(sel: &Selection, settings: &Settings, saves: &Path, share: Share) -> Invocation {
    let [s, w, n, e] = sel.bbox;
    let units = unit_regions(settings);
    let mut args: Vec<String> = vec![
        "--bbox".into(),
        format!("{s},{w},{n},{e}"),
        "--output-dir".into(),
        saves.display().to_string(),
        "--one-world".into(),
        "--world-name".into(),
        sel.world.clone(),
        "--progress".into(),
        "json".into(),
        "--no-update-check".into(),
        "--unit-regions".into(),
        units.to_string(),
    ];
    let mut caps = vec!["progress-json", "unit-regions"];
    let mut add = |flag: &str, value: Option<String>, cap: Option<&'static str>| {
        args.push(flag.into());
        args.extend(value);
        if let Some(c) = cap.filter(|c| !caps.contains(c)) {
            caps.push(c);
        }
    };
    for o in OPTS {
        let Some(v) = settings.get(o.key) else {
            continue;
        };
        match o.kind {
            Switch if v.as_bool() == Some(true) => add(o.flag, None, o.cap),
            Not if v.as_bool() == Some(false) => add(o.flag, None, o.cap),
            Val => add(o.flag, scalar(v), o.cap),
            _ => {}
        }
    }
    if share.workers_auto && !settings.contains_key("workers") {
        let auto = Some("auto".to_string());
        add("--one-world-workers", auto, Some("one-world-workers"));
    }
    let own_threads = settings.contains_key("threads") || settings.contains_key("cpu_target");
    if let Some(t) = share.threads.filter(|_| !own_threads) {
        add("--threads", Some(t.to_string()), Some("threads"));
    }
    if let Some(mb) = share
        .ram_budget_mb
        .filter(|_| !settings.contains_key("ram_budget_mb"))
    {
        add("--ram-budget-mb", Some(mb.to_string()), Some("ram-budget"));
    }
    if let Some(extra) = settings.get(EXTRA_ARGS).and_then(Value::as_array) {
        args.extend(extra.iter().filter_map(Value::as_str).map(String::from));
    }
    Invocation { args, caps }
}

/// The prewarm of `inv`: the same options plus `--prewarm`, which fills the
/// caches (and bakes an `--osm-pbf` extract) for every piece and writes no
/// world. Arnis refuses `--prewarm` with `--offline`, so that one goes.
pub fn prewarm(mut inv: Invocation) -> Invocation {
    inv.args
        .retain(|a| a != "--offline" && a != "--prewarm-first");
    inv.args.push("--prewarm".into());
    if !inv.caps.contains(&"prewarm") {
        inv.caps.push("prewarm");
    }
    inv
}

/// A bake: Arnis cuts `osm_pbf` for `bbox` once, and every later run with
/// that `--osm-pbf` inside the area reads the cut. It is a `--prewarm` with
/// the other sources off (flat ground, no Overture, canopy or 3D), so it
/// fetches OSM and land cover only, and writes no world.
pub fn bake(bbox: [f64; 4], osm_pbf: &str, url: Option<&str>, share: Share) -> Invocation {
    let [s, w, n, e] = bbox;
    let mut args: Vec<String> = [
        "--bbox",
        &format!("{s},{w},{n},{e}"),
        "--osm-pbf",
        osm_pbf,
        "--prewarm",
        "--mode",
        "geo-only",
        "--overture",
        "false",
        "--canopy-height",
        "false",
        "--no-3d",
        "--progress",
        "json",
        "--no-update-check",
    ]
    .map(String::from)
    .to_vec();
    if let Some(url) = url {
        args.extend(["--osm-pbf-url".into(), url.into()]);
    }
    let mut caps = vec!["progress-json", "osm-pbf", "prewarm"];
    if let Some(t) = share.threads {
        args.extend(["--threads".into(), t.to_string()]);
        caps.push("threads");
    }
    Invocation { args, caps }
}

/// Fails, naming them, when Arnis lacks capabilities the run needs.
pub fn require(inv: &Invocation, available: &[String]) -> Result<()> {
    let missing: Vec<_> = inv
        .caps
        .iter()
        .filter(|c| !available.iter().any(|a| a == *c))
        .collect();
    if !missing.is_empty() {
        bail!(
            "this Arnis lacks {missing:?}; use Arnis 3.4 (Arnis at Scale) or drop those settings"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Project;

    const PROJECT: &str = r#"
format = 1
name = "Golden"
output = "saves"

[defaults]
scale = 0.5
buildings = false
caves = true
cave_style = "vanilla"
snow_mode = "peaks"
snow_percent = 8.5
rocks = true
bushes = false
map_item = false
workers = "auto"

[[selection]]
id = "a"
bbox = [47.139, 9.52, 47.141, 9.523]
world = "Alps World"
settings = { unit_regions = 2, seed = 42, extra_args = ["--debug"] }

[[selection]]
id = "b"
bbox = [47.15, 9.52, 47.16, 9.53]
world = "Alps World"
settings = { threads = 3 }
"#;

    /// The exact command lines, so any change to them is a visible diff.
    #[test]
    fn golden_command_lines() {
        let p = Project::parse(PROJECT).unwrap();
        let share = Share {
            threads: Some(10),
            ram_budget_mb: Some(4096),
            workers_auto: true,
        };
        let got: Vec<String> = p
            .selections
            .iter()
            .map(|s| {
                let inv = build(s, &p.settings_for(s), Path::new("saves"), share);
                format!("{}\ncaps: {}", inv.args.join("\n"), inv.caps.join(","))
            })
            .collect();
        let got = got.join("\n\n") + "\n";
        let golden = include_str!("../tests/fixtures/golden-args.txt").replace("\r\n", "\n");
        assert_eq!(got, golden, "\n--- got ---\n{got}");
    }

    #[test]
    fn missing_capabilities_are_named() {
        let p = Project::parse(PROJECT).unwrap();
        let s = &p.selections[0];
        let inv = build(s, &p.settings_for(s), Path::new("saves"), Share::default());
        let stock: Vec<String> = ["progress-json", "unit-regions"].map(String::from).to_vec();
        let err = require(&inv, &stock).unwrap_err().to_string();
        assert!(err.contains("cave-style") && err.contains("seed"), "{err}");
        let all: Vec<String> = inv.caps.iter().map(|c| c.to_string()).collect();
        require(&inv, &all).unwrap();
    }
}
