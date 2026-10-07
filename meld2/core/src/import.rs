//! `meld2 import`: a Meld 1.x `project.json` (or preset) to a Meld 2 project
//! file, key by key as `docs/PLAN.md` §3.6 maps them. Meld 1 worlds are
//! equirectangular and One World cannot extend them, so an import starts a
//! new world; the old one stays playable.

use crate::project::Project;
use anyhow::{bail, Context, Result};
use serde_json::{Map, Value as Json};
use toml::{Table, Value};

/// A converted project or preset and what became of each Meld 1 key.
#[derive(Debug, Default)]
pub struct Imported {
    /// The Meld 2 TOML: a whole project, or a `[defaults]`/`[run]` fragment for a preset.
    pub toml: String,
    pub mapped: Vec<String>,
    /// Keys Meld 2 leaves out on purpose (the governor, cells, merge, export, server, ...).
    pub dropped: Vec<String>,
    /// Keys with no Meld 2 counterpart yet.
    pub unmapped: Vec<String>,
    pub notes: Vec<String>,
}

/// Dropped on purpose: Meld 1's scheduler, cell grid and merge, which Arnis
/// at Scale replaces, and what One World fixes itself. Export and server
/// keys (`export_*`, `server_*`) are Phase 4 and dropped by prefix.
const DROPPED: &[&str] = &[
    // the governor and its stagger, the cell prefetch, timers and sidecars
    "governor_mode",
    "governor_history",
    "governor_max_workers",
    "worker_autoscale",
    "ram_headroom_mb",
    "flush_threads_cap",
    "min_threads_per_worker",
    "cpu_stagger_seconds",
    "cpu_stagger_enabled",
    "cpu_stagger_adaptive",
    "prefetch_enabled",
    "prefetch_margin_m",
    "prefetch_tile_km2",
    "prefetch_concurrency",
    "prefetch_terrain",
    "datapack_tile_concurrency",
    "osm_bake_workers",
    "osm_cache_ttl_days",
    "osm_sidecars",
    "parse_fast_json",
    "phase2_timers",
    "timeout",
    "stream_to_disk",
    // cells and the merge
    "canonical_regions",
    "seam_buffer_chunks",
    "prune_cell_after_merge",
    "master_world_dir",
    "origin_corner",
    "tile_invariant_rendering",
    "elevation_mode",
    // rejected or fixed by One World
    "gpu_accel",
    "mc_version",
    "height_headroom",
    "height_underroom",
    "world_min_y",
    "world_max_y",
    "native_blinear_level",
];

/// A Meld 1 `project.json` (and its gallery folder, if any) as a Meld 2 project.
pub fn project(json: &Json, folder: Option<&str>) -> Result<Imported> {
    let name = json["name"].as_str().unwrap_or("Meld World").to_string();
    let empty = Map::new();
    let settings = json["settings"].as_object().unwrap_or(&empty);
    let mut out = Imported::default();
    let (mut defaults, run) = convert(settings, &mut out);

    if let (Some(lat), Some(lon)) = (
        json["origin"]["lat"].as_f64(),
        json["origin"]["lon"].as_f64(),
    ) {
        defaults.insert("origin".into(), Value::String(format!("{lat},{lon}")));
        out.mapped.push("origin".into());
    }
    if let Some(seed) = json["elevation"]["seed"].as_i64() {
        defaults.insert("seed".into(), Value::Integer(seed));
        out.mapped.push("elevation.seed".into());
    }
    out.dropped
        .push("elevation.min_m/max_m (One World pins elevation)".into());

    let sel = &json["selection"];
    let mut selection = Table::new();
    selection.insert("id".into(), "area".into());
    let rings: Vec<Value> = sel["polygons"]
        .as_array()
        .map(|rings| {
            rings
                .iter()
                .filter_map(|r| {
                    let pts: Vec<Value> = r
                        .as_array()?
                        .iter()
                        .filter_map(|p| Some([p[0].as_f64()?, p[1].as_f64()?]))
                        .map(|p| Value::Array(p.map(Value::Float).to_vec()))
                        .collect();
                    (pts.len() >= 3).then_some(Value::Array(pts))
                })
                .collect()
        })
        .unwrap_or_default();
    if !rings.is_empty() {
        selection.insert("polygon".into(), Value::Array(rings));
        out.mapped.push("selection.polygons".into());
    } else {
        let b = &sel["bbox"];
        let bbox = ["south", "west", "north", "east"].map(|k| b[k].as_f64());
        let Some(bbox) = bbox.iter().copied().collect::<Option<Vec<f64>>>() else {
            bail!("no selection saved in this project (selection.bbox)");
        };
        selection.insert(
            "bbox".into(),
            Value::Array(bbox.into_iter().map(Value::Float).collect()),
        );
        out.mapped.push("selection.bbox".into());
    }
    selection.insert("world".into(), Value::String(name.clone()));

    let mut doc = Table::new();
    doc.insert("format".into(), Value::Integer(crate::project::FORMAT));
    doc.insert("name".into(), Value::String(name.clone()));
    doc.insert("output".into(), "saves".into());
    doc.insert("run".into(), Value::Table(run));
    doc.insert("defaults".into(), Value::Table(defaults));
    doc.insert(
        "selection".into(),
        Value::Array(vec![Value::Table(selection)]),
    );
    out.notes.push(format!(
        "starts a new One World {name:?}: a Meld 1 world cannot be extended (its frame is equirectangular); the old world stays playable"
    ));
    if let Some(f) = folder {
        out.notes
            .push(format!("Meld 1 gallery folder {f:?} is not carried"));
    }
    let header = header(&name, &out);
    out.toml = header + &toml::to_string(&doc)?;
    Project::parse(&out.toml).context("the converted project does not load")?;
    Ok(out)
}

