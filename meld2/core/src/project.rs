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
use crate::frame::{Frame, Manifest};
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
    /// The Minecraft server its worlds are played on (`meld2 server`).
    pub server: Option<crate::server::Conf>,
    /// The polygon selections as written, before they became parts.
    #[serde(skip)]
    pub shapes: Vec<Selection>,
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
    /// For a part of a polygon selection: the polygon's id.
    #[serde(skip)]
    pub part_of: Option<String>,
}

/// A data step: Arnis cuts (bakes) an `.osm.pbf` extract for an area once,
/// and every selection with the same `osm_pbf` (and `osm_pbf_url`) then
/// reads the bake. Arnis keeps bakes per extract, so all of them must read
/// one extract: a file, or for `geofabrik` the one `osm_pbf_url` names.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bake {
    pub id: String,
    /// What `--osm-pbf` reads: an `.osm.pbf` path or `geofabrik`.
    pub osm_pbf: String,
    /// With `geofabrik`: the extract to download (`--osm-pbf-url`). Required,
    /// since the smallest region around the whole area may not be each selection's.
    pub osm_pbf_url: Option<String>,
    /// Area to bake. Unset: around every selection that reads this extract.
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
        Self::parse_at(&text, path).with_context(|| format!("in {}", path.display()))
    }

    /// A project from its text alone: polygons are cut in the frame their
    /// world will get, since no world on disk is known.
    pub fn parse(text: &str) -> Result<Self> {
        Self::parse_at(text, Path::new(""))
    }

    fn parse_at(text: &str, path: &Path) -> Result<Self> {
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
        project.path = path.to_path_buf();
        project.cover_polygons()?;
        project.validate()?;
        Ok(project)
    }

    fn validate(&self) -> Result<()> {
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
        if let Some(server) = &self.server {
            server.check(&self.worlds())?;
        }
        let mut bakes = HashSet::new();
        for b in &self.bakes {
            check_id(&b.id).with_context(|| format!("bake {}", b.id))?;
            if !bakes.insert(b.id.as_str()) {
                bail!("bake id {:?} is used twice", b.id);
            }
            if b.osm_pbf == "geofabrik" && b.osm_pbf_url.is_none() {
                bail!(
                    "bake {}: with osm_pbf = \"geofabrik\", set osm_pbf_url to the extract, here and in the selections' settings: Arnis keeps a bake per extract, and the smallest Geofabrik region around all the selections can differ from each one's",
                    b.id
                );
            }
            match &b.bbox {
                Some(bbox) => check_bbox(bbox).with_context(|| format!("bake {}", b.id))?,
                None if self.reading(b).next().is_none() => bail!(
                    "bake {}: no selection reads osm_pbf = {:?} with osm_pbf_url = {:?}; set them or give the bake a bbox",
                    b.id,
                    b.osm_pbf,
                    b.osm_pbf_url
                ),
                None => {}
            }
        }
        Ok(())
    }

    /// The frame `world` has, or will get from its first build: its manifest,
    /// else the `origin` of the world's first selection, else that selection's
    /// centre (what Arnis picks), at its scale.
    pub fn frame(&self, world: &str) -> Result<Option<Frame>> {
        if !self.path.as_os_str().is_empty() {
            if let Some(m) = Manifest::load(&self.output_dir().join(world))? {
                return Ok(Some(m.frame()));
            }
        }
        let Some(first) = self.selections.iter().find(|s| s.world == world) else {
            return Ok(None);
        };
        let settings = self.settings_for(first);
        let num = |v: &toml::Value| v.as_float().or_else(|| v.as_integer().map(|i| i as f64));
        let scale = settings.get("scale").and_then(num).unwrap_or(1.0);
        let origin = settings
            .get("origin")
            .and_then(toml::Value::as_str)
            .and_then(|o| o.split_once(','))
            .and_then(|(a, b)| Some((a.trim().parse().ok()?, b.trim().parse().ok()?)));
        let (lat, lon) = origin.unwrap_or_else(|| {
            let pts: Vec<[f64; 2]> = if first.polygon.is_empty() {
                vec![
                    [first.bbox[0], first.bbox[1]],
                    [first.bbox[2], first.bbox[3]],
                ]
            } else {
                first.polygon.iter().flatten().copied().collect()
            };
            let mid = |i: usize| {
                let (lo, hi) = pts.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| {
                    (lo.min(p[i]), hi.max(p[i]))
                });
                (lo + hi) / 2.0
            };
            (mid(0), mid(1))
        });
        Ok(Some(Frame::new(lat, lon, scale)))
    }

    /// Replaces each polygon selection with the bboxes that cover it: runs of
    /// the world's piece cells (`unit_regions` x 512 blocks, on the lattice
    /// Arnis cuts from block 0 of the frame) that overlap the shape, so each
    /// part builds whole pieces. Parts carry the frame's `origin`, so the
    /// world gets that frame if a part creates it.
    fn cover_polygons(&mut self) -> Result<()> {
        let all = std::mem::take(&mut self.selections);
        for sel in all {
            if sel.polygon.is_empty() {
                self.selections.push(sel);
                continue;
            }
            if sel.bbox != [0.0; 4] {
                bail!("selection {}: give a bbox or a polygon, not both", sel.id);
            }
            let pts = || sel.polygon.iter().flatten();
            let ok = sel.polygon.iter().all(|r| r.len() >= 3)
                && pts().all(|[lat, lng]| {
                    (-85.0..=85.0).contains(lat) && (-180.0..=180.0).contains(lng)
                });
            if !ok {
                bail!(
                    "selection {}: polygon rings need 3+ [lat, lng] points within 85 degrees of the equator",
                    sel.id
                );
            }
            // The world's first selection may be this one: look with it in place.
            self.selections.push(sel);
            let frame = self.frame(&self.selections.last().expect("pushed").world);
            let sel = self.selections.pop().expect("pushed");
            let frame = frame?.expect("the selection is in its world");
            let side = args::unit_regions(&self.settings_for(&sel)) as f64 * 512.0;
            let rings: Vec<Vec<[f64; 2]>> = sel
                .polygon
                .iter()
                .map(|r| {
                    r.iter()
                        .map(|&[lat, lon]| [frame.z(lat), frame.x(lon)])
                        .collect()
                })
                .collect();
            let cells = cover(&rings, side).with_context(|| format!("selection {}", sel.id))?;
            for (i, [z0, x0, z1, x1]) in cells.into_iter().enumerate() {
                let mut settings = sel.settings.clone();
                settings.insert("origin".into(), frame.origin_arg().into());
                self.selections.push(Selection {
                    id: format!("{}-{}", sel.id, i + 1),
                    bbox: [frame.lat(z1), frame.lon(x0), frame.lat(z0), frame.lon(x1)],
                    polygon: vec![],
                    world: sel.world.clone(),
                    settings,
                    part_of: Some(sel.id.clone()),
                });
            }
            self.shapes.push(sel);
        }
        Ok(())
    }

    /// Whether `sel` reads the extract `bake` cuts.
    pub fn reads(&self, sel: &Selection, bake: &Bake) -> bool {
        let settings = self.settings_for(sel);
        let get = |k| settings.get(k).and_then(toml::Value::as_str);
        get("osm_pbf") == Some(bake.osm_pbf.as_str())
            && get("osm_pbf_url") == bake.osm_pbf_url.as_deref()
    }

    /// The selections that read the extract `bake` cuts.
    pub fn reading<'a>(&'a self, bake: &'a Bake) -> impl Iterator<Item = &'a Selection> + 'a {
        self.selections.iter().filter(|s| self.reads(s, bake))
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
        for s in self.reading(bake) {
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

    /// The worlds the selections build, in the order they first appear.
    pub fn worlds(&self) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        for s in &self.selections {
            if !out.contains(&s.world) {
                out.push(s.world.clone());
            }
        }
        out
    }

    /// The saves folder, resolved against the project file.
    pub fn output_dir(&self) -> PathBuf {
        self.path
            .parent()
            .unwrap_or(Path::new("."))
            .join(&self.output)
    }

    /// The project's `arnis`, resolved against the project file.
    pub fn arnis_path(&self) -> Option<PathBuf> {
        let dir = self.path.parent().unwrap_or(Path::new("."));
        self.arnis.as_ref().map(|a| dir.join(a))
    }

    /// A selection's settings: the defaults with its own on top.
    pub fn settings_for(&self, sel: &Selection) -> Settings {
        let mut s = self.defaults.clone();
        s.extend(sel.settings.clone());
        s
    }
}

