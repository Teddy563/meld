//! Named presets: a project's `[defaults]` saved under a name and applied to
//! another project, Meld 1's "my look, your place" (`presets.py`). A preset
//! is `<data>/presets/<name>.toml` holding `description` and `[defaults]`.
//!
//! Like Meld 1, a preset carries the look of the world and nothing about the
//! machine or the place: worker, thread, memory and piece sizes, paths,
//! private endpoints, raw arguments and the origin are stripped on save and
//! again on load, so a hand-edited file cannot bring them back.

use crate::project::{Project, Settings};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Keys about the machine, its files or the place, never kept in a preset.
const MACHINE: &[&str] = &[
    "workers",
    "threads",
    "cpu_target",
    "ram_budget_mb",
    "max_downloads",
    "unit_regions",
    "tree_pack_dir",
    "loot_table",
    "osm_pbf",
    "osm_pbf_url",
    "osm_tiles_url",
    "overpass_url",
    "extra_args",
    "origin",
    "offline",
];

pub fn dir(data: &Path) -> PathBuf {
    data.join("presets")
}

/// Preset names: letters, digits, `-` and `_`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn file(data: &Path, name: &str) -> Result<PathBuf> {
    if !valid_name(name) {
        bail!("preset names use letters, digits, - and _");
    }
    Ok(dir(data).join(format!("{name}.toml")))
}

/// Splits off the keys a preset never keeps; returns them.
pub fn strip(settings: &mut Settings) -> Vec<String> {
    let gone: Vec<String> = settings
        .keys()
        .filter(|k| MACHINE.contains(&k.as_str()))
        .cloned()
        .collect();
    for k in &gone {
        settings.remove(k);
    }
    gone
}

#[derive(Debug, serde::Serialize)]
pub struct Preset {
    pub name: String,
    pub description: String,
    pub defaults: Settings,
}

/// Every preset, by name.
pub fn list(data: &Path) -> Vec<Preset> {
    let mut out: Vec<Preset> = std::fs::read_dir(dir(data))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e
                .file_name()
                .to_string_lossy()
                .strip_suffix(".toml")?
                .to_string();
            load(data, &name).ok()
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn load(data: &Path, name: &str) -> Result<Preset> {
    let f = file(data, name)?;
    let text = std::fs::read_to_string(&f).with_context(|| format!("no preset {name:?}"))?;
    parse(name, &text).with_context(|| format!("in {}", f.display()))
}

/// A preset file's text: `description` and `[defaults]`; anything else (an
/// imported Meld 1 preset's `[run]`) is about the machine and is dropped.
pub fn parse(name: &str, text: &str) -> Result<Preset> {
    let t: toml::Table = text.parse()?;
    let mut defaults: Settings = match t.get("defaults") {
        Some(d) => d.clone().try_into()?,
        None => Settings::new(),
    };
    strip(&mut defaults);
    crate::args::check(&defaults).context("in [defaults]")?;
    Ok(Preset {
        name: name.into(),
        description: t
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or_default()
            .into(),
        defaults,
    })
}

/// Saves `defaults` as preset `name` (replacing one of that name); returns
/// the keys it left out.
pub fn save(
    data: &Path,
    name: &str,
    description: &str,
    defaults: &Settings,
) -> Result<Vec<String>> {
    let f = file(data, name)?;
    let mut defaults = defaults.clone();
    let gone = strip(&mut defaults);
    crate::args::check(&defaults).context("in [defaults]")?;
    let mut t = toml::Table::new();
    t.insert("description".into(), description.into());
    t.insert(
        "defaults".into(),
        toml::Value::Table(defaults.into_iter().collect()),
    );
    std::fs::create_dir_all(dir(data))?;
    std::fs::write(&f, format!("# A Meld preset\n{}", toml::to_string(&t)?))?;
    Ok(gone)
}

pub fn delete(data: &Path, name: &str) -> Result<()> {
    std::fs::remove_file(file(data, name)?).with_context(|| format!("no preset {name:?}"))
}

/// The project file's text with the preset's settings laid over its
/// `[defaults]` (the preset wins), checked by loading it. Comments are not kept.
pub fn apply(project_text: &str, preset: &Preset) -> Result<String> {
    let mut t: toml::Table = project_text.parse()?;
    let d = t
        .entry("defaults")
        .or_insert_with(|| toml::Value::Table(Default::default()));
    let toml::Value::Table(d) = d else {
        bail!("[defaults] is not a table");
    };
    for (k, v) in &preset.defaults {
        d.insert(k.clone(), v.clone());
    }
    let text = toml::to_string(&t)?;
    Project::parse(&text).context("the project with the preset does not load")?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_strips_machine_keys_and_apply_overlays() {
        let data = std::env::temp_dir().join(format!("meld2-preset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let mut d = Settings::new();
        d.insert("caves".into(), true.into());
        d.insert("scale".into(), 0.5.into());
        d.insert("workers".into(), 6.into());
        d.insert("origin".into(), "47,9".into());
        let gone = save(&data, "alpine", "Caves at half scale", &d).unwrap();
        assert_eq!(gone, ["origin", "workers"]);
        assert!(save(&data, "../x", "", &d).is_err());

        // A hand-edited file cannot bring a machine key back.
        let f = dir(&data).join("hand.toml");
        std::fs::write(
            &f,
            "[defaults]\nthreads = 64\nsnow_mode = \"off\"\n[run]\njobs = 9\n",
        )
        .unwrap();
        let names: Vec<_> = list(&data).into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["alpine", "hand"]);
        let hand = load(&data, "hand").unwrap();
        assert!(!hand.defaults.contains_key("threads"));

        let p = load(&data, "alpine").unwrap();
        assert_eq!(p.description, "Caves at half scale");
        let project =
            "format = 1\nname = \"P\"\noutput = \"saves\"\n[defaults]\nscale = 1.0\nseed = 3\n";
        let out = Project::parse(&apply(project, &p).unwrap()).unwrap();
        assert_eq!(out.defaults["scale"].as_float(), Some(0.5));
        assert_eq!(out.defaults["seed"].as_integer(), Some(3));
        assert_eq!(out.defaults["caves"].as_bool(), Some(true));
        delete(&data, "hand").unwrap();
        assert!(load(&data, "hand").is_err());
        std::fs::remove_dir_all(data).unwrap();
    }
}
