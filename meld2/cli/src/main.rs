//! `meld2`: build a project of selections through Arnis at Scale, headless.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use meld_core::arnis::Arnis;
use meld_core::install::{self, Found, Probe};
use meld_core::plan::{self, Disk};
use meld_core::progress::Event;
use meld_core::project::Project;
use meld_core::queue::{self, Note};
use meld_core::state::{self, State};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
        Cmd::Run { project, arnis } => run(&project, arnis),
        Cmd::Plan { project, arnis } => {
            let project = Project::load(&project)?;
            let (arnis, _, _) = resolve(arnis, Some(&project))?;
            show_plan(&project, &arnis)
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
    }
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

fn run(path: &Path, arnis: Option<PathBuf>) -> Result<()> {
    let project = Project::load(path)?;
    let dir = state::project_dir(&project);
    let _lock = state::lock(&dir)?;
    let (arnis, found, probe) = resolve(arnis, Some(&project))?;
    let caps = probe.caps;
    println!(
        "{}: {} selection(s), {} job(s) at once, arnis {} ({}, {})",
        project.name,
        project.selections.len(),
        project.run.jobs,
        probe.version,
        found.source,
        found.path.display()
    );
    show_plan(&project, &arnis)?;

    let stop_file = dir.join("stop");
    let _ = std::fs::remove_file(&stop_file);
    let mut tenths: HashMap<String, i64> = HashMap::new();
    let summary = queue::run(
        &project,
        &arnis,
        &caps,
        &dir,
        &|| stop_file.exists(),
        &mut |id, note| show(id, note, &mut tenths),
    )?;
    let _ = std::fs::remove_file(&stop_file);
    println!(
        "{}: {} built, {} skipped, {} stopped, {} failed (state: {})",
        project.name,
        summary.done,
        summary.skipped,
        summary.stopped,
        summary.failed,
        dir.display()
    );
    if summary.failed > 0 {
        bail!("{} selection(s) failed", summary.failed);
    }
    Ok(())
}

/// The `--plan-units` dry run of every selection, summed against free disk.
/// Fails when the estimate does not fit.
fn show_plan(project: &Project, arnis: &Arnis) -> Result<()> {
    let plans = plan::project(project, arnis)?;
    println!(
        "  {:<16} {:>6} {:>7} {:>9} {:>9} {:>9}",
        "selection", "pieces", "regions", "chunks", "to build", "est. MB"
    );
    let mut need = 0.0;
    for (id, p) in &plans {
        need += p.todo_mb();
        println!(
            "  {id:<16} {:>6} {:>7} {:>9} {:>9} {:>9.1}",
            p.units.len(),
            p.regions(),
            p.chunks(),
            p.todo_chunks(),
            p.todo_mb()
        );
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
        Disk::Ok => println!("  disk: ok, {line}"),
        Disk::Tight => println!("  disk: WARNING, tight: {line}"),
        Disk::Short => bail!(
            "not enough disk: {line}. Free space on that volume, move `output`, shrink the selections, or lower run.min_free_mb"
        ),
    }
    Ok(())
}

/// One line per thing worth seeing; percentages every 10 %.
fn show(id: &str, note: Note, tenths: &mut HashMap<String, i64>) {
    match note {
        Note::Skipped(why) => println!("[{id}] skipped: {why}"),
        Note::Refused(why) => println!("[{id}] refused: {why}"),
        Note::Started {
            pid,
            resumed,
            share,
        } => println!(
            "[{id}] {} (pid {pid}); share: {} threads, {}, workers auto",
            if resumed { "resuming" } else { "starting" },
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
