//! A Minecraft server for a project's worlds, from Meld 1's `mcserver.py`:
//! a Leaf (or Paper) folder with the built worlds linked or copied in, its
//! config, datapacks, plugins and WorldGuard regions, and the server run as
//! a managed process (`meld2 server start|stop|status`, and over `serve`).
//!
//! Nothing runs through a shell: Java gets an argument list, and the only
//! console input Meld sends is `stop` and fixed Multiverse imports of the
//! folder names it chose. A folder Meld did not set up, a copied world and
//! the files an admin edits (server.properties, regions.yml) are replaced
//! only with `--force`. The EULA is accepted only when asked to.

use crate::arnis::Process;
use crate::frame::Manifest;
use crate::project::Project;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The `[server]` table of a project.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Conf {
    /// The server folder, relative to the project file.
    pub dir: PathBuf,
    pub flavor: Flavor,
    /// The Minecraft version; B_Linear needs Leaf 1.21.11 or newer.
    pub version: String,
    /// The worlds to serve, the first as the main level. Unset: every world
    /// of the project in order. More than one adds Multiverse-Core.
    pub worlds: Vec<String>,
    /// `blinear` converts each served world after its build and serves the
    /// `<World> [BLinear]` copies; `mca` serves the built worlds.
    pub format: Format,
    pub ip: String,
    pub port: u16,
    pub ram_mb: u32,
    /// Link the worlds into the folder (a junction on Windows) instead of copying them.
    pub link: bool,
    /// Modrinth project slugs, e.g. "worldguard", "worldedit", "voxy-server-side";
    /// `slug@version` pins a build (e.g. one that runs on the Java you have).
    pub plugins: Vec<String>,
    /// Datapack zips or folders for the main level's `datapacks/`.
    pub datapacks: Vec<PathBuf>,
    pub java: Option<PathBuf>,
    pub motd: String,
    /// Flags, owners and members of the WorldGuard regions Meld writes.
    pub worldguard: Guard,
    /// SkBee particle walls along every region, in
    /// `plugins/Skript/scripts/meld-border.sk` (adds the Skript and SkBee plugins).
    pub skript_walls: bool,
}

/// `[server.worldguard]`: what every `meld-<id>` region gets, a
/// selection's own under `[server.worldguard.selection.<id>]`, and the
/// `__global__` region's flags (outside every selection), e.g.
/// `global_flags = { block-break = "deny", block-place = "deny" }`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Guard {
    pub flags: BTreeMap<String, toml::Value>,
    pub owners: Vec<String>,
    pub members: Vec<String>,
    pub global_flags: BTreeMap<String, toml::Value>,
    pub selection: BTreeMap<String, Region>,
}

/// One selection's region: its flags go over the shared ones; its owners
/// and members, when given, replace them.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Region {
    pub flags: BTreeMap<String, toml::Value>,
    pub owners: Vec<String>,
    pub members: Vec<String>,
}

impl Default for Conf {
    fn default() -> Self {
        Self {
            dir: "server".into(),
            flavor: Flavor::Leaf,
            version: "1.21.11".into(),
            worlds: vec![],
            format: Format::Mca,
            ip: "127.0.0.1".into(),
            port: 25565,
            ram_mb: 4096,
            link: true,
            plugins: vec![],
            datapacks: vec![],
            java: None,
            motd: "A Meld world".into(),
            worldguard: Guard::default(),
            skript_walls: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Flavor {
    #[default]
    Leaf,
    Paper,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Mca,
    Blinear,
}

const LEAF_API: &str = "https://api.leafmc.one/v2/projects/leaf";
const PAPER_API: &str = "https://fill.papermc.io/v3/projects/paper";
const MODRINTH_API: &str = "https://api.modrinth.com/v2";
/// What `meld2 server setup` records for `start`.
const META: &str = "meld-server.json";
const LOCK: &str = "meld-server.lock";
const STOP: &str = "meld-server.stop";
const STATUS: &str = "meld-server.status.json";
/// Console commands waiting for the running server, one file each (`server send`).
const INBOX: &str = "meld-server.in";
/// Aikar's flags, as Meld 1 starts its servers with them.
const JVM_FLAGS: &[&str] = &[
    "-XX:+UseG1GC",
    "-XX:+ParallelRefProcEnabled",
    "-XX:MaxGCPauseMillis=200",
    "-XX:+UnlockExperimentalVMOptions",
    "-XX:+DisableExplicitGC",
    "-XX:G1HeapRegionSize=8M",
    "-XX:G1NewSizePercent=30",
    "-XX:G1MaxNewSizePercent=40",
    "-XX:G1ReservePercent=20",
    "-XX:InitiatingHeapOccupancyPercent=15",
    "-Dusing.aikars.flags=https://mcflags.emc.gs",
    "-Daikars.new.flags=true",
];

impl Conf {
    /// Checks the table against the project's worlds and selection ids.
    pub fn check(&self, worlds: &[String], ids: &[&str]) -> Result<()> {
        let plain = |s: &str, extra: &[char]| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || extra.contains(&c))
        };
        if !plain(&self.version, &['.', '-']) {
            bail!("[server] version {:?}: e.g. \"1.21.11\"", self.version);
        }
        if self.format == Format::Blinear && self.flavor != Flavor::Leaf {
            bail!("[server] format = \"blinear\": only Leaf (1.21.11 or newer) reads B_Linear");
        }
        let spec = |s: &str| match s.split_once('@') {
            Some((slug, v)) => plain(slug, &['-', '_']) && plain(v, &['-', '_', '.', '+']),
            None => plain(s, &['-', '_']),
        };
        if let Some(s) = self.plugins.iter().find(|s| !spec(s)) {
            bail!("[server] plugins: {s:?} is not a Modrinth slug");
        }
        if let Some(w) = self.worlds.iter().find(|w| !worlds.contains(w)) {
            bail!("[server] worlds: no selection builds {w:?}");
        }
        self.ip
            .parse::<std::net::IpAddr>()
            .with_context(|| format!("[server] ip {:?}", self.ip))?;
        if self.ram_mb < 512 {
            bail!("[server] ram_mb must be at least 512");
        }
        let g = &self.worldguard;
        if let Some(id) = g.selection.keys().find(|k| !ids.contains(&k.as_str())) {
            bail!("[server.worldguard.selection.{id}]: no selection {id:?}");
        }
        let regions = std::iter::once((&g.flags, &g.owners, &g.members)).chain(
            g.selection
                .values()
                .map(|r| (&r.flags, &r.owners, &r.members)),
        );
        for (flags, owners, members) in regions {
            for (k, v) in flags.iter().chain(&g.global_flags) {
                if !plain(k, &['-']) || yaml_scalar(v).is_none() {
                    bail!("[server.worldguard] flag {k:?} = {v}: a flag name and a plain value");
                }
            }
            // Minecraft names: 3-16 letters, digits and _.
            if let Some(p) = owners
                .iter()
                .chain(members)
                .find(|p| !(3..=16).contains(&p.len()) || !plain(p, &['_']))
            {
                bail!("[server.worldguard] {p:?} is not a player name");
            }
        }
        Ok(())
    }
}

/// The server's settings and where things are, for one project.
pub struct Server<'a> {
    pub project: &'a Project,
    pub conf: Conf,
    pub dir: PathBuf,
}

