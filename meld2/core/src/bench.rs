//! `meld2 bench`: the same area built once per arm (a matrix of workers,
//! piece size, threads and scale, or an A/B of two Arnis builds or two
//! settings), each into a fresh world, with Arnis's own `done` record
//! (wall, CPU seconds, peak RSS, chunks) and the world's size on disk.
//!
//! From Meld 1's `bench/`: every arm builds the same bbox from the same
//! origin, so only the knob under test moves (`bench_scheduler.py`'s group),
//! and an A/B is refused unless both sides built the same pair: the same
//! frame and the same areas in their `arnis_one_world.json`
//! (`ab_bucharest.py`'s H1). Timing starts warm: each arm's `prewarm` step
//! fills the caches first and is not counted.

use crate::project::Settings;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The default area: central Vaduz, about 1.1 x 1.1 km (3 x 3 regions at scale 1).
pub const BBOX: [f64; 4] = [47.135, 9.515, 47.145, 9.530];
/// The selection and world every arm builds.
pub const ID: &str = "bench";
pub const WORLD: &str = "Bench";

/// What to bench, from the CLI or the API (JSON).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Request {
    /// min_lat, min_lng, max_lat, max_lng [default: `BBOX`].
    pub bbox: Option<[f64; 4]>,
    /// Settings every arm shares.
    pub set: Settings,
    /// The matrix: every combination of these (an empty list leaves the knob to Arnis).
    pub workers: Vec<toml::Value>,
    pub cells: Vec<i64>,
    pub threads: Vec<i64>,
    pub scales: Vec<f64>,
    /// Builds of each arm.
    pub repeats: u32,
    /// The Arnis every arm uses (the A side in an A/B).
    pub arnis: Option<PathBuf>,
    /// Instead of the matrix: two arms on the same area.
    pub ab: Option<Ab>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Ab {
    /// Settings only side A or B has.
    pub a: Settings,
    pub b: Settings,
    /// Side B's Arnis [default: side A's].
    pub b_arnis: Option<PathBuf>,
}

/// One build to make.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Arm {
    pub name: String,
    pub arnis: Option<PathBuf>,
    pub settings: Settings,
}

impl Request {
    pub fn bbox(&self) -> [f64; 4] {
        self.bbox.unwrap_or(BBOX)
    }

    /// The arms, each checked. Every arm builds from the bbox's centre (an
    /// `origin` in `set` wins), so their frames differ only by scale.
    pub fn arms(&self) -> Result<Vec<Arm>> {
        let [s, w, n, e] = self.bbox();
        let mut base = self.set.clone();
        base.entry("origin".into())
            .or_insert_with(|| format!("{},{}", (s + n) / 2.0, (w + e) / 2.0).into());
        base.entry("prewarm".into()).or_insert(true.into());
        let arm = |name: String, arnis: Option<PathBuf>, extra: &Settings| {
            let mut settings = base.clone();
            settings.extend(extra.clone());
            Arm {
                name,
                arnis,
                settings,
            }
        };
        let mut arms = vec![];
        if let Some(ab) = &self.ab {
            arms.push(arm("A".into(), self.arnis.clone(), &ab.a));
            let b_arnis = ab.b_arnis.clone().or_else(|| self.arnis.clone());
            arms.push(arm("B".into(), b_arnis, &ab.b));
            if arms[0].settings == arms[1].settings && arms[0].arnis == arms[1].arnis {
                bail!("A and B are the same: give B another Arnis or other settings");
            }
        } else {
            let opt = |v: &[toml::Value]| -> Vec<Option<toml::Value>> {
                if v.is_empty() {
                    vec![None]
                } else {
                    v.iter().cloned().map(Some).collect()
                }
            };
            let ints = |v: &[i64]| opt(&v.iter().map(|&i| i.into()).collect::<Vec<_>>());
            let floats = |v: &[f64]| opt(&v.iter().map(|&f| f.into()).collect::<Vec<_>>());
            for wk in opt(&self.workers) {
                for c in ints(&self.cells) {
                    for t in ints(&self.threads) {
                        for sc in floats(&self.scales) {
                            let mut name = vec![];
                            let mut extra = Settings::new();
                            for (tag, key, v) in [
                                ("w", "workers", &wk),
                                ("c", "unit_regions", &c),
                                ("t", "threads", &t),
                                ("s", "scale", &sc),
                            ] {
                                if let Some(v) = v {
                                    let text = v.as_str().map_or(v.to_string(), String::from);
                                    name.push(format!("{tag}{text}"));
                                    extra.insert(key.into(), v.clone());
                                }
                            }
                            let name = if name.is_empty() {
                                "base".into()
                            } else {
                                name.join("-")
                            };
                            arms.push(arm(name, self.arnis.clone(), &extra));
                        }
                    }
                }
            }
        }
        let repeats = self.repeats.max(1);
        if repeats > 1 {
            arms = arms
                .into_iter()
                .flat_map(|a| {
                    (1..=repeats).map(move |r| Arm {
                        name: format!("{}-r{r}", a.name),
                        ..a.clone()
                    })
                })
                .collect();
        }
        if arms.len() > 64 {
            bail!("{} arms; keep a bench to 64 builds", arms.len());
        }
        for a in &arms {
            // The project each arm builds must load: settings and bbox checked.
            crate::project::Project::parse(&project_toml("check", self.bbox(), a)?)
                .with_context(|| format!("arm {}", a.name))?;
        }
        Ok(arms)
    }
}

