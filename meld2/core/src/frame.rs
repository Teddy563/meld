//! A One World's frame: Web Mercator with block (0, 0) at the world's
//! origin, the same math as Arnis's `projection/web_mercator.rs`, and the
//! parts of `arnis_one_world.json` Meld reads (origin, scale, areas).

use crate::project::Settings;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

/// Arnis's spherical Earth (WGS84 mean radius).
const EARTH_RADIUS: f64 = 6_371_000.0;

/// Block (0, 0) at `origin_lat, origin_lon`; x grows east, z grows south;
/// a block is `1 / scale` metres at the origin latitude.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub origin_lat: f64,
    pub origin_lon: f64,
    pub scale: f64,
}

fn mercator_y(lat: f64) -> f64 {
    EARTH_RADIUS
        * (std::f64::consts::FRAC_PI_4 + lat.to_radians() / 2.0)
            .tan()
            .ln()
}

impl Frame {
    /// A new world's frame as Arnis stores it: the origin rounded to 7 decimals.
    pub fn new(origin_lat: f64, origin_lon: f64, scale: f64) -> Self {
        let r = |v: f64| (v * 1e7).round() / 1e7;
        Self {
            origin_lat: r(origin_lat),
            origin_lon: r(origin_lon),
            scale,
        }
    }

    fn k(&self) -> f64 {
        self.scale * self.origin_lat.to_radians().cos()
    }

    pub fn x(&self, lon: f64) -> f64 {
        EARTH_RADIUS * (lon - self.origin_lon).to_radians() * self.k()
    }

    pub fn z(&self, lat: f64) -> f64 {
        -(mercator_y(lat) - mercator_y(self.origin_lat)) * self.k()
    }

    pub fn lon(&self, x: f64) -> f64 {
        self.origin_lon + (x / (EARTH_RADIUS * self.k())).to_degrees()
    }

    pub fn lat(&self, z: f64) -> f64 {
        let y = mercator_y(self.origin_lat) - z / self.k();
        (2.0 * ((y / EARTH_RADIUS).exp().atan() - std::f64::consts::FRAC_PI_4)).to_degrees()
    }

    /// The `--origin` value that gives a new world this frame.
    pub fn origin_arg(&self) -> String {
        format!("{},{}", self.origin_lat, self.origin_lon)
    }
}

/// What Meld reads of a world's `arnis_one_world.json`.
#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub origin_lat: f64,
    pub origin_lon: f64,
    pub scale: f64,
    #[serde(default)]
    pub ground_level: i64,
    #[serde(default = "yes")]
    pub terrain: bool,
    #[serde(default)]
    pub disable_height_limit: bool,
    #[serde(default)]
    pub aws_only_elevation: bool,
    #[serde(default = "one")]
    pub height_multiplier: f64,
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub areas: Vec<Area>,
}

fn yes() -> bool {
    true
}

fn one() -> f64 {
    1.0
}

/// One area the world holds, as inclusive block bounds.
#[derive(Debug, Deserialize)]
pub struct Area {
    pub min_x: i64,
    pub min_z: i64,
    pub max_x: i64,
    pub max_z: i64,
}

pub const MANIFEST: &str = "arnis_one_world.json";

