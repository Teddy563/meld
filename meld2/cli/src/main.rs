//! `meld2`: build a project of selections through Arnis at Scale, headless.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use meld_core::arnis::Arnis;
use meld_core::convert;
use meld_core::import;
use meld_core::install::{self, Found, Probe};
use meld_core::plan::{self, Disk};
use meld_core::progress::Event;
use meld_core::project::Project;
use meld_core::queue::{self, Note};
use meld_core::report::Report;
use meld_core::state::{self, State};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

mod serve;

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
    /// Which Arnis Meld uses, and installing the pinned release.
    #[command(subcommand)]
    Arnis(ArnisCmd),
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

/// The project's `arnis`, resolved against the project file.
fn project_arnis(p: &Project) -> Option<PathBuf> {
    let dir = p.path.parent().unwrap_or(Path::new("."));
    p.arnis.as_ref().map(|a| dir.join(a))
}

/// Finds (or downloads) Arnis and checks its version and capabilities.
fn resolve(flag: Option<PathBuf>, project: Option<&Project>) -> Result<(Arnis, Found, Probe)> {
    let found = install::locate(flag, project.and_then(project_arnis), &state::data_dir())?;
    let arnis = Arnis::new(&found.path);
    let probe = install::probe(&arnis)?;
    Ok((arnis, found, probe))
}

fn arnis_cmd(cmd: ArnisCmd) -> Result<()> {
    let data = state::data_dir();
    match cmd {
        ArnisCmd::Status { arnis, project } => {
            let project = project.map(|p| Project::load(&p)).transpose()?;
            let pin = format!("{} v{}", install::REPO, install::VERSION);
            let Some(found) =
                install::find_here(arnis, project.as_ref().and_then(project_arnis), &data)
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
            let found = install::find_here(arnis, project.as_ref().and_then(project_arnis), &data)
                .context("no Arnis found; run `meld2 arnis install`")?;
            println!("{}", found.path.display());
            Ok(())
        }
    }
}

/// Runs a project: lock, find Arnis, plan, rebuild what was asked, build,
/// check, and write the JSON report. Text goes to `say`, progress to `on`;
/// `meld2 run` prints both, `meld2 serve` streams them.
pub fn run_project(
    path: &Path,
    arnis: Option<PathBuf>,
    rebuild: Option<&[String]>,
    say: &mut dyn FnMut(String),
    on: &mut dyn FnMut(&str, &Note),
) -> Result<queue::Summary> {
    let project = Project::load(path)?;
    let dir = state::project_dir(&project);
    let _lock = state::lock(&dir)?;
    let (arnis, found, probe) = resolve(arnis, Some(&project))?;
    say(format!(
        "{}: {} selection(s), {} bake(s), {} job(s) at once, arnis {} ({}, {})",
        project.name,
        project.selections.len(),
        project.bakes.len(),
        project.run.jobs,
        probe.version,
        found.source,
        found.path.display()
    ));
    let plans = show_plan(&project, &arnis, say)?;
    if let Some(ids) = rebuild {
        start_over(&project, &dir, &plans, ids, say)?;
    }

    let stop_file = dir.join("stop");
    let _ = std::fs::remove_file(&stop_file);
    let mut report = Report::new(&project.path, &project.name);
    let summary = queue::run(
        &project,
        &arnis,
        &probe.caps,
        &dir,
        &|| stop_file.exists(),
        &mut |id, note| {
            report.note(id, &note);
            on(id, &note);
        },
    )?;
    let _ = std::fs::remove_file(&stop_file);

    // Final check: what Arnis still finds missing in every built selection.
    let st = State::load(&dir)?;
    for sel in &project.selections {
        if st.selections.get(&sel.id).map(|s| s.status) == Some(state::Status::Done) {
            let missing = plan::selection(&project, sel, &arnis)?.todo_chunks();
            report.missing_chunks.insert(sel.id.clone(), missing);
            if missing > 0 {
                say(format!(
                    "[{}] final check: {missing} chunk(s) missing; `meld2 run --rebuild={}` builds it again",
                    sel.id, sel.id
                ));
            }
        }
    }
    let checked = report.missing_chunks.len();
    let missing: u64 = report.missing_chunks.values().sum();
    say(format!(
        "final check: {checked} built selection(s), {missing} chunk(s) missing"
    ));
    let written = report.write(&dir, summary)?;
    say(format!(
        "{}: {} done, {} skipped, {} stopped, {} failed (report: {})",
        project.name,
        summary.done,
        summary.skipped,
        summary.stopped,
        summary.failed,
        written.display()
    ));
    Ok(summary)
}

/// `--rebuild`: forgets the selections' saved state and Arnis's partial job,
/// so they build every piece again over what is there.
fn start_over(
    project: &Project,
    dir: &Path,
    plans: &[(String, plan::Plan)],
    ids: &[String],
    say: &mut dyn FnMut(String),
) -> Result<()> {
    // `li` also names the parts `li-1`, `li-2`, ... of a polygon selection.
    let named = |id: &str, r: &str| {
        id == r
            || id
                .strip_prefix(r)
                .and_then(|t| t.strip_prefix('-'))
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    };
    for r in ids {
        if !project.selections.iter().any(|s| named(&s.id, r)) {
            bail!("--rebuild: no selection {r:?}");
        }
    }
    let mut st = State::load(dir)?;
    for (sel, (_, plan)) in project.selections.iter().zip(plans) {
        if !ids.is_empty() && !ids.iter().any(|r| named(&sel.id, r)) {
            continue;
        }
        let job = project
            .output_dir()
            .join(&sel.world)
            .join("arnis_one_world/jobs")
            .join(plan.job_dir());
        if job.exists() {
            std::fs::remove_dir_all(&job).with_context(|| format!("removing {}", job.display()))?;
        }
        st.selections.remove(&sel.id);
        say(format!(
            "[{}] rebuild: all {} piece(s); {} existing chunk(s) are replaced",
            sel.id,
            plan.units.len(),
            plan.chunks() - plan.todo_chunks()
        ));
    }
    st.save(dir)
}

/// The `--plan-units` dry run of every selection, summed against free disk.
/// Fails when the estimate does not fit.
fn show_plan(
    project: &Project,
    arnis: &Arnis,
    say: &mut dyn FnMut(String),
) -> Result<Vec<(String, plan::Plan)>> {
    let plans = plan::project(project, arnis)?;
    say(format!(
        "  {:<16} {:>6} {:>7} {:>9} {:>9} {:>9}",
        "selection", "pieces", "regions", "chunks", "to build", "est. MB"
    ));
    let mut need = 0.0;
    for (id, p) in &plans {
        need += p.todo_mb();
        say(format!(
            "  {id:<16} {:>6} {:>7} {:>9} {:>9} {:>9.1}",
            p.units.len(),
            p.regions(),
            p.chunks(),
            p.todo_chunks(),
            p.todo_mb()
        ));
    }
    let saves = project.output_dir();
    let free = plan::free_mb(&saves)?;
    let reserve = project.run.min_free_mb;
    let line = format!(
        "~{need:.0} MB to write (+{:.0} % margin), {free} MB free on {}, keeping {reserve} MB free",
        (plan::MARGIN - 1.0) * 100.0,
        saves.display()
    );
    match plan::verdict(need, free, reserve) {
        Disk::Ok => say(format!("  disk: ok, {line}")),
        Disk::Tight => say(format!("  disk: WARNING, tight: {line}")),
        Disk::Short => bail!(
            "not enough disk: {line}. Free space on that volume, move `output`, shrink the selections, or lower run.min_free_mb"
        ),
    }
    Ok(plans)
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