/// The project file one arm builds: one selection, one world, one job.
pub fn project_toml(name: &str, bbox: [f64; 4], arm: &Arm) -> Result<String> {
    let mut sel = toml::Table::new();
    sel.insert("id".into(), ID.into());
    sel.insert("bbox".into(), toml::Value::try_from(bbox)?);
    sel.insert("world".into(), WORLD.into());
    sel.insert(
        "settings".into(),
        toml::Value::Table(arm.settings.clone().into_iter().collect()),
    );
    let mut t = toml::Table::new();
    t.insert("format".into(), 1.into());
    t.insert("name".into(), format!("bench {name} {}", arm.name).into());
    t.insert("output".into(), "saves".into());
    t.insert("selection".into(), toml::Value::Array(vec![sel.into()]));
    Ok(toml::to_string(&t)?)
}

/// One arm's result.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Row {
    pub name: String,
    pub arnis: String,
    pub arnis_version: String,
    pub settings: Value,
    pub wall_s: f64,
    pub cpu_s: Option<f64>,
    /// CPU seconds over wall seconds, as a share of every core.
    pub cpu_pct: Option<f64>,
    /// The largest single process: Arnis's coordinator or its biggest piece.
    pub peak_rss_mb: Option<u64>,
    /// What the whole process tree had committed at its peak (Windows).
    pub tree_peak_mb: Option<u64>,
    pub chunks: u64,
    pub chunks_per_s: f64,
    pub disk_mb: f64,
    pub regions: usize,
    /// The world's `arnis_one_world.json`.
    #[serde(skip_serializing_if = "Value::is_null", default)]
    pub manifest: Value,
    pub error: Option<String>,
}

impl Row {
    /// From Arnis's `done` record.
    pub fn done(
        &mut self,
        wall_s: f64,
        cpu_s: Option<f64>,
        peak: Option<u64>,
        chunks: u64,
        cores: usize,
    ) {
        self.wall_s = wall_s;
        self.cpu_s = cpu_s;
        self.cpu_pct = cpu_s
            .filter(|_| wall_s > 0.0)
            .map(|c| 100.0 * c / wall_s / cores.max(1) as f64);
        self.peak_rss_mb = peak;
        self.chunks = chunks;
        self.chunks_per_s = if wall_s > 0.0 {
            chunks as f64 / wall_s
        } else {
            0.0
        };
    }
}

/// The size of a world folder and its region files.
pub fn world_size(world: &Path) -> (u64, usize) {
    let bytes = crate::server::dir_bytes(world).unwrap_or(0);
    let regions = std::fs::read_dir(world.join("region"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".mca"))
        .count();
    (bytes, regions)
}

/// Manifest keys that change with every build, not with what was built.
const VOLATILE: &[&str] = &[
    "created_at",
    "created_with",
    "generated_at",
    "arnis_version",
    "id",
    "next_area_id",
    "preview",
];
/// What makes two builds the same pair: the frame and the built areas.
const FRAME: &[&str] = &["origin_lat", "origin_lon", "scale"];
const AREA: &[&str] = &[
    "min_x", "min_z", "max_x", "max_z", "min_lat", "min_lon", "max_lat", "max_lon",
];

