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
//! settings = { caves = true }
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
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            jobs: 1,
            cpu_target: 90,
            ram_budget_mb: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub id: String,
    /// min_lat, min_lng, max_lat, max_lng: the order of Arnis's --bbox.
    pub bbox: [f64; 4],
    /// One World folder in `output`. Selections sharing a world extend it one after another.
    pub world: String,
    #[serde(default)]
    pub settings: Settings,
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
        let project: Self = table.try_into()?;
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
            let ok_id = !s.id.is_empty()
                && s.id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if !ok_id {
                bail!("selection id {:?}: use letters, digits, - and _", s.id);
            }
            if !ids.insert(s.id.as_str()) {
                bail!("selection id {:?} is used twice", s.id);
            }
            if s.world.trim().is_empty() {
                bail!("selection {}: empty world", s.id);
            }
            let [s_lat, w_lng, n_lat, e_lng] = s.bbox;
            let valid = s.bbox.iter().all(|v| v.is_finite())
                && (-90.0..=90.0).contains(&s_lat)
                && (-90.0..=90.0).contains(&n_lat)
                && (-180.0..=180.0).contains(&w_lng)
                && (-180.0..=180.0).contains(&e_lng)
                && s_lat < n_lat
                && w_lng < e_lng;
            if !valid {
                bail!(
                    "selection {}: bbox must be [min_lat, min_lng, max_lat, max_lng]",
                    s.id
                );
            }
            args::check(&s.settings).with_context(|| format!("in selection {}", s.id))?;
        }
        Ok(())
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
}
