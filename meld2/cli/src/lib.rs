//! The parts of `meld2` the desktop app shares: the API server and a project run.

use anyhow::{bail, Context, Result};
use meld_core::arnis::Arnis;
use meld_core::install::{self, Found, Probe};
use meld_core::plan::{self, Disk};
use meld_core::project::Project;
use meld_core::queue;
use meld_core::queue::Note;
use meld_core::report::Report;
use meld_core::state::{self, State};
use std::path::{Path, PathBuf};

mod assets;
pub mod bench;
pub mod serve;

/// Finds (or downloads) Arnis and checks its version and capabilities.
pub fn resolve(flag: Option<PathBuf>, project: Option<&Project>) -> Result<(Arnis, Found, Probe)> {
    let found = install::locate(
        flag,
        project.and_then(Project::arnis_path),
        &state::data_dir(),
    )?;
    let arnis = Arnis::new(&found.path);
    let probe = install::probe(&arnis)?;
    Ok((arnis, found, probe))
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
    if project.selections.is_empty() {
        bail!("{}: nothing to build; add a [[selection]]", project.name);
    }
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
    let mut built: Vec<String> = vec![];
    let summary = queue::run(
        &project,
        &arnis,
        &probe.caps,
        &dir,
        &|| stop_file.exists(),
        &mut |id, note| {
            report.note(id, &note);
            if let Note::Finished(st) = &note {
                if st.status == state::Status::Done {
                    built.push(id.to_string());
                }
            }
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
    if let Some(ext) = &project.run.export {
        auto_export(&project, &built, ext, say);
    }
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

/// `[run] export`: packs each world a selection of this run built, into
/// `exports/` beside the project file. A failure is reported, not fatal.
fn auto_export(project: &Project, built: &[String], ext: &str, say: &mut dyn FnMut(String)) {
    let dir = project
        .path
        .parent()
        .unwrap_or(Path::new("."))
        .join("exports");
    for world in project.worlds() {
        let ran = project
            .selections
            .iter()
            .any(|s| s.world == world && built.contains(&s.id));
        if !ran {
            continue;
        }
        let src = project.output_dir().join(&world);
        let to = meld_core::export::name_in(&dir, &src, ext);
        match meld_core::export::export(&src, &to, project.run.min_free_mb) {
            Ok(e) => say(format!(
                "export: {} ({} file(s), {:.1} MB)",
                to.display(),
                e.files,
                e.zip_bytes as f64 / 1e6
            )),
            Err(e) => say(format!("export of {world} failed: {e:#}")),
        }
    }
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
pub fn show_plan(
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

/// The cache's parts (its top-level folders and files) with their sizes in bytes.
pub fn cache_parts(root: &Path) -> Vec<(String, u64)> {
    let mut parts: Vec<(String, u64)> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| {
            let p = e.path();
            let bytes = if p.is_dir() {
                meld_core::server::dir_bytes(&p).unwrap_or(0)
            } else {
                e.metadata().map_or(0, |m| m.len())
            };
            (e.file_name().to_string_lossy().into_owned(), bytes)
        })
        .collect();
    parts.sort();
    parts
}

/// Empties one part of the cache, or all of it; Arnis fetches again what it needs.
pub fn clear_cache(root: &Path, what: &str) -> Result<()> {
    let names: Vec<String> = cache_parts(root).into_iter().map(|p| p.0).collect();
    let pick: Vec<&String> = names
        .iter()
        .filter(|n| what == "all" || *n == what)
        .collect();
    if pick.is_empty() {
        bail!("no cache part {what:?}; there are: {}", names.join(", "));
    }
    for n in pick {
        let p = root.join(n);
        let r = if p.is_dir() {
            std::fs::remove_dir_all(&p)
        } else {
            std::fs::remove_file(&p)
        };
        r.with_context(|| format!("removing {}", p.display()))?;
    }
    Ok(())
}
