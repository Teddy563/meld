//! `meld2`: build a project of selections through Arnis at Scale, headless.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use meld_core::arnis::Arnis;
use meld_core::convert;
use meld_core::export;
use meld_core::import;
use meld_core::install;
use meld_core::progress::Event;
use meld_core::project::Project;
use meld_core::queue::Note;
use meld_core::server::Server;
use meld_core::state::{self, State};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use meld2::{resolve, run_project, serve, show_plan};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Build every selection of a project that is not built yet, resuming partial ones.
    Run {
        project: PathBuf,
        /// Arnis executable; see `meld2 arnis status` for the lookup order.
        #[arg(long)]
        arnis: Option<PathBuf>,
        /// Build these selections again from scratch (`--rebuild=a,b`; a polygon's
        /// id takes all its parts), or every selection (`--rebuild`).
        #[arg(long, num_args = 0..=1, require_equals = true, value_delimiter = ',')]
        rebuild: Option<Vec<String>>,
    },
    /// Show each selection's pieces, regions and estimated size against free disk.
    Plan {
        project: PathBuf,
        #[arg(long)]
        arnis: Option<PathBuf>,
    },
    /// Show the saved state of one project, or list every project Meld knows.
    Status { project: Option<PathBuf> },
    /// Ask a running `meld2 run` of a project to stop; it resumes on the next run.
    Stop { project: PathBuf },
    /// Print an Arnis executable's version and capabilities.
    Caps {
        #[arg(long)]
        arnis: Option<PathBuf>,
    },
    /// Serve the JSON API and a status page; every request needs the printed token.
    Serve {
        /// Address to listen on. Off loopback (e.g. 0.0.0.0:7878), anyone on the
        /// network who has the token can drive Meld, over plain HTTP.
        #[arg(long, default_value = "127.0.0.1:7878")]
        bind: String,
        /// Folder of `<name>/project.toml` the API serves [default: <data>/workspace].
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        arnis: Option<PathBuf>,
    },
    /// Convert Meld 1 projects (a project folder, its `projects/` folder or
    /// the data folder) and presets to Meld 2 project files. Each starts a new world.
    Import {
        /// A Meld 1 project folder (with project.json), a folder of them, or a preset .json.
        path: PathBuf,
        /// Where to write `<slug>/project.toml` and `presets/<slug>.toml`.
        #[arg(long, default_value = ".")]
        out: PathBuf,
    },
    /// Convert a built world to B_Linear (Leaf 1.21.11+) in a `<World> [BLinear]`
    /// sibling, read a sample back and swap the regions in only when all is done.
    Convert {
        /// The world folder (with level.dat and region/).
        world: PathBuf,
        /// Where to write [default: `<world> [BLinear]` beside it].
        #[arg(long)]
        out: Option<PathBuf>,
        /// Replace the region files of an existing folder Meld did not write,
        /// or one changed since.
        #[arg(long)]
        force: bool,
        /// Regions converted at once [default: the cores].
        #[arg(long)]
        threads: Option<usize>,
    },
    /// Zip a world (a backup, or to hand it on), after checking the disk.
    Export {
        /// The world folder (with level.dat).
        world: PathBuf,
        /// The zip to write [default: `<world>-<unix time>.zip` beside it].
        #[arg(long)]
        out: Option<PathBuf>,
        /// Disk to keep free after the zip, in MB.
        #[arg(long, default_value_t = 1024)]
        min_free_mb: u64,
    },
    /// Which Arnis Meld uses, and installing the pinned release.
    #[command(subcommand)]
    Arnis(ArnisCmd),
    /// A Leaf or Paper server for the project's worlds, from its `[server]` table.
    #[command(subcommand)]
    Server(ServerCmd),
}