/// A world's folder name inside the server: letters, digits, `-` and `_`.
pub fn level_name(world: &str) -> String {
    let s: String = world
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    match s.trim_matches('_') {
        "" => "world".into(),
        t => t.into(),
    }
}

/// What `setup` records for `start`.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Meta {
    jar: String,
    level: String,
    /// Console commands for the first start, sent once after `Done (`.
    first_start: Vec<String>,
    /// The plugin jars Meld put in `plugins/`; one no longer wanted goes.
    plugins: Vec<String>,
}

/// What `status` reports.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    pub running: bool,
    pub pid: Option<u32>,
    /// `starting`, `ready` or `stopping` while it runs.
    pub state: String,
    pub dir: PathBuf,
    pub log: Vec<String>,
}

impl<'a> Server<'a> {
    pub fn of(project: &'a Project) -> Result<Self> {
        let mut conf = project
            .server
            .clone()
            .context("the project has no [server] table")?;
        if conf.worlds.is_empty() {
            conf.worlds = project.worlds();
        }
        let dir = project
            .path
            .parent()
            .unwrap_or(Path::new("."))
            .join(&conf.dir);
        Ok(Self { project, conf, dir })
    }

    /// The built world `world` is served from: the build, or its B_Linear copy.
    pub fn source(&self, world: &str) -> PathBuf {
        let built = self.project.output_dir().join(world);
        match self.conf.format {
            Format::Mca => built,
            Format::Blinear => crate::convert::sibling(&built),
        }
    }

    fn java_major(&self) -> u32 {
        if self.conf.version.starts_with("1.") {
            21
        } else {
            25
        }
    }