/// The A/B pair check on the two worlds' manifests: fails unless they have
/// the same frame and the same areas; otherwise returns the other keys that
/// differ (what the A/B changed, e.g. a seed), as `key: a -> b`.
pub fn same_pair(a: &Value, b: &Value) -> Result<Vec<String>> {
    for k in FRAME {
        if a[k].is_null() || a[k] != b[k] {
            bail!("not the same pair: {k} is {} on A and {} on B", a[k], b[k]);
        }
    }
    let areas = |m: &Value| -> Vec<Vec<Value>> {
        m["areas"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|ar| AREA.iter().map(|k| ar[k].clone()).collect())
            .collect()
    };
    let (aa, ba) = (areas(a), areas(b));
    if aa.is_empty() || aa != ba {
        bail!("not the same pair: A built areas {aa:?}, B built {ba:?}");
    }
    let mut diff = vec![];
    fn walk(path: String, a: &Value, b: &Value, diff: &mut Vec<String>) {
        match (a, b) {
            (Value::Object(x), Value::Object(y)) => {
                let keys: std::collections::BTreeSet<_> = x.keys().chain(y.keys()).collect();
                for k in keys.into_iter().filter(|k| !VOLATILE.contains(&k.as_str())) {
                    let p = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    walk(
                        p,
                        x.get(k).unwrap_or(&Value::Null),
                        y.get(k).unwrap_or(&Value::Null),
                        diff,
                    );
                }
            }
            (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
                for (i, (p, q)) in x.iter().zip(y).enumerate() {
                    walk(format!("{path}[{i}]"), p, q, diff);
                }
            }
            _ if a != b => diff.push(format!("{path}: {a} -> {b}")),
            _ => {}
        }
    }
    walk(String::new(), a, b, &mut diff);
    Ok(diff)
}

/// The whole bench, as written to `bench.json`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Report {
    pub id: String,
    pub started: u64,
    pub finished: Option<u64>,
    pub cores: usize,
    pub bbox: [f64; 4],
    pub request: Value,
    pub rows: Vec<Row>,
    /// A/B: the manifest keys that differ between A and B (frame and areas
    /// are equal, or `pair_error` says why not).
    pub pair_diff: Option<Vec<String>>,
    pub pair_error: Option<String>,
    pub error: Option<String>,
}

impl Report {
    pub fn csv(&self) -> String {
        let mut s = String::from(
            "name,arnis_version,settings,wall_s,cpu_s,cpu_pct,peak_rss_mb,tree_peak_mb,chunks,chunks_per_s,disk_mb,regions,error\n",
        );
        let opt = |v: Option<f64>| v.map_or(String::new(), |x| format!("{x:.1}"));
        for r in &self.rows {
            let _ = writeln!(
                s,
                "{},{},\"{}\",{:.1},{},{},{},{},{},{:.0},{:.1},{},\"{}\"",
                r.name,
                r.arnis_version,
                r.settings.to_string().replace('"', "'"),
                r.wall_s,
                opt(r.cpu_s),
                opt(r.cpu_pct),
                r.peak_rss_mb.map_or(String::new(), |m| m.to_string()),
                r.tree_peak_mb.map_or(String::new(), |m| m.to_string()),
                r.chunks,
                r.chunks_per_s,
                r.disk_mb,
                r.regions,
                r.error.as_deref().unwrap_or("").replace('"', "'"),
            );
        }
        s
    }