/// A Meld 1 preset (`"meld_preset": 1`) as a `[defaults]` (and `[run]`) fragment.
pub fn preset(json: &Json) -> Result<Imported> {
    if json["meld_preset"].as_i64() != Some(1) {
        bail!("not a Meld 1 preset (meld_preset = 1)");
    }
    let name = json["name"].as_str().unwrap_or("preset").to_string();
    let empty = Map::new();
    let mut out = Imported::default();
    let (defaults, run) = convert(json["settings"].as_object().unwrap_or(&empty), &mut out);
    let mut doc = Table::new();
    if !run.is_empty() {
        doc.insert("run".into(), Value::Table(run));
    }
    doc.insert("defaults".into(), Value::Table(defaults));
    out.toml = header(&name, &out)
        + "# A preset: paste these tables into a project file.\n"
        + &toml::to_string(&doc)?;
    // The fragment must load as part of a project.
    let probe = format!(
        "format = 1\nname = \"p\"\noutput = \"saves\"\n{}\n[[selection]]\nid = \"a\"\nbbox = [1.0, 2.0, 3.0, 4.0]\nworld = \"w\"\n",
        out.toml
    );
    Project::parse(&probe).context("the converted preset does not load")?;
    Ok(out)
}

fn header(name: &str, out: &Imported) -> String {
    let mut h = format!("# Imported from Meld 1: {name}\n");
    if !out.unmapped.is_empty() {
        h += &format!(
            "# Not carried (no Meld 2 setting yet): {}\n",
            out.unmapped.join(", ")
        );
    }
    h + "\n"
}