    /// Sets the folder up. `say` gets one line per thing done or kept.
    pub fn setup(
        &self,
        force: bool,
        accept_eula: bool,
        download: bool,
        say: &mut dyn FnMut(String),
    ) -> Result<()> {
        let dir = &self.dir;
        let ours = dir.join(META).is_file();
        let empty = std::fs::read_dir(dir).map_or(true, |mut d| d.next().is_none());
        if !ours && !empty && !force {
            bail!(
                "{} exists and Meld did not set it up; pick another [server] dir, or pass --force to set it up there",
                dir.display()
            );
        }
        for w in &self.conf.worlds {
            let src = self.source(w);
            if !src.join("level.dat").is_file() {
                bail!(
                    "{} is not built yet{}; run the project first",
                    src.display(),
                    match self.conf.format {
                        Format::Blinear => " (format = blinear: the run converts it)",
                        Format::Mca => "",
                    }
                );
            }
        }
        std::fs::create_dir_all(dir.join("plugins"))?;
        let level = level_name(&self.conf.worlds[0]);
        let mut first_start = vec![];
        // Worlds Multiverse already holds: `Name:` (4.x) or `minecraft:name:` (5.x).
        let known = std::fs::read_to_string(dir.join("plugins/Multiverse-Core/worlds.yml"))
            .unwrap_or_default();
        let imported = |n: &str| {
            known
                .lines()
                .any(|l| l == format!("{n}:") || l.eq_ignore_ascii_case(&format!("minecraft:{n}:")))
        };
        for (i, w) in self.conf.worlds.iter().enumerate() {
            let name = level_name(w);
            self.place(&self.source(w), &dir.join(&name), force, say)?;
            if i > 0 && !imported(&name) {
                first_start.push(format!("mv import {name} normal"));
            }
            let regions = dir
                .join("plugins/WorldGuard/worlds")
                .join(&name)
                .join("regions.yml");
            if let Some(yml) = self.regions_yml(w)? {
                write_new(&regions, &yml, force, say)?;
            }
        }
        if self.conf.skript_walls {
            let sk = dir.join("plugins/Skript/scripts/meld-border.sk");
            write_new(&sk, &self.skript()?, true, say)?;
        }
        for pack in &self.conf.datapacks {
            let pack = self
                .project
                .path
                .parent()
                .unwrap_or(Path::new("."))
                .join(pack);
            let name = pack.file_name().context("a datapack path has no name")?;
            let to = dir.join(&level).join("datapacks").join(name);
            if to.exists() && !force {
                say(format!(
                    "kept {} (exists; --force replaces it)",
                    to.display()
                ));
                continue;
            }
            copy_tree(&pack, &to).with_context(|| format!("copying {}", pack.display()))?;
            say(format!("datapack {}", to.display()));
        }
        write_new(
            &dir.join("server.properties"),
            &self.properties(&level),
            force,
            say,
        )?;
        if self.conf.flavor == Flavor::Leaf {
            let fmt = match self.conf.format {
                Format::Mca => "MCA",
                Format::Blinear => "B_LINEAR",
            };
            // Leaf fills in its other defaults on first boot.
            let yml = format!(
                "# Written by Meld: the region format of the worlds it placed here.\nconfig-version: '3.0'\nmisc:\n  region-format:\n    format-name: {fmt}\n    compress-level: 6\n"
            );
            write_new(&dir.join("config/leaf-global.yml"), &yml, true, say)?;
        }
        let eula = dir.join("eula.txt");
        if accept_eula {
            std::fs::write(
                &eula,
                "# Accepted through Meld on request: https://aka.ms/MinecraftEULA\neula=true\n",
            )?;
            say("eula.txt: accepted (--accept-eula)".into());
        } else if !eula.exists() {
            std::fs::write(&eula, "# Running the server means agreeing to https://aka.ms/MinecraftEULA\n# `meld2 server setup --accept-eula` sets this to true.\neula=false\n")?;
            say(
                "eula.txt: not accepted; pass --accept-eula once you agree to the Minecraft EULA"
                    .into(),
            );
        }

        let mut meta: Meta = read_json(&dir.join(META)).unwrap_or_default();
        meta.level = level;
        meta.first_start = first_start;
        if download {
            let jar = jar(self.conf.flavor, &self.conf.version)?;
            let file = dir.join(&jar.name);
            if file.is_file() {
                crate::install::verify(&file, Sum::Sha256(&jar.sha256))?;
            } else {
                say(format!("downloading {}", jar.url));
                crate::install::download(&jar.url, &file, Sum::Sha256(&jar.sha256))?;
            }
            say(format!("server jar {} (sha256 verified)", jar.name));
            meta.jar = jar.name;
            let mut plugins = self.conf.plugins.clone();
            if self.conf.worlds.len() > 1 && !plugins.iter().any(|p| p == "multiverse-core") {
                plugins.push("multiverse-core".into());
            }
            if self.conf.skript_walls {
                for p in ["skript", "skbee"] {
                    if !plugins.iter().any(|x| x.split('@').next() == Some(p)) {
                        plugins.push(p.into());
                    }
                }
            }
            let mut installed = vec![];
            for slug in &plugins {
                let p = plugin(slug, &self.conf.version)?;
                installed.push(p.file.clone());
                let to = dir.join("plugins").join(&p.file);
                if to.is_file() {
                    crate::install::verify(&to, Sum::Sha512(&p.sha512))?;
                } else {
                    crate::install::download(&p.url, &to, Sum::Sha512(&p.sha512))?;
                }
                say(format!("plugin {} {} (sha512 verified)", slug, p.file));
            }
            for old in meta.plugins.iter().filter(|f| !installed.contains(f)) {
                if std::fs::remove_file(dir.join("plugins").join(old)).is_ok() {
                    say(format!(
                        "removed plugin {old} (no longer in [server] plugins)"
                    ));
                }
            }
            meta.plugins = installed;
        } else if meta.jar.is_empty() {
            say(
                "no server jar: run setup without --no-download before `meld2 server start`".into(),
            );
        }
        let tmp = dir.join(format!("{META}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec_pretty(&meta)?)?;
        std::fs::rename(&tmp, dir.join(META))?;
        say(format!("server ready in {}", dir.display()));
        Ok(())
    }

    /// Puts `src` at `dest`: a link (replaced freely), or a copy (replaced only with `force`).
    fn place(
        &self,
        src: &Path,
        dest: &Path,
        force: bool,
        say: &mut dyn FnMut(String),
    ) -> Result<()> {
        if let Ok(m) = std::fs::symlink_metadata(dest) {
            if m.file_type().is_symlink() {
                remove_link(dest)?;
            } else if force && dest.join("level.dat").is_file() {
                std::fs::remove_dir_all(dest)
                    .with_context(|| format!("removing {}", dest.display()))?;
            } else if dest.join("level.dat").is_file() {
                say(format!(
                    "kept {} (a world is there; --force replaces it)",
                    dest.display()
                ));
                return Ok(());
            } else {
                bail!(
                    "{} is in the way and is not a world; move it",
                    dest.display()
                );
            }
        }
        let src = std::path::absolute(src)?;
        if self.conf.link {
            link(&src, dest).with_context(|| {
                format!(
                    "linking {} to {}; set [server] link = false to copy instead",
                    dest.display(),
                    src.display()
                )
            })?;
            say(format!(
                "world {} -> {} (linked)",
                dest.display(),
                src.display()
            ));
        } else {
            let need = dir_bytes(&src)? / (1 << 20) + 512;
            let free = crate::plan::free_mb(dest.parent().unwrap_or(Path::new(".")))?;
            if free < need {
                bail!(
                    "not enough disk to copy {}: ~{need} MB needed, {free} MB free",
                    src.display()
                );
            }
            copy_tree(&src, dest)?;
            say(format!(
                "world {} (copied from {})",
                dest.display(),
                src.display()
            ));
        }
        Ok(())
    }

    fn properties(&self, level: &str) -> String {
        let local = self
            .conf
            .ip
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
        let c = &self.conf;
        let mut s = String::from(
            "# Written by Meld. Edit freely; `meld2 server setup --force` writes it again.\n",
        );
        // Offline mode only while the server listens on this machine alone.
        for (k, v) in [
            ("motd", c.motd.replace(['\n', '\r'], " ")),
            ("level-name", level.into()),
            ("server-ip", c.ip.clone()),
            ("server-port", c.port.to_string()),
            ("online-mode", (!local).to_string()),
            ("gamemode", "creative".into()),
            ("difficulty", "normal".into()),
            ("generate-structures", "false".into()),
            ("allow-nether", "false".into()),
            ("spawn-protection", "0".into()),
            ("view-distance", "10".into()),
            ("simulation-distance", "6".into()),
            ("allow-flight", "true".into()),
            ("enable-rcon", "false".into()),
            ("white-list", "false".into()),
        ] {
            let _ = writeln!(s, "{k}={v}");
        }
        s
    }

    /// A world's regions in its One World frame: `(region, selection, ring)`,
    /// a polygon's rings as drawn, a bbox as the chunks Arnis builds for it.
    #[allow(clippy::type_complexity)]
    fn rings(
        &self,
        world: &str,
    ) -> Result<Option<(Manifest, Vec<(String, String, Vec<[i64; 2]>)>)>> {
        let Some(m) = Manifest::load(&self.project.output_dir().join(world))? else {
            return Ok(None);
        };
        let f = m.frame();
        let mut rings = vec![];
        for s in self
            .project
            .selections
            .iter()
            .filter(|s| s.world == world && s.part_of.is_none())
        {
            let [south, west, north, east] = s.bbox;
            let chunk = |v: f64, up: bool| {
                let c = if up {
                    (v / 16.0 - 1e-9).ceil()
                } else {
                    (v / 16.0 + 1e-9).floor()
                };
                c as i64 * 16
            };
            let (x0, x1) = (chunk(f.x(west), false), chunk(f.x(east), true) - 1);
            let (z0, z1) = (chunk(f.z(north), false), chunk(f.z(south), true) - 1);
            let ring = vec![[x0, z0], [x1, z0], [x1, z1], [x0, z1]];
            rings.push((s.id.clone(), s.id.clone(), ring));
        }
        for s in self.project.shapes.iter().filter(|s| s.world == world) {
            for (i, ring) in s.polygon.iter().enumerate() {
                let pts = ring
                    .iter()
                    .map(|&[lat, lon]| [f.x(lon).round() as i64, f.z(lat).round() as i64]);
                rings.push((format!("{}-{}", s.id, i + 1), s.id.clone(), pts.collect()));
            }
        }
        Ok(Some((m, rings)))
    }

    /// WorldGuard regions for a world's selections (`meld-<id>`, with
    /// `[server.worldguard]`'s flags, owners and members), plus `__global__`
    /// when `global_flags` are set.
    pub fn regions_yml(&self, world: &str) -> Result<Option<String>> {
        let Some((m, rings)) = self.rings(world)? else {
            return Ok(None);
        };
        let (min_y, max_y) = m.y_range();
        let g = &self.conf.worldguard;
        let mut y = String::from("# Written by Meld: one region per selection, in the world's One World frame.\nregions:\n");
        if !g.global_flags.is_empty() {
            // WorldGuard drops a region missing priority, owners or members.
            let _ = writeln!(
                y,
                "  __global__:\n    type: global\n    priority: 0\n    flags: {}\n    owners: {{}}\n    members: {{}}",
                flags_yaml(&g.global_flags)
            );
        }
        for (id, sel, pts) in rings {
            let own = g.selection.get(&sel);
            let mut flags = g.flags.clone();
            flags.extend(own.map(|r| r.flags.clone()).unwrap_or_default());
            let pick = |mine: Option<&Vec<String>>, all: &Vec<String>| {
                let v = mine.filter(|v| !v.is_empty()).unwrap_or(all);
                if v.is_empty() {
                    "{}".to_string()
                } else {
                    format!("{{players: [{}]}}", v.join(", "))
                }
            };
            let _ = writeln!(
                y,
                "  meld-{}:\n    type: poly2d\n    min-y: {min_y}\n    max-y: {max_y}\n    priority: 0\n    flags: {}\n    owners: {}\n    members: {}\n    points:",
                id.to_lowercase(),
                flags_yaml(&flags),
                pick(own.map(|r| &r.owners), &g.owners),
                pick(own.map(|r| &r.members), &g.members),
            );
            for [x, z] in pts {
                let _ = writeln!(y, "    - {{x: {x}, z: {z}}}");
            }
        }
        Ok(Some(y))
    }

    /// `meld-border.sk`: SkBee dust walls along every region of the served
    /// worlds, drawn near players only. Meld 1's wall renderer (`border.py`),
    /// without its buffered rings and fling-back.
    pub fn skript(&self) -> Result<String> {
        let mut sets = String::new();
        let mut n = 0;
        for w in &self.conf.worlds {
            let Some((_, rings)) = self.rings(w)? else {
                continue;
            };
            let level = level_name(w);
            let mut count: BTreeMap<String, usize> = BTreeMap::new();
            for (_, _, ring) in rings {
                for (a, b) in ring.iter().zip(ring.iter().cycle().skip(1)) {
                    // At most 24 blocks a segment, so a player's 3 x 3 cells find it.
                    let len = (((b[0] - a[0]).pow(2) + (b[1] - a[1]).pow(2)) as f64).sqrt();
                    let steps = (len / 24.0).ceil().max(1.0) as i64;
                    let at = |t: i64, k: usize| {
                        a[k] as f64 + (b[k] - a[k]) as f64 * t as f64 / steps as f64
                    };
                    for i in 0..steps {
                        let (ax, az, bx, bz) = (at(i, 0), at(i, 1), at(i + 1, 0), at(i + 1, 1));
                        let key = format!(
                            "c{}_{}",
                            ((ax + bx) / 2.0 / 128.0).floor(),
                            ((az + bz) / 2.0 / 128.0).floor()
                        );
                        let c = count.entry(key.clone()).or_default();
                        *c += 1;
                        let _ = writeln!(
                            sets,
                            "    set {{meldwall::{level}::{key}::{c}}} to vector({ax:.0}, 0, {az:.0})"
                        );
                        let _ = writeln!(
                            sets,
                            "    set {{meldwallb::{level}::{key}::{c}}} to vector({bx:.0}, 0, {bz:.0})"
                        );
                        n += 1;
                    }
                }
            }
        }
        Ok(SKRIPT
            .replace("{SETS}", sets.trim_end())
            .replace("{N}", &n.to_string()))
    }

    /// Runs the server until it ends: `stop()` (or `meld2 server stop`)
    /// sends `stop` on its console and kills it after 60 s. Every console
    /// line goes to `on_line`. Returns its exit code.
    pub fn start(
        &self,
        java: Option<PathBuf>,
        stop: &dyn Fn() -> bool,
        on_line: &mut dyn FnMut(&str),
    ) -> Result<i32> {
        let dir = &self.dir;
        let mut meta: Meta = read_json(&dir.join(META)).with_context(|| {
            format!(
                "{} is not set up; run `meld2 server setup` first",
                dir.display()
            )
        })?;
        if meta.jar.is_empty() || !dir.join(&meta.jar).is_file() {
            bail!(
                "no server jar in {}; run `meld2 server setup` (with downloads)",
                dir.display()
            );
        }
        let eula = std::fs::read_to_string(dir.join("eula.txt")).unwrap_or_default();
        if !eula.lines().any(|l| l.trim() == "eula=true") {
            bail!("the Minecraft EULA is not accepted in {}; `meld2 server setup --accept-eula` once you agree to it", dir.display());
        }
        let lock = std::fs::File::create(dir.join(LOCK))?;
        if lock.try_lock().is_err() {
            bail!("the server in {} is already running", dir.display());
        }
        std::net::TcpListener::bind((self.conf.ip.as_str(), self.conf.port)).with_context(
            || {
                format!(
                    "port {}:{} is taken; stop what uses it or change [server] port",
                    self.conf.ip, self.conf.port
                )
            },
        )?;
        let java = find_java(java.or(self.conf.java.clone()), self.java_major())?;
        let _ = std::fs::remove_file(dir.join(STOP));
        on_line(&format!(
            "[meld] {} -jar {} with {} MB, in {}",
            java.display(),
            meta.jar,
            self.conf.ram_mb,
            dir.display()
        ));

        let mut cmd = std::process::Command::new(&java);
        let heap = format!("{}M", self.conf.ram_mb);
        cmd.args([format!("-Xms{heap}"), format!("-Xmx{heap}")])
            .args(JVM_FLAGS)
            .args(["-jar", &meta.jar, "--nogui"])
            .current_dir(dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::fs::File::create(dir.join("meld-server.err"))?);
        let mut child = cmd
            .spawn()
            .with_context(|| format!("starting {}", java.display()))?;
        let (mut stdin, stdout) = (
            child.stdin.take().expect("piped"),
            child.stdout.take().expect("piped"),
        );
        let process = Process::adopt(child)?;
        let mut status = Status {
            running: true,
            pid: Some(process.id()),
            state: "starting".into(),
            dir: dir.clone(),
            log: vec![],
        };
        let save = |s: &Status| {
            std::fs::write(dir.join(STATUS), serde_json::to_vec(s).unwrap_or_default())
        };
        save(&status)?;

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut deadline: Option<Instant> = None;
        loop {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(line) => {
                    on_line(&line);
                    // The vanilla ready line: `Done (12.3s)! For help, type "help"`.
                    if status.state == "starting" && line.contains("Done (") {
                        status.state = "ready".into();
                        save(&status)?;
                        for c in std::mem::take(&mut meta.first_start) {
                            on_line(&format!("[meld] > {c}"));
                            let _ = writeln!(stdin, "{c}");
                        }
                        std::fs::write(dir.join(META), serde_json::to_vec_pretty(&meta)?)?;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if deadline.is_none() {
                for c in take_inbox(&dir.join(INBOX)) {
                    on_line(&format!("[meld] > {c}"));
                    let _ = writeln!(stdin, "{c}").and_then(|_| stdin.flush());
                }
            }
            if deadline.is_none() && (stop() || dir.join(STOP).exists()) {
                on_line("[meld] > stop");
                let _ = writeln!(stdin, "stop").and_then(|_| stdin.flush());
                deadline = Some(Instant::now() + Duration::from_secs(60));
                status.state = "stopping".into();
                save(&status)?;
            }
            if deadline.is_some_and(|d| Instant::now() > d) {
                on_line("[meld] the server did not stop in 60 s; killing it");
                process.kill();
            }
        }
        let code = process.wait()?.code().unwrap_or(-1);
        let _ = std::fs::remove_file(dir.join(STOP));
        let _ = std::fs::remove_file(dir.join(STATUS));
        drop(lock);
        Ok(code)
    }

    /// Asks a running server (started here or by another Meld) to stop.
    pub fn request_stop(&self) -> Result<()> {
        if !self.status(0).running {
            bail!("the server in {} is not running", self.dir.display());
        }
        std::fs::write(self.dir.join(STOP), b"")?;
        Ok(())
    }

    /// Queues one console command for the running server (started here or
    /// by another Meld), which sends it within half a second.
    pub fn send(&self, command: &str) -> Result<()> {
        let c = command.trim();
        if c.is_empty() || c.len() > 1000 || c.chars().any(char::is_control) {
            bail!("a console command is one line of up to 1000 characters");
        }
        if !self.status(0).running {
            bail!("the server in {} is not running", self.dir.display());
        }
        let inbox = self.dir.join(INBOX);
        std::fs::create_dir_all(&inbox)?;
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = inbox.join(format!("{t:032}.tmp"));
        std::fs::write(&tmp, c)?;
        std::fs::rename(&tmp, tmp.with_extension("cmd"))?;
        Ok(())
    }

    /// Whether it runs (its lock is held), and the last `lines` of its log.
    pub fn status(&self, lines: usize) -> Status {
        let running = std::fs::File::open(self.dir.join(LOCK))
            .is_ok_and(|f| matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
        let mut s: Status = if running {
            read_json(&self.dir.join(STATUS)).unwrap_or_default()
        } else {
            Status::default()
        };
        s.running = running;
        s.dir = self.dir.clone();
        let log = std::fs::read_to_string(self.dir.join("logs/latest.log")).unwrap_or_default();
        let all: Vec<&str> = log.lines().collect();
        s.log = all[all.len().saturating_sub(lines)..]
            .iter()
            .map(|l| l.to_string())
            .collect();
        s
    }
}

/// The queued console commands, oldest first, removed as they are read.
fn take_inbox(inbox: &Path) -> Vec<String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(inbox)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "cmd"))
        .collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|f| {
            let c = std::fs::read_to_string(&f).ok();
            let _ = std::fs::remove_file(&f);
            c
        })
        .collect()
}

fn read_json<T: for<'de> Deserialize<'de>>(file: &Path) -> Result<T> {
    let bytes = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("reading {}", file.display()))
}

