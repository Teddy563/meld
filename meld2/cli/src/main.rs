//! `meld2`: build a project of selections through Arnis at Scale, headless.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use meld_core::arnis::Arnis;
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
        /// Arnis executable; else the project's `arnis`, else MELD2_ARNIS.
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
        arnis: PathBuf,
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
            let a = Arnis::new(arnis);
            println!("{}", a.version()?);
            println!("{}", a.capabilities()?.join(" "));
            Ok(())
        }
    }
}

fn run(path: &Path, arnis: Option<PathBuf>) -> Result<()> {
    let project = Project::load(path)?;
    let exe = arnis
        .or_else(|| {
            project
                .arnis
                .as_ref()
                .map(|a| path.parent().unwrap_or(Path::new(".")).join(a))
        })
        .or_else(|| std::env::var_os("MELD2_ARNIS").map(PathBuf::from))
        .context("no Arnis: pass --arnis, set `arnis` in the project or MELD2_ARNIS")?;
    let arnis = Arnis::new(exe);
    let caps = arnis.capabilities()?;
    println!(
        "{}: {} selection(s), {} job(s) at once, {}",
        project.name,
        project.selections.len(),
        project.run.jobs,
        arnis.version()?
    );

    let dir = state::project_dir(&project);
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

/// One line per thing worth seeing; percentages every 10 %.
fn show(id: &str, note: Note, tenths: &mut HashMap<String, i64>) {
    match note {
        Note::Skipped(why) => println!("[{id}] skipped: {why}"),
        Note::Refused(why) => println!("[{id}] refused: {why}"),
        Note::Started { pid, resumed } => println!(
            "[{id}] {} (pid {pid})",
            if resumed { "resuming" } else { "starting" }
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