/// Meld 1 settings to Meld 2 `[defaults]` and `[run]`, sorting every key
/// into mapped, dropped or unmapped.
fn convert(s: &Map<String, Json>, out: &mut Imported) -> (Table, Table) {
    let (mut d, mut run) = (Table::new(), Table::new());
    let num = |k: &str| {
        s.get(k)
            .and_then(|v| v.as_f64().or_else(|| v.as_str()?.trim().parse().ok()))
    };
    let flag = |k: &str| s.get(k).and_then(Json::as_bool);
    let terrain = flag("terrain").unwrap_or(true);
    let scale = num("scale").unwrap_or(1.0);
    for (key, v) in s {
        let k = key.as_str();
        let mapped = match k {
            "scale" | "ground_level" | "field_scale" | "snow_percent" | "snow_y" => {
                let n = num(k);
                let keep = match k {
                    "field_scale" => n.is_some_and(|f| f != 100.0),
                    // One World has no `peaks` (below), so no share of the relief either.
                    "snow_percent" => false,
                    "snow_y" => {
                        terrain
                            && ["manual", "peaks"]
                                .map(Json::from)
                                .contains(s.get("snow_mode").unwrap_or(&Json::Null))
                    }
                    _ => true,
                };
                if let Some(n) = n.filter(|_| keep) {
                    let int = k != "scale" && k != "snow_percent";
                    let n = if k == "field_scale" {
                        n.clamp(25.0, 400.0)
                    } else {
                        n
                    };
                    d.insert(
                        k.into(),
                        if int {
                            Value::Integer(n as i64)
                        } else {
                            Value::Float(n)
                        },
                    );
                }
                n.is_some()
            }
            "interior" | "overture" | "caves" | "bake_lighting" | "map_item" | "grass_texture"
            | "land_texture" => {
                if let Some(b) = v.as_bool() {
                    d.insert(k.into(), Value::Boolean(b));
                }
                v.is_boolean()
            }
            "buildings" | "fill_ground" | "offline_elevation" => {
                let to = match k {
                    "fill_ground" => "fillground",
                    "offline_elevation" => "offline",
                    _ => k,
                };
                if let Some(b) = v.as_bool() {
                    d.insert(to.into(), Value::Boolean(b));
                    if to == "offline" && b {
                        // Meld 1 filled its data pack first; a prewarm step does that here.
                        d.insert("prewarm".into(), Value::Boolean(true));
                        out.notes.push("offline_elevation: --offline needs filled caches, so prewarm = true runs a cache step before the build".into());
                    }
                }
                v.is_boolean()
            }
            "terrain" => {
                if !terrain {
                    d.insert("mode".into(), "geo-only".into());
                }
                true
            }
            "gamemode" => {
                let g = v.as_str().unwrap_or("").to_lowercase();
                if ["survival", "creative", "spectator"].contains(&g.as_str()) {
                    d.insert(k.into(), Value::String(g));
                }
                true
            }
            "snow_mode" => {
                let m = v.as_str().unwrap_or("").to_lowercase();
                if terrain && m == "peaks" {
                    // Arnis refuses peaks with One World: each area's relief differs.
                    d.insert(k.into(), "manual".into());
                    out.notes.push(format!(
                        "snow_mode peaks: One World refuses it, so snow lies from snow_y = {} up (manual); snow_percent is not carried",
                        num("snow_y").unwrap_or(80.0)
                    ));
                    if num("snow_y").is_none() {
                        d.insert("snow_y".into(), Value::Integer(80));
                    }
                } else if terrain && ["off", "realistic", "manual"].contains(&m.as_str()) {
                    d.insert(k.into(), Value::String(m));
                }
                true
            }
            "road_detail_level" => {
                let mut rd = v.as_str().unwrap_or("auto").trim().to_lowercase();
                if rd == "auto" || rd.is_empty() {
                    rd = if scale < 0.7 { "compact" } else { "clean" }.into();
                }
                // `max` is Arnis's own default.
                if rd == "compact" || rd == "clean" {
                    d.insert("road_detail".into(), Value::String(rd));
                }
                true
            }
            "river_bed_v1" => {
                if v.as_bool() == Some(true) {
                    d.insert("river_bed".into(), "v1".into());
                }
                true
            }
            "scatter_mode" | "rocks" | "bushes" => {
                let mut mode = s
                    .get("scatter_mode")
                    .and_then(Json::as_str)
                    .unwrap_or("")
                    .to_lowercase();
                if !["none", "rocks", "bushes", "both"].contains(&mode.as_str()) {
                    let r = flag("rocks") == Some(true);
                    let b = flag("bushes") == Some(true);
                    mode = match (r, b) {
                        (true, true) => "both",
                        (true, false) => "rocks",
                        (false, true) => "bushes",
                        _ => "none",
                    }
                    .into();
                }
                if mode == "rocks" || mode == "both" {
                    d.insert("rocks".into(), Value::Boolean(true));
                }
                if mode == "bushes" || mode == "both" {
                    d.insert("bushes".into(), Value::Boolean(true));
                }
                true
            }
            "field_mix" | "farm_crops" | "tree_size_weights" => {
                if let Some(spec) = v.as_object().and_then(|m| mix(k, m)) {
                    d.insert(k.into(), Value::String(spec));
                }
                v.is_object()
            }
            "job_size_regions" | "max_workers" => {
                let to = if k == "max_workers" {
                    "workers"
                } else {
                    "unit_regions"
                };
                // 0 workers is Meld 1's "auto", which Meld 2 passes anyway.
                if let Some(n) = num(k).filter(|n| *n >= 1.0) {
                    d.insert(to.into(), Value::Integer(n as i64));
                }
                num(k).is_some()
            }
            "cpu_target_pct" => {
                if let Some(n) = num(k) {
                    run.insert(
                        "cpu_target".into(),
                        Value::Integer(n.clamp(10.0, 100.0) as i64),
                    );
                }
                num(k).is_some()
            }
            "overpass_url" => {
                let urls = match v {
                    Json::String(u) => u.clone(),
                    Json::Array(a) => a
                        .iter()
                        .filter_map(Json::as_str)
                        .collect::<Vec<_>>()
                        .join(","),
                    _ => String::new(),
                };
                if !urls.trim().is_empty() {
                    d.insert(k.into(), Value::String(urls.trim().into()));
                }
                true
            }
            "native_region_format" => {
                if v.as_str()
                    .is_some_and(|f| f.eq_ignore_ascii_case("blinear"))
                {
                    out.notes.push("native_region_format = blinear: One World writes Anvil; convert to B_Linear after the build (Phase 4)".into());
                }
                out.dropped.push(k.into());
                continue;
            }
            _ => {
                if DROPPED.contains(&k) || k.starts_with("export_") || k.starts_with("server_") {
                    out.dropped.push(k.into());
                } else {
                    out.unmapped.push(k.into());
                }
                continue;
            }
        };
        if mapped {
            out.mapped.push(k.into());
        } else {
            out.unmapped.push(k.into());
        }
    }
    (d, run)
}