/// Writes `text` unless the file exists and `force` is off.
fn write_new(file: &Path, text: &str, force: bool, say: &mut dyn FnMut(String)) -> Result<()> {
    if file.exists() && !force {
        say(format!(
            "kept {} (exists; --force writes it again)",
            file.display()
        ));
        return Ok(());
    }
    std::fs::create_dir_all(file.parent().unwrap_or(Path::new(".")))?;
    std::fs::write(file, text)?;
    say(format!("wrote {}", file.display()));
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    if from.is_file() {
        std::fs::create_dir_all(to.parent().unwrap_or(Path::new(".")))?;
        std::fs::copy(from, to)?;
        return Ok(());
    }
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        if e.file_name() != "session.lock" {
            copy_tree(&e.path(), &to.join(e.file_name()))?;
        }
    }
    Ok(())
}

/// Bytes of every file under `dir`.
pub fn dir_bytes(dir: &Path) -> Result<u64> {
    let mut n = 0;
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        n += if e.file_type()?.is_dir() {
            dir_bytes(&e.path())?
        } else {
            e.metadata()?.len()
        };
    }
    Ok(n)
}

#[cfg(windows)]
fn link(src: &Path, dest: &Path) -> Result<()> {
    // A junction: no admin rights or developer mode needed, unlike a symlink.
    Ok(junction::create(src, dest)?)
}