impl Manifest {
    /// The world's manifest, or `None` when the world does not exist yet.
    pub fn load(world_dir: &Path) -> Result<Option<Self>> {
        let file = world_dir.join(MANIFEST);
        match std::fs::read_to_string(&file) {
            Ok(text) => serde_json::from_str(&text)
                .map(Some)
                .with_context(|| format!("reading {}", file.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", file.display())),
        }
    }

    pub fn frame(&self) -> Frame {
        Frame {
            origin_lat: self.origin_lat,
            origin_lon: self.origin_lon,
            scale: self.scale,
        }
    }

    /// The bounds of every area, `[min_x, min_z, max_x, max_z]` inclusive:
    /// what Arnis's `--world-border` surrounds.
    pub fn extent(&self) -> Option<[i64; 4]> {
        let first = self.areas.first()?;
        Some(self.areas.iter().fold(
            [first.min_x, first.min_z, first.max_x, first.max_z],
            |e, a| {
                [
                    e[0].min(a.min_x),
                    e[1].min(a.min_z),
                    e[2].max(a.max_x),
                    e[3].max(a.max_z),
                ]
            },
        ))
    }

    /// The world's build height, for regions that span it.
    pub fn y_range(&self) -> (i32, i32) {
        if self.disable_height_limit {
            (-2032, 2031)
        } else {
            (-64, 319)
        }
    }
}

/// A world setting this run passes with another value than the world was
/// created with.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Diff {
    pub key: &'static str,
    pub label: &'static str,
    /// This project's value, and the world's, as the settings form shows them.
    pub ours: String,
    pub world: String,
    /// The world's value as the setting, for "use the world's value".
    pub fix: toml::Value,
    /// Arnis refuses to add an area over it (`one_world.rs`
    /// `compatibility_errors`); over the others it keeps the world's value.
    pub refuses: bool,
}

/// What Meld does with a selection whose world exists with other settings.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Build: the differences are ones Arnis settles by keeping the world's.
    Build,
    /// Nothing is built in it: move it aside and start the world again.
    Fresh,
    /// Areas are built and Arnis would refuse: stop before spawning it.
    Refuse,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct WorldCheck {
    pub world: String,
    pub areas: usize,
    pub action: Action,
    pub diffs: Vec<Diff>,
    pub message: String,
}

/// `world_dir`'s manifest against the frame settings in `settings` (a
/// selection's, as `args::build` passes them). `None`: no world yet, or no
/// difference.
pub fn check_world(world_dir: &Path, settings: &Settings) -> Result<Option<WorldCheck>> {
    let Some(m) = Manifest::load(world_dir)? else {
        return Ok(None);
    };
    let s = crate::args::with_frame(settings);
    let num = |k: &str| crate::args::text(&s, k).and_then(|v| v.parse::<f64>().ok());
    let on = |k: &str| s.get(k).and_then(toml::Value::as_bool).unwrap_or(false);
    let onoff = |b: bool| if b { "on" } else { "off" }.to_string();
    let mut diffs = vec![];
    let mut diff = |key, label, ours: String, world: String, fix, refuses| {
        diffs.push(Diff {
            key,
            label,
            ours,
            world,
            fix,
            refuses,
        })
    };
    if let Some(v) = num("scale").filter(|v| (v - m.scale).abs() > 1e-9) {
        let fix = toml::Value::Float(m.scale);
        diff(
            "scale",
            "World Scale",
            v.to_string(),
            m.scale.to_string(),
            fix,
            true,
        );
    }
    if let Some(v) = num("ground_level").filter(|v| *v != m.ground_level as f64) {
        let fix = toml::Value::Integer(m.ground_level);
        let world = m.ground_level.to_string();
        diff(
            "ground_level",
            "Ground Level",
            v.to_string(),
            world,
            fix,
            true,
        );
    }
    let mode = crate::args::text(&s, "mode").unwrap_or_default();
    if (mode != "geo-only") != m.terrain {
        let world = if m.terrain { "geo-terrain" } else { "geo-only" };
        let fix = toml::Value::String(world.into());
        diff("mode", "Generation Mode", mode, world.into(), fix, true);
    }
    if let Some(v) = num("height_multiplier").filter(|v| (v - m.height_multiplier).abs() > 1e-6) {
        let (w, fix) = (m.height_multiplier, toml::Value::Float(m.height_multiplier));
        diff(
            "height_multiplier",
            "Terrain Height",
            v.to_string(),
            w.to_string(),
            fix,
            false,
        );
    }
    for (key, label, world) in [
        (
            "disable_height_limit",
            "Extend build height",
            m.disable_height_limit,
        ),
        ("aws_only_elevation", "Legacy Terrain", m.aws_only_elevation),
    ] {
        // Arnis extends every new One World whatever the setting, so only
        // "on" against a vanilla world is a difference.
        let forced = key == "disable_height_limit" && world;
        if on(key) != world && !forced {
            diff(
                key,
                label,
                onoff(on(key)),
                onoff(world),
                world.into(),
                false,
            );
        }
    }
    // Unset and 0 are both Arnis's default look.
    let seed = num("seed").map(|v| v as u64).filter(|v| *v != 0);
    let world_seed = m.seed.filter(|v| *v != 0);
    if seed != world_seed {
        let show = |v: Option<u64>| v.map_or("default".into(), |v| v.to_string());
        let fix = toml::Value::Integer(world_seed.unwrap_or(0) as i64);
        diff(
            "seed",
            "World Seed",
            show(seed),
            show(world_seed),
            fix,
            false,
        );
    }
    if let Some(o) = crate::args::text(&s, "origin") {
        let r = |v: f64| (v * 1e7).round() / 1e7;
        let ll: Vec<f64> = o.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        if ll.len() == 2 && (r(ll[0]), r(ll[1])) != (m.origin_lat, m.origin_lon) {
            let world = format!("{},{}", m.origin_lat, m.origin_lon);
            diff("origin", "Origin", o, world.clone(), world.into(), false);
        }
    }
    if diffs.is_empty() {
        return Ok(None);
    }
    let world = world_dir
        .file_name()
        .map_or(String::new(), |n| n.to_string_lossy().into_owned());
    let areas = m.areas.len();
    let named = |only_refusing: bool| {
        diffs
            .iter()
            .filter(|d| d.refuses || !only_refusing)
            .map(|d| {
                format!(
                    "{} is {} here but {} in the world",
                    d.label, d.ours, d.world
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    let (action, message) = if areas == 0 {
        let why = named(false);
        (
            Action::Fresh,
            format!("World '{world}' was created with other settings ({why}) but has nothing built; starting it fresh."),
        )
    } else if diffs.iter().any(|d| d.refuses) {
        let why = named(true);
        (
            Action::Refuse,
            format!("World '{world}' has {areas} area(s) built with other settings: {why}. Use the world's value, or build into a new world name."),
        )
    } else {
        let why = named(false);
        (
            Action::Build,
            format!("World '{world}' keeps the settings it was created with: {why}."),
        )
    };
    Ok(Some(WorldCheck {
        world,
        areas,
        action,
        diffs,
        message,
    }))
}

/// Moves an empty world aside as `<world>.empty-<unix seconds>` (nothing is
/// deleted), so the next run creates it again; returns the new folder.
pub fn set_aside(world_dir: &Path) -> Result<std::path::PathBuf> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let name = world_dir
        .file_name()
        .context("a world folder has a name")?
        .to_string_lossy();
    let to = world_dir.with_file_name(format!("{name}.empty-{now}"));
    std::fs::rename(world_dir, &to)
        .with_context(|| format!("moving {} to {}", world_dir.display(), to.display()))?;
    Ok(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world(dir: &Path, areas: &str) {
        std::fs::create_dir_all(dir).unwrap();
        // The user's `buc` manifest, as Arnis 3.4.0-beta.1 wrote it.
        let text = include_str!("../tests/fixtures/buc-manifest.json")
            .replace(r#""areas": []"#, &format!(r#""areas": [{areas}]"#));
        std::fs::write(dir.join(MANIFEST), text).unwrap();
    }

    #[test]
    fn world_check_starts_an_empty_world_again_and_refuses_a_built_one() {
        let root = std::env::temp_dir().join(format!("meld2-wcheck-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("buc");
        let mut s = Settings::new();
        // No world yet, then the settings it was made with: nothing to say.
        assert!(check_world(&dir, &s).unwrap().is_none());
        world(&dir, "");
        s.insert("ground_level".into(), 0.into());
        s.insert("seed".into(), 563.into());
        assert!(check_world(&dir, &s).unwrap().is_none());

        // The user's run: ground level unset (-62) against a world of 0, nothing built.
        let c = check_world(&dir, &Settings::new()).unwrap().unwrap();
        assert_eq!(c.action, Action::Fresh);
        // Arnis extended the world by itself; that is no difference.
        let keys: Vec<_> = c.diffs.iter().map(|d| d.key).collect();
        assert_eq!(keys, ["ground_level", "seed"]);
        assert_eq!(
            (c.diffs[0].ours.as_str(), c.diffs[0].world.as_str()),
            ("-62", "0")
        );
        assert!(
            c.message
                .starts_with("World 'buc' was created with other settings")
                && c.message
                    .ends_with("but has nothing built; starting it fresh."),
            "{}",
            c.message
        );
        let moved = set_aside(&dir).unwrap();
        assert!(!dir.exists() && moved.join(MANIFEST).is_file());
        assert!(moved
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("buc.empty-"));

        // One area built: a refusal naming each setting Arnis refuses, with both values.
        world(
            &dir,
            r#"{"id":1,"min_x":0,"min_z":0,"max_x":15,"max_z":15}"#,
        );
        let mut other = s.clone();
        other.insert("ground_level".into(), (-62).into());
        other.insert("scale".into(), 0.5.into());
        let c = check_world(&dir, &other).unwrap().unwrap();
        assert_eq!((c.action, c.areas), (Action::Refuse, 1));
        for part in [
            "World Scale is 0.5 here but 1 in the world",
            "Ground Level is -62 here but 0 in the world",
            "Use the world's value, or build into a new world name",
        ] {
            assert!(c.message.contains(part), "{}", c.message);
        }
        let gl = c.diffs.iter().find(|d| d.key == "ground_level").unwrap();
        assert_eq!(gl.fix, toml::Value::Integer(0));
        // Taking the world's values makes it build.
        for d in &c.diffs {
            other.insert(d.key.into(), d.fix.clone());
        }
        assert!(check_world(&dir, &other).unwrap().is_none());

        // Only what Arnis keeps differs: it builds, and says what the world keeps.
        let mut kept = s.clone();
        kept.insert("seed".into(), 7.into());
        let c = check_world(&dir, &kept).unwrap().unwrap();
        assert_eq!(c.action, Action::Build);
        assert!(
            c.message
                .contains("World Seed is 7 here but 563 in the world"),
            "{}",
            c.message
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Arnis's own checks (web_mercator.rs tests): the origin is block 0,
    /// one metre is one block at the origin, north is -z, and it round-trips.
    #[test]
    fn matches_arnis_web_mercator() {
        let f = Frame::new(48.8566, 2.3522, 1.0);
        assert!(f.x(2.3522).abs() < 1e-9 && f.z(48.8566).abs() < 1e-9);
        let d = 0.0005_f64;
        let ground = EARTH_RADIUS * (2.0 * d).to_radians();
        let ratio = (f.z(48.8566 - d) - f.z(48.8566 + d)) / ground;
        assert!((ratio - 1.0).abs() < 1e-3, "{ratio}");
        assert!(f.z(49.0) < 0.0 && f.x(3.0) > 0.0);
        for (lat, lon) in [(47.14, 9.52), (-33.9, 151.2), (64.1, -21.9)] {
            let (x, z) = (f.x(lon), f.z(lat));
            assert!((f.lat(z) - lat).abs() < 1e-9 && (f.lon(x) - lon).abs() < 1e-9);
        }
        assert_eq!(Frame::new(47.123456789, 9.5, 2.0).origin_lat, 47.1234568);
    }
}