#[derive(Subcommand)]
enum ServerCmd {
    /// Create or refresh the server folder: worlds (linked or copied), config,
    /// WorldGuard regions, datapacks, and the verified server jar and plugins.
    Setup {
        project: PathBuf,
        /// Set up over a folder Meld did not create, and replace copied worlds,
        /// server.properties, regions.yml and datapacks that exist.
        #[arg(long)]
        force: bool,
        /// You agree to the Minecraft EULA (https://aka.ms/MinecraftEULA).
        #[arg(long)]
        accept_eula: bool,
        /// Write the folder only; download no jar or plugin.
        #[arg(long)]
        no_download: bool,
    },
    /// Run the server until it stops, printing its console. `meld2 server stop`
    /// (from another shell, or the API) saves and stops it.
    Start {
        project: PathBuf,
        /// Java to run it with [default: JAVA_HOME, the Modrinth app's, `java`].
        #[arg(long)]
        java: Option<PathBuf>,
    },
    /// Send one console command to the running server (started here or by the API).
    Send {
        project: PathBuf,
        /// The command, e.g. `say hello` or `save-all flush`.
        #[arg(required = true, num_args = 1..)]
        command: Vec<String>,
    },
    /// Ask the running server to save and stop, and wait until it has.
    Stop { project: PathBuf },
    /// Whether the server runs, and the end of its log.
    Status {
        project: PathBuf,
        #[arg(long, default_value_t = 20)]
        lines: usize,
    },
}

#[derive(Subcommand)]
enum ArnisCmd {
    /// Which Arnis would be used and why, with its version and capabilities.
    Status {
        #[arg(long)]
        arnis: Option<PathBuf>,
        /// A project file whose `arnis` setting counts.
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Download and verify the pinned Arnis release into the data dir.
    Install {
        #[arg(long, default_value = install::VERSION)]
        version: String,
    },
    /// Print the path of the Arnis that would be used (no download).
    Path {
        #[arg(long)]
        arnis: Option<PathBuf>,
        #[arg(long)]
        project: Option<PathBuf>,
    },
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Run {
            project,
            arnis,
            rebuild,
        } => {
            let mut tenths = HashMap::new();
            let summary = run_project(
                &project,
                arnis,
                rebuild.as_deref(),
                &mut |line| println!("{line}"),
                &mut |id, note| show(id, note, &mut tenths),
            )?;
            if summary.failed > 0 {
                bail!("{} step(s) failed", summary.failed);
            }
            Ok(())
        }
        Cmd::Plan { project, arnis } => {
            let project = Project::load(&project)?;
            let (arnis, _, _) = resolve(arnis, Some(&project))?;
            show_plan(&project, &arnis, &mut |line| println!("{line}")).map(drop)
        }
        Cmd::Status { project } => status(project.as_deref()),
        Cmd::Stop { project } => {
            let p = Project::load(&project)?;
            let dir = state::project_dir(&p);
            std::fs::create_dir_all(&dir)?;
            std::fs::write(dir.join("stop"), b"")?;
            println!("asked {} to stop", p.name);
            Ok(())
        }
        Cmd::Caps { arnis } => {
            let (_, _, probe) = resolve(arnis, None)?;
            println!("arnis {}", probe.version);
            println!("{}", probe.caps.join(" "));
            Ok(())
        }
        Cmd::Arnis(cmd) => arnis_cmd(cmd),
        Cmd::Server(cmd) => server_cmd(cmd),
        Cmd::Convert {
            world,
            out,
            force,
            threads,
        } => {
            let dest = out.unwrap_or_else(|| convert::sibling(&world));
            let threads = threads
                .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()));
            let mut tenth = usize::MAX;
            let c = convert::convert(&world, &dest, threads, force, &|| false, &mut |d, of| {
                if of > 0 && d * 10 / of != tenth {
                    tenth = d * 10 / of;
                    println!("converted {d}/{of} region file(s)");
                }
            })?;
            println!(
                "{}: {} region file(s), {} chunks, {:.1} MB -> {:.1} MB; {} read back chunk for chunk",
                dest.display(),
                c.regions,
                c.chunks,
                c.mca_bytes as f64 / 1e6,
                c.blinear_bytes as f64 / 1e6,
                c.verified
            );
            Ok(())
        }
        Cmd::Export {
            world,
            out,
            min_free_mb,
        } => {
            let to = out.unwrap_or_else(|| {
                export::name_in(world.parent().unwrap_or(Path::new(".")), &world)
            });
            let e = export::export(&world, &to, min_free_mb)?;
            println!(
                "{}: {} file(s), {:.1} MB -> {:.1} MB",
                to.display(),
                e.files,
                e.bytes as f64 / 1e6,
                e.zip_bytes as f64 / 1e6
            );
            Ok(())
        }
        Cmd::Serve { bind, dir, arnis } => {
            let token = match std::env::var("MELD2_TOKEN") {
                Ok(t) if t.len() >= 16 => t,
                Ok(t) if !t.is_empty() => bail!("MELD2_TOKEN must be at least 16 characters"),
                _ => serve::new_token()?,
            };
            let workspace = dir.unwrap_or_else(|| state::data_dir().join("workspace"));
            std::fs::create_dir_all(&workspace)?;
            let server = tiny_http::Server::http(&bind)
                .map_err(|e| anyhow::anyhow!("listening on {bind}: {e}"))?;
            let addr = server.server_addr().to_ip().context("not an IP address")?;
            println!("meld2 serve on http://{addr}/?token={token}");
            println!("workspace: {}", workspace.display());
            println!("token: {token} (header X-Meld-Token or Authorization: Bearer, or ?token=)");
            if !addr.ip().is_loopback() {
                println!("note: reachable from the network; every request needs the token, and traffic is plain HTTP (use a trusted LAN, an SSH tunnel or a TLS proxy)");
            }
            serve::serve(server, serve::Ctx::new(workspace, token, arnis));
            Ok(())
        }
        Cmd::Import { path, out } => {
            if import_any(&path, &out, 0)? == 0 {
                bail!(
                    "no Meld 1 project.json or preset found in {}",
                    path.display()
                );
            }
            Ok(())
        }
    }
}