#[cfg(unix)]
fn link(src: &Path, dest: &Path) -> Result<()> {
    Ok(std::os::unix::fs::symlink(src, dest)?)
}

/// Removes a link, never what it points at.
fn remove_link(p: &Path) -> Result<()> {
    // A Windows junction or directory symlink goes with remove_dir, a Unix symlink with remove_file.
    std::fs::remove_dir(p)
        .or_else(|_| std::fs::remove_file(p))
        .with_context(|| format!("removing the link {}", p.display()))
}

pub use crate::install::Sum;

/// A GET's JSON body.
fn json(req: ureq::RequestBuilder<ureq::typestate::WithoutBody>) -> Result<serde_json::Value> {
    let text = req
        .header("User-Agent", "Meld2 (server setup)")
        .call()?
        .into_body()
        .read_to_string()?;
    Ok(serde_json::from_str(&text)?)
}

struct Download {
    name: String,
    url: String,
    sha256: String,
}

/// The newest build of a Leaf or Paper version.
fn jar(flavor: Flavor, version: &str) -> Result<Download> {
    let d = newest_jar(flavor, version)?;
    if !d.name.ends_with(".jar") || d.name.contains(['/', '\\']) || d.name.starts_with('.') {
        bail!("the {flavor:?} API named an unexpected file {:?}", d.name);
    }
    Ok(d)
}

