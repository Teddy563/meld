//! The project file: a saved set of selections, each with its own settings,
//! building into one or several One Worlds. TOML, versioned by `format`.
//!
//! ```toml
//! format = 1
//! name = "Alps"
//! output = "saves"            # the saves folder, relative to this file
//!
//! [run]
//! jobs = 2                    # Arnis processes at once
//! cpu_target = 90             # share of the cores all jobs split
//!
//! [defaults]                  # every selection starts from these
//! scale = 1.0
//!
//! [[selection]]
//! id = "vaduz"
//! bbox = [47.139, 9.520, 47.141, 9.523]   # min_lat, min_lng, max_lat, max_lng
//! world = "Alps"
//! settings = { caves = true, prewarm = true }   # prewarm: a cache-filling step first
//!
//! [[selection]]               # a shape instead of a bbox: rings of [lat, lng]
//! id = "li"
//! polygon = [[[47.05, 9.47], [47.27, 9.53], [47.06, 9.63]]]
//! world = "Alps"
//!
//! [[bake]]                    # a country .osm.pbf cut once, before the builds that read it
//! id = "li"
//! osm_pbf = "geofabrik"       # the selections' own `osm_pbf` value
//! ```

use crate::args;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// The newest project format this build reads.
pub const FORMAT: i64 = 1;

/// Settings by key, as `args::OPTS` names them.
pub type Settings = BTreeMap<String, toml::Value>;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub format: i64,
    pub name: String,
    /// Saves folder the worlds are built in. Relative paths are taken from the project file.
    pub output: PathBuf,
    /// Arnis executable, when not given on the command line.
    pub arnis: Option<PathBuf>,
    #[serde(default)]
    pub run: Budget,
    #[serde(default)]
    pub defaults: Settings,
    #[serde(rename = "selection", default)]
    pub selections: Vec<Selection>,
    #[serde(rename = "bake", default)]
    pub bakes: Vec<Bake>,
    /// Where the project was read from.
    #[serde(skip)]
    pub path: PathBuf,
}

/// What all jobs of one run share.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Budget {
    /// Arnis processes running at once. Selections of one world never overlap.
    pub jobs: u32,
    /// Share of the cores, in percent, split evenly between the jobs.
    pub cpu_target: u32,
    /// Memory split evenly between the jobs, in MB. Unset lets each Arnis read free RAM.
    pub ram_budget_mb: Option<u64>,
    /// Disk space, in MB, a run must leave free on the saves volume after its
    /// estimated size (plus 25 % margin); a run that would not is refused.
    pub min_free_mb: u64,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            jobs: 1,
            cpu_target: 90,
            ram_budget_mb: None,
            min_free_mb: 1024,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub id: String,
    /// min_lat, min_lng, max_lat, max_lng: the order of Arnis's --bbox.
    #[serde(default)]
    pub bbox: [f64; 4],
    /// Instead of `bbox`: rings of `[lat, lng]` points; their union is the area.
    /// Arnis takes only a bbox, so on load it becomes selections `<id>-1`, `<id>-2`, ...:
    /// one bbox per row of piece-sized cells that touch the shape.
    #[serde(default)]
    pub polygon: Vec<Vec<[f64; 2]>>,
    /// One World folder in `output`. Selections sharing a world extend it one after another.
    pub world: String,
    #[serde(default)]
    pub settings: Settings,
}

/// A data step: Arnis cuts (bakes) an `.osm.pbf` extract for an area once,
/// and every selection with the same `osm_pbf` then reads the bake.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bake {
    pub id: String,
    /// What `--osm-pbf` reads: an `.osm.pbf` path or `geofabrik`.
    pub osm_pbf: String,
    /// Area to bake. Unset: around every selection with this `osm_pbf`.
    pub bbox: Option<[f64; 4]>,
}

fn check_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !ok {
        bail!("id {id:?}: use letters, digits, - and _");
    }
    Ok(())
}

fn check_bbox(b: &[f64; 4]) -> Result<()> {
    let [s_lat, w_lng, n_lat, e_lng] = *b;
    let valid = b.iter().all(|v| v.is_finite())
        && (-90.0..=90.0).contains(&s_lat)
        && (-90.0..=90.0).contains(&n_lat)
        && (-180.0..=180.0).contains(&w_lng)
        && (-180.0..=180.0).contains(&e_lng)
        && s_lat < n_lat
        && w_lng < e_lng;
    if !valid {
        bail!("bbox must be [min_lat, min_lng, max_lat, max_lng]");
    }
    Ok(())
}