/// Meld 1's `name=pct` specs (`arnis_cmd.py`), or `None` where Meld 1 sent no flag.
fn mix(key: &str, m: &Map<String, Json>) -> Option<String> {
    let pct = |name: &str, default: i64| {
        m.get(name)
            .and_then(|v| v.as_f64().map(|f| f as i64))
            .unwrap_or(default)
            .clamp(0, 200)
    };
    let join = |parts: Vec<(&str, i64)>| {
        parts
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    match key {
        // Sent only when a share other than farm is set; then every share above 0.
        "field_mix" => {
            let all: Vec<_> = ["coarse", "plains", "flower", "farm", "moss"]
                .map(|k| (k, pct(k, 0)))
                .into();
            let other: i64 = all.iter().filter(|(k, _)| *k != "farm").map(|p| p.1).sum();
            (other > 0).then(|| join(all.into_iter().filter(|p| p.1 > 0).collect()))
        }
        // Sent only when a crop differs from its default share; then every crop above 0.
        "farm_crops" => {
            let all: Vec<_> = [
                ("wheat", 40),
                ("potato", 15),
                ("carrot", 15),
                ("beetroot", 8),
                ("sunflower", 12),
                ("pumpkin", 5),
                ("fallow", 5),
            ]
            .map(|(k, d)| (k, pct(k, d), d))
            .into();
            all.iter()
                .any(|(_, v, d)| v != d)
                .then(|| join(all.iter().filter(|p| p.1 > 0).map(|p| (p.0, p.1)).collect()))
        }
        // The tiers that differ from their default.
        _ => {
            let diff: Vec<_> = [
                ("small", 100),
                ("medium", 100),
                ("big", 100),
                ("tall", 100),
                ("giant", 0),
            ]
            .into_iter()
            .map(|(k, d)| (k, pct(k, d), d))
            .filter(|(_, v, d)| v != d)
            .map(|(k, v, _)| (k, v))
            .collect();
            (!diff.is_empty()).then(|| join(diff))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real Meld 1.9.x `project.json` (Bucharest, 1:10, 159 settings).
    #[test]
    fn imports_a_real_meld1_project() {
        let json: Json =
            serde_json::from_str(include_str!("../tests/fixtures/meld1-project.json")).unwrap();
        let out = project(&json, None).unwrap();
        let p = Project::parse(&out.toml).unwrap();
        assert_eq!(p.name, "Meld World");
        let s = &p.selections[0];
        assert_eq!(s.world, "Meld World");
        assert_eq!(s.bbox[0], 44.24962581956621);
        assert_eq!(s.bbox[3], 26.334918915217298);
        let d = &p.defaults;
        let get = |k: &str| d.get(k).map(|v| v.to_string());
        assert_eq!(get("scale").as_deref(), Some("0.1"));
        assert_eq!(get("ground_level").as_deref(), Some("-56"));
        assert_eq!(get("buildings").as_deref(), Some("false"));
        assert_eq!(get("fillground").as_deref(), Some("true"));
        assert_eq!(get("road_detail").as_deref(), Some("\"compact\"")); // auto at 0.1
                                                                        // One World refuses peaks: manual from Meld 1's snow_y.
        assert_eq!(get("snow_mode").as_deref(), Some("\"manual\""));
        assert_eq!(get("snow_y").as_deref(), Some("80"));
        assert_eq!(get("snow_percent"), None);
        assert_eq!(get("rocks").as_deref(), Some("true"));
        assert_eq!(get("bushes").as_deref(), Some("true"));
        assert_eq!(get("field_mix"), None); // farm only: Meld 1 sent no flag
        assert_eq!(get("farm_crops"), None); // all at their defaults
        assert_eq!(get("tree_size_weights"), None);
        assert_eq!(get("unit_regions").as_deref(), Some("4"));
        assert_eq!(get("workers").as_deref(), Some("4"));
        assert_eq!(get("seed").as_deref(), Some("1"));
        assert_eq!(
            get("origin").as_deref(),
            Some("\"44.429752066115704,26.08477631813682\"")
        );
        assert_eq!(p.run.cpu_target, 90);
        assert!(get("overpass_url").is_none() && get("mode").is_none());
        assert_eq!(get("offline").as_deref(), Some("false"));
        // Every key is accounted for, once.
        let settings = json["settings"].as_object().unwrap().len();
        let counted = out
            .mapped
            .iter()
            .filter(|k| !k.contains('.') && *k != "origin")
            .count()
            + out.dropped.iter().filter(|k| !k.contains(' ')).count()
            + out.unmapped.len();
        assert_eq!(counted, settings);
        assert!(out.dropped.contains(&"governor_mode".into()));
        assert!(out.dropped.contains(&"server_ram_gb".into()));
        assert!(out.unmapped.contains(&"signage".into()));
        assert!(out.toml.contains("# Not carried (no Meld 2 setting yet): "));
    }

    #[test]
    fn imports_presets_and_polygons() {
        let preset_json = serde_json::json!({
            "meld_preset": 1, "name": "P",
            "settings": {"scale": 1.0, "terrain": false, "offline_elevation": true,
                "road_detail_level": "auto", "river_bed_v1": true,
                "tree_size_weights": {"big": 70, "giant": 0},
                "field_mix": {"farm": 100, "moss": 10}, "farm_crops": {"wheat": 50}}
        });
        let out = preset(&preset_json).unwrap();
        for want in [
            "mode = \"geo-only\"",
            "offline = true",
            "prewarm = true",
            "road_detail = \"clean\"",
            "river_bed = \"v1\"",
            "tree_size_weights = \"big=70\"",
            "field_mix = \"farm=100,moss=10\"",
            "farm_crops = \"wheat=50,potato=15,carrot=15,beetroot=8,sunflower=12,pumpkin=5,fallow=5\"",
        ] {
            assert!(out.toml.contains(want), "{want} not in\n{}", out.toml);
        }
        let mut json: Json =
            serde_json::from_str(include_str!("../tests/fixtures/meld1-project.json")).unwrap();
        json["selection"]["polygons"] =
            serde_json::json!([[[44.3, 25.9], [44.6, 25.9], [44.3, 26.3]]]);
        let p = Project::parse(&project(&json, None).unwrap().toml).unwrap();
        assert!(p.selections.len() > 1 && p.selections[0].id == "area-1");
    }
}