fn newest_jar(flavor: Flavor, version: &str) -> Result<Download> {
    let get = |url: &str| json(ureq::get(url)).with_context(|| format!("asking {url}"));
    let missing = || format!("no {flavor:?} build for {version}");
    match flavor {
        Flavor::Leaf => {
            let builds = get(&format!("{LEAF_API}/versions/{version}/builds"))?;
            let b = builds["builds"]
                .as_array()
                .and_then(|b| b.last())
                .with_context(missing)?;
            let dl = &b["downloads"]["primary"];
            let name = dl["name"].as_str().with_context(missing)?.to_string();
            Ok(Download {
                url: format!(
                    "{LEAF_API}/versions/{version}/builds/{}/downloads/{name}",
                    b["build"]
                ),
                sha256: dl["sha256"].as_str().with_context(missing)?.into(),
                name,
            })
        }
        Flavor::Paper => {
            let b = get(&format!("{PAPER_API}/versions/{version}/builds/latest"))?;
            let dl = &b["downloads"]["server:default"];
            Ok(Download {
                name: dl["name"].as_str().with_context(missing)?.into(),
                url: dl["url"].as_str().with_context(missing)?.into(),
                sha256: dl["checksums"]["sha256"]
                    .as_str()
                    .with_context(missing)?
                    .into(),
            })
        }
    }
}

struct Plugin {
    file: String,
    url: String,
    sha512: String,
}