    /// A Markdown table; `Δ wall` against the first row, `+` = faster.
    pub fn table(&self) -> String {
        let mut s = String::from(
            "| arm | wall s | Δ wall | CPU s | CPU % | peak RSS MB | tree peak MB | chunks | chunks/s | disk MB | regions |\n|---|---|---|---|---|---|---|---|---|---|---|\n",
        );
        let base = self.rows.first().map(|r| r.wall_s).filter(|w| *w > 0.0);
        for r in &self.rows {
            if let Some(e) = &r.error {
                let _ = writeln!(s, "| {} | failed: {e} ||||||||||", r.name);
                continue;
            }
            let delta = base.map_or("–".into(), |b| {
                format!("{:+.1} %", (b - r.wall_s) / b * 100.0)
            });
            let opt = |v: Option<f64>| v.map_or("–".into(), |x| format!("{x:.1}"));
            let _ = writeln!(
                s,
                "| {} | {:.1} | {delta} | {} | {} | {} | {} | {} | {:.0} | {:.1} | {} |",
                r.name,
                r.wall_s,
                opt(r.cpu_s),
                opt(r.cpu_pct),
                r.peak_rss_mb.map_or("–".into(), |m| m.to_string()),
                r.tree_peak_mb.map_or("–".into(), |m| m.to_string()),
                r.chunks,
                r.chunks_per_s,
                r.disk_mb,
                r.regions
            );
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matrix_arms_and_projects() {
        let req: Request = serde_json::from_value(json!({
            "workers": [1, "auto"], "cells": [1, 2], "set": {"caves": true}, "repeats": 2
        }))
        .unwrap();
        let arms = req.arms().unwrap();
        assert_eq!(arms.len(), 8);
        assert_eq!(arms[0].name, "w1-c1-r1");
        assert_eq!(arms[7].name, "wauto-c2-r2");
        let s = &arms[7].settings;
        assert_eq!(s["workers"].as_str(), Some("auto"));
        assert_eq!(s["unit_regions"].as_integer(), Some(2));
        assert_eq!(s["origin"].as_str(), Some("47.14,9.5225"));
        assert_eq!(s["prewarm"].as_bool(), Some(true));
        let p = crate::project::Project::parse(&project_toml("t", req.bbox(), &arms[7]).unwrap())
            .unwrap();
        assert_eq!(p.selections[0].bbox, BBOX);
        assert_eq!(p.selections[0].world, WORLD);

        // An A/B needs two different sides; a bad key is refused.
        let ab = |v: Value| serde_json::from_value::<Request>(v).unwrap().arms();
        assert!(ab(json!({"ab": {}}))
            .unwrap_err()
            .to_string()
            .contains("the same"));
        let arms = ab(json!({"ab": {"b": {"seed": 2}}})).unwrap();
        assert_eq!((arms[0].name.as_str(), arms[1].name.as_str()), ("A", "B"));
        assert!(ab(json!({"set": {"cavez": true}})).is_err());
        assert!(ab(json!({"bbox": [47.2, 9.5, 47.1, 9.6]})).is_err());
    }

    /// The A/B pair assertion: it fails when the two sides are not the same pair.
    #[test]
    fn pair_assertion_fails_unless_same_frame_and_areas() {
        let m = |lat: f64, max_x: i64, seed: i64| {
            json!({"origin_lat": lat, "origin_lon": 9.5225, "scale": 1.0, "seed": seed,
                   "created_at": 1, "next_area_id": 2,
                   "areas": [{"id": 1, "generated_at": 5, "min_x": -570, "min_z": -560,
                              "max_x": max_x, "max_z": 559, "min_lat": 47.135, "min_lon": 9.515,
                              "max_lat": 47.145, "max_lon": 9.53}]})
        };
        let a = m(47.14, 569, 1);
        let mut b = m(47.14, 569, 2);
        b["created_at"] = 9.into();
        b["areas"][0]["generated_at"] = 7.into();
        assert_eq!(same_pair(&a, &b).unwrap(), ["seed: 1 -> 2"]);
        assert!(same_pair(&a, &m(47.14, 569, 1)).unwrap().is_empty());
        let e = same_pair(&a, &m(47.2, 569, 1)).unwrap_err().to_string();
        assert!(e.contains("origin_lat"), "{e}");
        let e = same_pair(&a, &m(47.14, 600, 1)).unwrap_err().to_string();
        assert!(e.contains("areas"), "{e}");
        let mut none = a.clone();
        none["areas"] = json!([]);
        assert!(
            same_pair(&none, &none).is_err(),
            "no areas built is no pair"
        );
        assert!(same_pair(&Value::Null, &Value::Null).is_err());
    }

    #[test]
    fn row_rates_and_report_table() {
        let mut r = Row {
            name: "w1".into(),
            ..Default::default()
        };
        r.done(10.0, Some(40.0), Some(900), 5000, 8);
        assert_eq!((r.cpu_pct, r.chunks_per_s), (Some(50.0), 500.0));
        let mut fast = r.clone();
        fast.name = "w2".into();
        fast.done(8.0, Some(40.0), Some(1200), 5000, 8);
        let rep = Report {
            rows: vec![r, fast],
            ..Default::default()
        };
        assert!(
            rep.table().contains("| w2 | 8.0 | +20.0 % |"),
            "{}",
            rep.table()
        );
        assert_eq!(rep.csv().lines().count(), 3);
    }
}