/// Cells covering the union of `rings` (points `[a, b]`): a grid of `side`
/// cells anchored at 0, keeping every cell that overlaps a ring, merged into
/// one `[a0, b0, a1, b1]` per run of cells along `b`.
// ponytail: no antimeridian crossing.
pub fn cover(rings: &[Vec<[f64; 2]>], side: f64) -> Result<Vec<[f64; 4]>> {
    let pts = || rings.iter().flatten();
    let edge =
        |i: usize, f: fn(f64, f64) -> f64, init: f64| pts().map(|p| p[i]).fold(init, f) / side;
    let (r0, c0) = (
        edge(0, f64::min, f64::MAX).floor(),
        edge(1, f64::min, f64::MAX).floor(),
    );
    let (r1, c1) = (
        edge(0, f64::max, f64::MIN).ceil(),
        edge(1, f64::max, f64::MIN).ceil(),
    );
    let rows = (r1 - r0).max(1.0) as usize;
    let cols = (c1 - c0).max(1.0) as usize;
    if rows.saturating_mul(cols) > 4_000_000 {
        bail!("polygon spans {rows} x {cols} cells; raise unit_regions or lower scale");
    }
    let at = |r: usize, c: usize| [(r0 + r as f64) * side, (c0 + c as f64) * side];
    let mut out = vec![];
    for r in 0..rows {
        let mut run: Option<usize> = None;
        for c in 0..=cols {
            let ([a0, b0], [a1, b1]) = (at(r, c), at(r + 1, c + 1));
            let hit = c < cols && overlaps(rings, [a0, b0, a1, b1]);
            match (hit, run) {
                (true, None) => run = Some(c),
                (false, Some(start)) => {
                    out.push([a0, at(r, start)[1], a1, b0]);
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
    use crate::frame::Frame;

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
osm_pbf = \"x.osm.pbf\"
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
            cover(&l, 1.0).unwrap(),
            [[0.0, 0.0, 1.0, 2.0], [1.0, 0.0, 2.0, 1.0]]
        );
        let sliver = vec![
            vec![[0.0, 0.0], [0.0, 3.0], [0.01, 3.0], [0.01, 0.0]],
            vec![[2.5, 2.5], [2.6, 2.5], [2.6, 2.6]],
        ];
        assert_eq!(
            cover(&sliver, 1.0).unwrap(),
            [[0.0, 0.0, 1.0, 3.0], [2.0, 2.0, 3.0, 3.0]]
        );
        // Cells sit on the lattice from 0, not on the shape's corner.
        assert_eq!(
            cover(&[vec![[0.5, 0.5], [0.5, 0.7], [0.7, 0.6]]], 1.0).unwrap(),
            [[0.0, 0.0, 1.0, 1.0]]
        );

        // In a project: runs of whole pieces (scale 0.5, 4-region pieces:
        // 2048-block cells) on the lattice of the world's frame.
        let text = GOOD.replace(
            "bbox = [47.15, 9.52, 47.16, 9.53]",
            "polygon = [[[47.15, 9.52], [47.25, 9.52], [47.15, 9.66]]]",
        );
        let on_lattice = |p: &Project, frame: Frame| {
            let parts: Vec<_> = p
                .selections
                .iter()
                .filter(|s| s.part_of.is_some())
                .collect();
            assert!(parts.len() > 1 && parts[0].id == "b-1");
            for s in parts {
                let [south, west, north, east] = s.bbox;
                for v in [frame.z(south), frame.x(west), frame.z(north), frame.x(east)] {
                    let off = v / 2048.0 - (v / 2048.0).round();
                    assert!(off.abs() < 1e-9, "{} edge {v} is off the lattice", s.id);
                }
                assert_eq!(s.world, "One");
                assert_eq!(s.settings["caves"].as_bool(), Some(true));
                assert_eq!(
                    s.settings["origin"].as_str(),
                    Some(frame.origin_arg().as_str())
                );
            }
            assert_eq!(p.shapes[0].id, "b");
        };
        // No world yet: the frame of the world's first selection, `a`, centred.
        let p = Project::parse(&text).unwrap();
        on_lattice(&p, Frame::new(47.14, 9.5215, 0.5));

        // A world on disk: its manifest's frame.
        let dir = std::env::temp_dir().join(format!("meld2-frame-{}", std::process::id()));
        let world = dir.join("saves").join("One");
        std::fs::create_dir_all(&world).unwrap();
        std::fs::write(
            world.join("arnis_one_world.json"),
            r#"{"origin_lat": 47.2, "origin_lon": 9.6, "scale": 0.5, "areas": []}"#,
        )
        .unwrap();
        std::fs::write(dir.join("p.toml"), &text).unwrap();
        let p = Project::load(&dir.join("p.toml")).unwrap();
        on_lattice(&p, Frame::new(47.2, 9.6, 0.5));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