impl Project {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut project = Self::parse(&text).with_context(|| format!("in {}", path.display()))?;
        project.path = path.to_path_buf();
        Ok(project)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let table: toml::Table = text.parse()?;
        match table.get("format").and_then(toml::Value::as_integer) {
            Some(FORMAT) => {}
            Some(n) if n > FORMAT => {
                bail!("project format {n} is newer than this Meld reads ({FORMAT}); update Meld")
            }
            Some(n) => bail!("unknown project format {n}"),
            None => bail!("missing `format = {FORMAT}`"),
        }
        let mut project: Self = table.try_into()?;
        project.cover_polygons()?;
        project.validate()?;
        Ok(project)
    }

    fn validate(&self) -> Result<()> {
        if self.selections.is_empty() {
            bail!("no [[selection]]");
        }
        if self.run.jobs == 0 {
            bail!("run.jobs must be at least 1");
        }
        if !(10..=100).contains(&self.run.cpu_target) {
            bail!("run.cpu_target must be 10 to 100");
        }
        args::check(&self.defaults).context("in [defaults]")?;
        let mut ids = HashSet::new();
        for s in &self.selections {
            check_id(&s.id).with_context(|| format!("selection {}", s.id))?;
            if !ids.insert(s.id.as_str()) {
                bail!("selection id {:?} is used twice", s.id);
            }
            if s.world.trim().is_empty() {
                bail!("selection {}: empty world", s.id);
            }
            check_bbox(&s.bbox).with_context(|| format!("selection {}", s.id))?;
            args::check(&s.settings).with_context(|| format!("in selection {}", s.id))?;
        }
        let mut bakes = HashSet::new();
        for b in &self.bakes {
            check_id(&b.id).with_context(|| format!("bake {}", b.id))?;
            if !bakes.insert(b.id.as_str()) {
                bail!("bake id {:?} is used twice", b.id);
            }
            match &b.bbox {
                Some(bbox) => check_bbox(bbox).with_context(|| format!("bake {}", b.id))?,
                None if self.reading(&b.osm_pbf).next().is_none() => bail!(
                    "bake {}: no selection has osm_pbf = {:?}; set one or give the bake a bbox",
                    b.id,
                    b.osm_pbf
                ),
                None => {}
            }
        }
        Ok(())
    }

    /// Replaces each polygon selection with the bboxes that cover it.
    fn cover_polygons(&mut self) -> Result<()> {
        let mut out = Vec::with_capacity(self.selections.len());
        for sel in std::mem::take(&mut self.selections) {
            if sel.polygon.is_empty() {
                out.push(sel);
                continue;
            }
            if sel.bbox != [0.0; 4] {
                bail!("selection {}: give a bbox or a polygon, not both", sel.id);
            }
            let pts = || sel.polygon.iter().flatten();
            let ok = sel.polygon.iter().all(|r| r.len() >= 3)
                && pts().all(|[lat, lng]| {
                    (-90.0..=90.0).contains(lat) && (-180.0..=180.0).contains(lng)
                });
            if !ok {
                bail!(
                    "selection {}: polygon rings need 3+ [lat, lng] points",
                    sel.id
                );
            }
            // Cells the size of a piece: (unit_regions x 512 blocks) / scale metres.
            let settings = self.settings_for(&sel);
            let scale = settings
                .get("scale")
                .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                .unwrap_or(1.0)
                .max(0.01);
            let side_m = args::unit_regions(&settings) as f64 * 512.0 / scale;
            let mid = pts().map(|p| p[0]).sum::<f64>() / pts().count() as f64;
            let dlat = side_m / 111_320.0;
            let dlon = dlat / mid.to_radians().cos().max(0.01);
            let boxes =
                cover(&sel.polygon, dlat, dlon).with_context(|| format!("selection {}", sel.id))?;
            for (i, bbox) in boxes.into_iter().enumerate() {
                out.push(Selection {
                    id: format!("{}-{}", sel.id, i + 1),
                    bbox,
                    polygon: vec![],
                    world: sel.world.clone(),
                    settings: sel.settings.clone(),
                });
            }
        }
        self.selections = out;
        Ok(())
    }

    /// The selections whose `osm_pbf` is `src`.
    pub fn reading<'a>(&'a self, src: &'a str) -> impl Iterator<Item = &'a Selection> + 'a {
        self.selections.iter().filter(move |s| {
            self.settings_for(s)
                .get("osm_pbf")
                .and_then(toml::Value::as_str)
                == Some(src)
        })
    }

    /// The area a bake cuts: its own bbox, or the selections reading it, padded
    /// by what Arnis keeps past a piece (2 x 64 blocks at the smallest scale),
    /// so each selection's own bake lookup finds this one.
    pub fn bake_bbox(&self, bake: &Bake) -> [f64; 4] {
        if let Some(b) = bake.bbox {
            return b;
        }
        let mut u = [90.0, 180.0, -90.0, -180.0_f64];
        let mut pad_m: f64 = 0.0;
        for s in self.reading(&bake.osm_pbf) {
            u = [
                u[0].min(s.bbox[0]),
                u[1].min(s.bbox[1]),
                u[2].max(s.bbox[2]),
                u[3].max(s.bbox[3]),
            ];
            let scale = self
                .settings_for(s)
                .get("scale")
                .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)));
            pad_m = pad_m.max(128.0 / scale.unwrap_or(1.0).max(0.01) + 64.0);
        }
        let lat = pad_m / 111_320.0;
        let lon = lat / u[0].abs().max(u[2].abs()).to_radians().cos().max(0.01);
        [
            (u[0] - lat).max(-85.0),
            (u[1] - lon).max(-180.0),
            (u[2] + lat).min(85.0),
            (u[3] + lon).min(180.0),
        ]
    }

    /// The saves folder, resolved against the project file.
    pub fn output_dir(&self) -> PathBuf {
        self.path
            .parent()
            .unwrap_or(Path::new("."))
            .join(&self.output)
    }

    /// A selection's settings: the defaults with its own on top.
    pub fn settings_for(&self, sel: &Selection) -> Settings {
        let mut s = self.defaults.clone();
        s.extend(sel.settings.clone());
        s
    }
}