/// Imports what `path` holds, looking two folders down (data dir, then
/// `projects/`, then a project); returns how many it wrote.
fn import_any(path: &Path, out: &Path, depth: u32) -> Result<usize> {
    let read = |p: &Path| -> Result<serde_json::Value> {
        let text =
            std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
        serde_json::from_str(&text).with_context(|| format!("in {}", p.display()))
    };
    let slug = |p: &Path| {
        p.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "imported".into())
    };
    if path.is_file() {
        let json = read(path)?;
        if json.get("meld_preset").is_some() {
            let imported = import::preset(&json)?;
            let to = out.join("presets").join(format!("{}.toml", slug(path)));
            return save_import(path, &to, imported).map(|_| 1);
        }
        let imported = import::project(&json, None)?;
        return save_import(path, &out.join(slug(path)).join("project.toml"), imported).map(|_| 1);
    }
    let pj = path.join("project.json");
    if pj.is_file() {
        let name = slug(path);
        // The gallery folder, from the projects folder's _org.json.
        let org = path
            .parent()
            .map(|d| d.join("_org.json"))
            .filter(|o| o.is_file())
            .and_then(|o| read(&o).ok());
        let folder = org.as_ref().and_then(|o| o["assign"][&name].as_str());
        let mut imported = import::project(&read(&pj)?, folder)?;
        if let Ok(grid) = read(&path.join("grid.json")) {
            let cells = grid.as_object().map_or(0, |g| g.len());
            imported.notes.push(format!(
                "grid.json: {cells} cell(s) of the Meld 1 world are not carried; Meld 2 builds by pieces"
            ));
        }
        save_import(&pj, &out.join(&name).join("project.toml"), imported)?;
        return Ok(1);
    }
    let mut n = 0;
    if depth < 2 {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
            .with_context(|| format!("reading {}", path.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        entries.sort();
        for e in entries {
            let preset = e.extension().is_some_and(|x| x == "json")
                && read(&e).is_ok_and(|j| j.get("meld_preset").is_some());
            if e.is_dir() || preset {
                n += import_any(&e, out, depth + 1)?;
            }
        }
    }
    Ok(n)
}

/// Writes one import, never over an existing file, and says what became of each key.
fn save_import(from: &Path, to: &Path, imported: import::Imported) -> Result<()> {
    if to.exists() {
        bail!("{} exists; move it or pick another --out", to.display());
    }
    std::fs::create_dir_all(to.parent().unwrap_or(Path::new(".")))?;
    std::fs::write(to, &imported.toml)?;
    println!("{} -> {}", from.display(), to.display());
    let list = |what: &str, keys: &[String]| {
        if !keys.is_empty() {
            println!("  {what} ({}): {}", keys.len(), keys.join(", "));
        }
    };
    list("mapped", &imported.mapped);
    list("dropped on purpose", &imported.dropped);
    list("NOT MAPPED", &imported.unmapped);
    for note in &imported.notes {
        println!("  note: {note}");
    }
    Ok(())
}

fn server_cmd(cmd: ServerCmd) -> Result<()> {
    let (ServerCmd::Setup { project, .. }
    | ServerCmd::Start { project, .. }
    | ServerCmd::Stop { project }
    | ServerCmd::Send { project, .. }
    | ServerCmd::Status { project, .. }) = &cmd;
    let p = Project::load(project)?;
    let srv = Server::of(&p)?;
    match cmd {
        ServerCmd::Setup {
            force,
            accept_eula,
            no_download,
            ..
        } => srv.setup(force, accept_eula, !no_download, &mut |l| println!("{l}")),
        ServerCmd::Start { java, .. } => {
            let code = srv.start(java, &|| false, &mut |l| println!("{l}"))?;
            println!("server exited ({code})");
            Ok(())
        }
        ServerCmd::Send { command, .. } => {
            srv.send(&command.join(" "))?;
            println!("sent; the console shows the reply (`meld2 server status`)");
            Ok(())
        }
        ServerCmd::Stop { .. } => {
            srv.request_stop()?;
            println!("asked the server to stop; waiting");
            let t = std::time::Instant::now();
            while srv.status(0).running {
                if t.elapsed() > std::time::Duration::from_secs(90) {
                    bail!("still running after 90 s");
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            println!("stopped");
            Ok(())
        }
        ServerCmd::Status { lines, .. } => {
            let s = srv.status(lines);
            if s.running {
                let pid = s.pid.map_or("?".into(), |p| p.to_string());
                println!("running ({}), pid {pid}, {}", s.state, s.dir.display());
            } else {
                println!("not running, {}", s.dir.display());
            }
            for l in s.log {
                println!("  {l}");
            }
            Ok(())
        }
    }
}

fn arnis_cmd(cmd: ArnisCmd) -> Result<()> {
    let data = state::data_dir();
    match cmd {
        ArnisCmd::Status { arnis, project } => {
            let project = project.map(|p| Project::load(&p)).transpose()?;
            let pin = format!("{} v{}", install::REPO, install::VERSION);
            let Some(found) =
                install::find_here(arnis, project.as_ref().and_then(Project::arnis_path), &data)
            else {
                println!(
                    "no Arnis found; `meld2 run` will download {pin} to {}",
                    install::cached(&data).display()
                );
                println!(
                    "lookup order: --arnis, project `arnis`, MELD2_ARNIS, {} next to meld2, {}",
                    install::EXE,
                    install::cached(&data).display()
                );
                return Ok(());
            };
            println!("arnis:   {}", found.path.display());
            println!("source:  {}", found.source);
            println!("pinned:  {pin} (needs {} or newer)", install::MIN_VERSION);
            let probe = install::probe(&Arnis::new(&found.path))?;
            println!("version: {} (ok)", probe.version);
            println!("caps:    {}", probe.caps.join(" "));
            Ok(())
        }
        ArnisCmd::Install { version } => {
            let exe = install::install(&data, &version)?;
            let probe = install::probe(&Arnis::new(&exe))?;
            println!("{} (arnis {})", exe.display(), probe.version);
            Ok(())
        }
        ArnisCmd::Path { arnis, project } => {
            let project = project.map(|p| Project::load(&p)).transpose()?;
            let found =
                install::find_here(arnis, project.as_ref().and_then(Project::arnis_path), &data)
                    .context("no Arnis found; run `meld2 arnis install`")?;
            println!("{}", found.path.display());
            Ok(())
        }
    }
}

/// One line per thing worth seeing; percentages every 10 %.
fn show(id: &str, note: &Note, tenths: &mut HashMap<String, i64>) {
    match note {
        Note::Skipped(why) => println!("[{id}] skipped: {why}"),
        Note::Refused(why) => println!("[{id}] refused: {why}"),
        Note::Started {
            pid,
            resumed,
            share,
        } => println!(
            "[{id}] {} (pid {pid}); share: {} threads, {}, workers auto",
            if *resumed { "resuming" } else { "starting" },
            share.threads.unwrap_or(0),
            share
                .ram_budget_mb
                .map_or("RAM read by Arnis".into(), |mb| format!("{mb} MB RAM")),
        ),
        Note::Stopping => println!("stopping: killing running jobs"),
        Note::Finished(st) => match &st.error {
            Some(e) => println!("[{id}] {:?}: {e}", st.status),
            None => println!("[{id}] {:?}", st.status),
        },
        Note::Event(e) => match e {
            Event::Phase { name, .. } => println!("[{id}] {name}"),
            Event::Progress { progress } => {
                let t = (*progress / 10.0) as i64;
                if tenths.insert(id.to_string(), t) != Some(t) {
                    println!("[{id}] {progress:.0}%");
                }
            }
            Event::Piece {
                piece,
                of,
                state,
                wall_s,
                ..
            } => match wall_s {
                Some(w) => println!("[{id}] piece {}/{of} {state} in {w:.1}s", piece + 1),
                None => println!("[{id}] piece {}/{of} {state}", piece + 1),
            },
            Event::Transfer {
                stage,
                name,
                percent,
                ..
            } => println!("[{id}] {stage} {name} {percent:.0}%"),
            Event::Error { message } => println!("[{id}] error: {message}"),
            Event::Done { wall_s, chunks, .. } => {
                println!("[{id}] done: {chunks} chunks in {wall_s:.1}s")
            }
            Event::Other => {}
        },
    }
}

fn status(project: Option<&Path>) -> Result<()> {
    let dirs: Vec<PathBuf> = match project {
        Some(p) => vec![state::project_dir(&Project::load(p)?)],
        None => {
            let root = state::data_dir().join("projects");
            match std::fs::read_dir(&root) {
                Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path())).collect(),
                Err(_) => vec![],
            }
        }
    };
    if dirs.is_empty() {
        println!("no projects yet ({})", state::data_dir().display());
    }
    for dir in dirs {
        let s = State::load(&dir)?;
        println!("{} ({})", s.name, s.project.display());
        for (id, st) in &s.selections {
            let pieces = if st.pieces > 0 {
                format!(" pieces {}/{}", st.pieces_done, st.pieces)
            } else {
                String::new()
            };
            println!(
                "  {id:<16} {:<8} {:>5.1}%{pieces} runs {}{}",
                format!("{:?}", st.status).to_lowercase(),
                st.progress,
                st.runs,
                st.error
                    .as_deref()
                    .map(|e| format!("  {e}"))
                    .unwrap_or_default()
            );
        }
    }
    Ok(())
}