/// The newest Modrinth build of `slug` for `version` (a stable one first),
/// else for its family (`1.21`): Meld 1's `resolve_plugin`. `slug@x` is build x.
fn plugin(spec: &str, version: &str) -> Result<Plugin> {
    let (slug, pin) = spec
        .split_once('@')
        .map_or((spec, None), |(s, v)| (s, Some(v)));
    let loaders = r#"["paper","bukkit","spigot","folia"]"#;
    let list = |game: Option<&str>| -> Result<Vec<serde_json::Value>> {
        let url = format!("{MODRINTH_API}/project/{slug}/version");
        let mut req = ureq::get(&url).query("loaders", loaders);
        if let Some(g) = game {
            req = req.query("game_versions", format!(r#"["{g}"]"#));
        }
        let v = json(req).with_context(|| format!("asking Modrinth for {slug}"))?;
        Ok(v.as_array().cloned().unwrap_or_default())
    };
    let family: String = version.split('.').take(2).collect::<Vec<_>>().join(".");
    let pick = |vs: Vec<serde_json::Value>| {
        vs.iter()
            .find(|v| v["version_type"] == "release")
            .or(vs.first())
            .cloned()
    };
    let v = match pin {
        Some(pin) => list(None)?
            .into_iter()
            .find(|v| v["version_number"] == pin)
            .with_context(|| format!("{slug}: no build {pin} on Modrinth"))?,
        None => match pick(list(Some(version))?) {
            Some(v) => v,
            None => pick(
                list(None)?
                    .into_iter()
                    .filter(|v| {
                        v["game_versions"].as_array().is_some_and(|g| {
                            g.iter()
                                .filter_map(|g| g.as_str())
                                .any(|g| g == family || g.starts_with(&format!("{family}.")))
                        })
                    })
                    .collect(),
            )
            .with_context(|| format!("{slug}: no build for Minecraft {version} on Modrinth"))?,
        },
    };
    let files = v["files"].as_array().context("no files")?;
    let f = files
        .iter()
        .find(|f| f["primary"] == true)
        .or(files.first())
        .context("no files")?;
    let file = f["filename"].as_str().context("no file name")?;
    if !file.ends_with(".jar") || file.contains(['/', '\\']) {
        bail!("{slug}: unexpected file {file:?}");
    }
    Ok(Plugin {
        file: file.into(),
        url: f["url"].as_str().context("no url")?.into(),
        sha512: f["hashes"]["sha512"].as_str().context("no sha512")?.into(),
    })
}

/// Java's major version from `java -version` output (`"21.0.9"`, `"1.8.0_481"`).
pub fn java_major(text: &str) -> Option<u32> {
    let v = text.split("version \"").nth(1)?.split('"').next()?;
    let mut parts = v.split(['.', '_', '-', '+']);
    match parts.next()?.parse().ok()? {
        1 => parts.next()?.parse().ok(),
        n => Some(n),
    }
}

/// The Javas Meld finds by itself: `JAVA_HOME`'s, the Modrinth app's
/// runtimes, and `java` on PATH.
pub fn java_candidates() -> Vec<PathBuf> {
    let exe = if cfg!(windows) { "java.exe" } else { "java" };
    let mut candidates: Vec<PathBuf> = vec![];
    if let Some(home) = std::env::var_os("JAVA_HOME") {
        candidates.push(PathBuf::from(home).join("bin").join(exe));
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let meta = PathBuf::from(appdata).join("ModrinthApp/meta/java_versions");
        for e in std::fs::read_dir(meta).into_iter().flatten().flatten() {
            candidates.push(e.path().join("bin").join(exe));
        }
    }
    candidates.push(PathBuf::from("java"));
    candidates
}

/// The Java to run the server: the one given (at least `need`), else the
/// newest of `JAVA_HOME`, the Modrinth app's runtimes and `java` on PATH,
/// since plugins move to new Java before servers do (WorldEdit 7.4.5 needs 25).
pub fn find_java(given: Option<PathBuf>, need: u32) -> Result<PathBuf> {
    let major = |c: &Path| {
        let o = std::process::Command::new(c)
            .arg("-version")
            .output()
            .ok()?;
        java_major(&(String::from_utf8_lossy(&o.stderr) + String::from_utf8_lossy(&o.stdout)))
    };
    if let Some(c) = given {
        return match major(&c) {
            Some(m) if m >= need => Ok(c),
            m => bail!(
                "{} is Java {m:?}; this server needs {need} or newer",
                c.display()
            ),
        };
    }
    let found: Vec<(u32, PathBuf)> = java_candidates()
        .into_iter()
        .filter_map(|c| Some((major(&c)?, c)))
        .collect();
    let newest = found.iter().max_by_key(|(m, _)| *m);
    match newest {
        Some((m, c)) if *m >= need => Ok(c.clone()),
        _ => bail!(
            "no Java {need} or newer found ({:?}); install one, or set [server] java or --java",
            found
        ),
    }
}

/// A flag value as YAML: strings quoted, numbers and booleans plain.
fn yaml_scalar(v: &toml::Value) -> Option<String> {
    match v {
        toml::Value::String(s) => serde_json::to_string(s).ok(),
        toml::Value::Integer(i) => Some(i.to_string()),
        toml::Value::Float(f) => Some(f.to_string()),
        toml::Value::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

fn flags_yaml(flags: &BTreeMap<String, toml::Value>) -> String {
    let body: Vec<String> = flags
        .iter()
        .filter_map(|(k, v)| Some(format!("{k}: {}", yaml_scalar(v)?)))
        .collect();
    format!("{{{}}}", body.join(", "))
}

/// The wall script; `{SETS}` are the segments, bucketed in 128-block cells.
/// The draw loop is Meld 1's (`border.py` `_wall_draw_block`), whose loop
/// numbering was fixed in game there.
const SKRIPT: &str = r#"# meld-border.sk - written by Meld: particle walls along the Meld regions.
# Needs Skript and SkBee (dust). The WorldGuard regions do the protecting; this only draws.
# {N} wall segments. Reload with /sk reload meld-border

options:
    radius: 96
    wall-h: 6
    ticks: 10

on load:
    delete {meldwall::*}
    delete {meldwallb::*}
{SETS}

every {@ticks} ticks:
    loop all players:
        set {_px} to x-coordinate of loop-player
        set {_py} to y-coordinate of loop-player
        set {_pz} to z-coordinate of loop-player
        set {_w} to world of loop-player
        set {_wn} to "%world of loop-player%"
        set {_cx} to floor({_px} / 128)
        set {_cz} to floor({_pz} / 128)
        delete {_k::*}
        set {_k::1} to "c%{_cx} - 1%_%{_cz} - 1%"
        set {_k::2} to "c%{_cx}%_%{_cz} - 1%"
        set {_k::3} to "c%{_cx} + 1%_%{_cz} - 1%"
        set {_k::4} to "c%{_cx} - 1%_%{_cz}%"
        set {_k::5} to "c%{_cx}%_%{_cz}%"
        set {_k::6} to "c%{_cx} + 1%_%{_cz}%"
        set {_k::7} to "c%{_cx} - 1%_%{_cz} + 1%"
        set {_k::8} to "c%{_cx}%_%{_cz} + 1%"
        set {_k::9} to "c%{_cx} + 1%_%{_cz} + 1%"
        loop {_k::*}:
            loop {meldwall::%{_wn}%::%loop-value-2%::*}:
                set {_a} to loop-value-3
                set {_dx} to (x of {_a}) - {_px}
                set {_dz} to (z of {_a}) - {_pz}
                if ({_dx} * {_dx}) + ({_dz} * {_dz}) <= {@radius} * {@radius}:
                    set {_b} to {meldwallb::%{_wn}%::%loop-value-2%::%loop-index-2%}
                    set {_ax} to x of {_a}
                    set {_az} to z of {_a}
                    set {_bx} to x of {_b}
                    set {_bz} to z of {_b}
                    set {_n} to ceil(sqrt(({_bx} - {_ax}) ^ 2 + ({_bz} - {_az}) ^ 2) / 4)
                    if {_n} < 1:
                        set {_n} to 1
                    loop integers from 0 to {_n}:
                        set {_t} to loop-value-4 / {_n}
                        set {_x} to {_ax} + ({_bx} - {_ax}) * {_t}
                        set {_z} to {_az} + ({_bz} - {_az}) * {_t}
                        loop integers from -{@wall-h} to {@wall-h}:
                            make 1 of dust using dustOption(aqua, 2.2) at location({_x}, {_py} + loop-value-5, {_z}, {_w})

command /meldborder:
    trigger:
        send "Meld walls: {N} segments, drawn within {@radius} blocks every {@ticks} ticks"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_versions_and_names() {
        assert_eq!(
            java_major("java version \"21.0.9\" 2025-10-21 LTS"),
            Some(21)
        );
        assert_eq!(java_major("openjdk version \"25\" 2025-09-16"), Some(25));
        assert_eq!(java_major("java version \"1.8.0_481\""), Some(8));
        assert_eq!(java_major("no java here"), None);
        assert_eq!(level_name("Alps World"), "Alps_World");
        assert_eq!(level_name("../x"), "x");
        assert_eq!(level_name("??"), "world");
    }

    /// The WorldGuard ring of a bbox selection lands on the area Arnis
    /// recorded for it, so on its `--world-border` (the Phase 1 e2e Vaduz
    /// world: frame 47.14, 9.5215 at scale 1, area -128..127 x -112..111).
    #[test]
    fn worldguard_ring_matches_the_built_area() {
        let dir = std::env::temp_dir().join(format!("meld2-wg-{}", std::process::id()));
        let world = dir.join("saves/Vaduz");
        std::fs::create_dir_all(&world).unwrap();
        std::fs::write(
            world.join("arnis_one_world.json"),
            r#"{"origin_lat":47.14,"origin_lon":9.5215,"scale":1.0,"disable_height_limit":true,
                "areas":[{"min_x":-128,"min_z":-112,"max_x":127,"max_z":111}]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("p.toml"),
            "format = 1\nname = \"V\"\noutput = \"saves\"\n[server]\nskript_walls = true\n[server.worldguard]\nflags = { greeting = \"Hi\" }\nowners = [\"Teddy563\"]\nglobal_flags = { block-break = \"deny\" }\n[server.worldguard.selection.vaduz]\nflags = { pvp = \"deny\" }\nmembers = [\"Alex_1\"]\n[[selection]]\nid = \"vaduz\"\nbbox = [47.1390, 9.5200, 47.1410, 9.5230]\nworld = \"Vaduz\"\n",
        )
        .unwrap();
        let text = std::fs::read_to_string(dir.join("p.toml")).unwrap();
        for (from, to, want) in [
            ("Alex_1", "a b", "not a player name"),
            ("selection.vaduz", "selection.nope", "no selection"),
        ] {
            let err = format!("{:#}", Project::parse(&text.replace(from, to)).unwrap_err());
            assert!(err.contains(want), "{err}");
        }
        let p = Project::load(&dir.join("p.toml")).unwrap();
        let s = Server::of(&p).unwrap();
        assert_eq!(s.conf.worlds, ["Vaduz"]);
        let yml = s.regions_yml("Vaduz").unwrap().unwrap();
        for want in [
            "meld-vaduz:",
            "type: poly2d",
            "min-y: -2032",
            "- {x: -128, z: -112}",
            "- {x: 127, z: 111}",
            "__global__:\n    type: global",
            "flags: {block-break: \"deny\"}",
            "flags: {greeting: \"Hi\", pvp: \"deny\"}",
            "owners: {players: [Teddy563]}",
            "members: {players: [Alex_1]}",
        ] {
            assert!(yml.contains(want), "{want} not in\n{yml}");
        }
        assert!(!s.status(5).running && s.request_stop().is_err());
        assert!(s
            .send("say hi")
            .unwrap_err()
            .to_string()
            .contains("not running"));
        assert!(s
            .send(
                "a
b"
            )
            .is_err());

        // Setup (no downloads): a foreign folder needs --force, a non-world in
        // a world's place is never removed, and the EULA stays unaccepted.
        std::fs::write(world.join("level.dat"), b"").unwrap();
        let srv = dir.join("server");
        std::fs::create_dir_all(srv.join("Vaduz")).unwrap();
        std::fs::write(srv.join("Vaduz/notes.txt"), b"mine").unwrap();
        let mut said = vec![];
        let err = s
            .setup(false, false, false, &mut |l| said.push(l))
            .unwrap_err();
        assert!(err.to_string().contains("did not set it up"), "{err}");
        let err = s
            .setup(true, false, false, &mut |l| said.push(l))
            .unwrap_err();
        assert!(err.to_string().contains("not a world"), "{err}");
        assert!(srv.join("Vaduz/notes.txt").is_file());
        std::fs::remove_dir_all(srv.join("Vaduz")).unwrap();
        s.setup(true, false, false, &mut |l| said.push(l)).unwrap();
        // Now it is Meld's folder: a second setup needs no --force.
        s.setup(false, false, false, &mut |l| said.push(l)).unwrap();
        let props = std::fs::read_to_string(srv.join("server.properties")).unwrap();
        assert!(
            props.contains(
                "level-name=Vaduz
"
            ) && props.contains(
                "online-mode=false
"
            )
        );
        assert!(std::fs::read_to_string(srv.join("eula.txt"))
            .unwrap()
            .contains("eula=false"));
        assert!(srv.join("Vaduz/arnis_one_world.json").is_file(), "linked");
        assert!(srv
            .join("plugins/WorldGuard/worlds/Vaduz/regions.yml")
            .is_file());
        // The walls: the 256 x 224 ring in segments of at most 24 blocks.
        let sk =
            std::fs::read_to_string(srv.join("plugins/Skript/scripts/meld-border.sk")).unwrap();
        assert!(
            sk.contains("set {meldwall::Vaduz::c-1_-1::1} to vector(-128, 0, -112)"),
            "{sk}"
        );
        assert!(sk.contains("# 42 wall segments"), "{}", &sk[..300]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn console_inbox_in_order_once() {
        let d = std::env::temp_dir().join(format!("meld2-inbox-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for (n, c) in [(2, "say b"), (1, "say a")] {
            std::fs::write(d.join(format!("{n:032}.cmd")), c).unwrap();
        }
        std::fs::write(d.join("3.tmp"), "half written").unwrap();
        assert_eq!(take_inbox(&d), ["say a", "say b"]);
        assert!(take_inbox(&d).is_empty());
        std::fs::remove_dir_all(d).unwrap();
    }
}