/// Bboxes covering the union of `rings` (`[lat, lng]`): a grid of
/// `dlat` x `dlon` cells from the rings' south-west corner, keeping every cell
/// that overlaps a ring, merged into one bbox per run of cells in a row.
// ponytail: cells are piece-sized, not snapped to Arnis's piece lattice (that
// needs the world's origin); neighbours share a world, so their seams match.
// No antimeridian crossing.
pub fn cover(rings: &[Vec<[f64; 2]>], dlat: f64, dlon: f64) -> Result<Vec<[f64; 4]>> {
    let pts = || rings.iter().flatten();
    let fold = |f: fn(f64, f64) -> f64, i: usize, init: f64| pts().map(|p| p[i]).fold(init, f);
    let (s, w) = (fold(f64::min, 0, 90.0), fold(f64::min, 1, 180.0));
    let (n, e) = (fold(f64::max, 0, -90.0), fold(f64::max, 1, -180.0));
    let rows = ((n - s) / dlat).ceil().max(1.0) as usize;
    let cols = ((e - w) / dlon).ceil().max(1.0) as usize;
    if rows.saturating_mul(cols) > 4_000_000 {
        bail!("polygon spans {rows} x {cols} cells; raise unit_regions or lower scale");
    }
    let mut out = vec![];
    for r in 0..rows {
        let mut run: Option<usize> = None;
        for c in 0..=cols {
            let cell = [
                s + r as f64 * dlat,
                w + c as f64 * dlon,
                s + (r + 1) as f64 * dlat,
                w + (c + 1) as f64 * dlon,
            ];
            let hit = c < cols && overlaps(rings, cell);
            match (hit, run) {
                (true, None) => run = Some(c),
                (false, Some(c0)) => {
                    out.push([cell[0], w + c0 as f64 * dlon, cell[2], cell[1]].map(|v| {
                        // Rounded to ~1 cm, so the bboxes read cleanly in the project and logs.
                        (v * 1e7).round() / 1e7
                    }));
                    run = None;
                }
                _ => {}
            }
        }
    }
    Ok(out)
}

/// Whether a `[s, w, n, e]` cell and the union of `rings` share some area:
/// its centre is inside a ring, or a ring edge crosses it. The cell is shrunk
/// a hair first, so a ring that only runs along its border does not count.
fn overlaps(rings: &[Vec<[f64; 2]>], cell: [f64; 4]) -> bool {
    let (ilat, ilon) = ((cell[2] - cell[0]) * 1e-6, (cell[3] - cell[1]) * 1e-6);
    let r = [
        cell[0] + ilat,
        cell[1] + ilon,
        cell[2] - ilat,
        cell[3] - ilon,
    ];
    let c = [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0];
    rings.iter().any(|ring| {
        let edges = || ring.iter().zip(ring.iter().cycle().skip(1));
        // Even-odd: a ray from the centre towards +lng crosses the ring an odd number of times.
        let inside = edges()
            .filter(|(a, b)| (a[0] > c[0]) != (b[0] > c[0]))
            .filter(|(a, b)| c[1] < a[1] + (c[0] - a[0]) * (b[1] - a[1]) / (b[0] - a[0]))
            .count()
            % 2
            == 1;
        inside || edges().any(|(a, b)| crosses(*a, *b, r))
    })
}

/// Liang-Barsky: whether segment `a`-`b` meets the rectangle `[s, w, n, e]`.
fn crosses(a: [f64; 2], b: [f64; 2], r: [f64; 4]) -> bool {
    let d = [b[0] - a[0], b[1] - a[1]];
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [
        (-d[0], a[0] - r[0]),
        (d[0], r[2] - a[0]),
        (-d[1], a[1] - r[1]),
        (d[1], r[3] - a[1]),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
        } else if p < 0.0 {
            t0 = t0.max(q / p);
        } else {
            t1 = t1.min(q / p);
        }
    }
    t0 <= t1
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
format = 1
name = "Demo"
output = "saves"

[run]
jobs = 2

[defaults]
scale = 0.5
caves = false

[[selection]]
id = "a"
bbox = [47.139, 9.52, 47.141, 9.523]
world = "One"

[[selection]]
id = "b"
bbox = [47.15, 9.52, 47.16, 9.53]
world = "One"
settings = { caves = true, snow_mode = "peaks" }
"#;

    #[test]
    fn parses_and_merges_settings() {
        let p = Project::parse(GOOD).unwrap();
        assert_eq!(p.name, "Demo");
        assert_eq!(p.run.jobs, 2);
        assert_eq!(p.run.cpu_target, 90);
        assert_eq!(p.selections.len(), 2);
        let b = p.settings_for(&p.selections[1]);
        assert_eq!(b["scale"].as_float(), Some(0.5));
        assert_eq!(b["caves"].as_bool(), Some(true));
        assert_eq!(b["snow_mode"].as_str(), Some("peaks"));
        let a = p.settings_for(&p.selections[0]);
        assert_eq!(a["caves"].as_bool(), Some(false));
    }

    #[test]
    fn rejects_bad_files() {
        let cases = [
            (GOOD.replace("format = 1", "format = 2"), "newer"),
            (GOOD.replace("format = 1", ""), "missing `format"),
            (GOOD.replace("id = \"b\"", "id = \"a\""), "used twice"),
            (GOOD.replace("id = \"b\"", "id = \"b c\""), "letters"),
            (
                GOOD.to_string()
                    + "[[bake]]
id = \"x\"
osm_pbf = \"geofabrik\"
",
                "no selection",
            ),
            (
                GOOD.replace("[47.15, 9.52, 47.16, 9.53]", "[47.16, 9.52, 47.15, 9.53]"),
                "bbox",
            ),
            (
                GOOD.replace("caves = true", "cavez = true"),
                "unknown setting",
            ),
            (GOOD.replace("caves = false", "caves = \"no\""), "caves"),
            (
                GOOD.replace("caves = false", "region_format = \"blinear\""),
                "refuses",
            ),
            (
                GOOD.replace("name = \"Demo\"", "name = \"Demo\"\ncolour = 1"),
                "colour",
            ),
            (GOOD.replace("jobs = 2", "jobs = 0"), "jobs"),
        ];
        for (text, want) in cases {
            let err = format!("{:#}", Project::parse(&text).unwrap_err());
            assert!(err.contains(want), "{want:?} not in {err:?}");
        }
    }

    #[test]
    fn polygons_become_covering_bboxes() {
        // An L: the bottom row is two cells wide, the top row one. A ring that
        // runs along a cell's border does not take the cell; a thin sliver does.
        let l = vec![vec![
            [0.0, 0.0],
            [0.0, 2.0],
            [1.0, 2.0],
            [1.0, 1.0],
            [2.0, 1.0],
            [2.0, 0.0],
        ]];
        assert_eq!(
            cover(&l, 1.0, 1.0).unwrap(),
            [[0.0, 0.0, 1.0, 2.0], [1.0, 0.0, 2.0, 1.0]]
        );
        let sliver = vec![
            vec![[0.0, 0.0], [0.0, 3.0], [0.01, 3.0], [0.01, 0.0]],
            vec![[2.5, 2.5], [2.6, 2.5], [2.6, 2.6]],
        ];
        assert_eq!(
            cover(&sliver, 1.0, 1.0).unwrap(),
            [[0.0, 0.0, 1.0, 3.0], [2.0, 2.0, 3.0, 3.0]]
        );
        // In a project: piece-sized rows in the selection's world and settings.
        let text = GOOD.replace(
            "bbox = [47.15, 9.52, 47.16, 9.53]",
            "polygon = [[[47.15, 9.52], [47.25, 9.52], [47.15, 9.66]]]",
        );
        let p = Project::parse(&text).unwrap();
        let ids: Vec<_> = p.selections.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.len() > 2 && ids[1] == "b-1", "{ids:?}");
        let b1 = &p.selections[1];
        assert_eq!(
            (b1.world.as_str(), b1.settings["caves"].as_bool()),
            ("One", Some(true))
        );
        // scale 0.5, 4-region pieces: 4096 m rows, ~0.037 degrees.
        assert!((b1.bbox[2] - b1.bbox[0] - 4096.0 / 111_320.0).abs() < 1e-6);
    }
}
