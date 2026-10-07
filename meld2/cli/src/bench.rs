//! Runs a bench (`meld_core::bench`): each arm a fresh one-selection project
//! under `<out>/<arm>/`, built by the same `run_project` as `meld2 run`.

use anyhow::{Context, Result};
use meld_core::bench::{self, Report, Request, Row};
use meld_core::progress::Event;
use meld_core::queue::Note;
use std::path::Path;

fn unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Builds every arm in turn, saving `bench.json`, `bench.csv` and `bench.md`
/// in `out` after each, and returns the report. An A/B whose sides did not
/// build the same pair has `pair_error` set.
pub fn run(
    req: &Request,
    out: &Path,
    say: &mut dyn FnMut(String),
    on: &mut dyn FnMut(&str, &Note),
) -> Result<Report> {
    let arms = req.arms()?;
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut report = Report {
        id: out
            .file_name()
            .map_or("bench".into(), |n| n.to_string_lossy().into_owned()),
        started: unix(),
        cores,
        bbox: req.bbox(),
        request: serde_json::to_value(req)?,
        ..Default::default()
    };
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    save(out, &report)?;
    for arm in &arms {
        say(format!(
            "bench: arm {} ({} of {})",
            arm.name,
            report.rows.len() + 1,
            arms.len()
        ));
        let dir = out.join(&arm.name);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).with_context(|| format!("clearing {}", dir.display()))?;
        }
        std::fs::create_dir_all(&dir)?;
        let file = dir.join("project.toml");
        std::fs::write(&file, bench::project_toml(&report.id, report.bbox, arm)?)?;
        // A fresh world and a fresh run state: its prewarm step runs too.
        let st = meld_core::state::project_dir(&meld_core::project::Project::load(&file)?);
        if st.exists() {
            std::fs::remove_dir_all(&st).with_context(|| format!("clearing {}", st.display()))?;
        }
        let mut row = Row {
            name: arm.name.clone(),
            settings: serde_json::to_value(&arm.settings)?,
            ..Default::default()
        };
        match crate::resolve(arm.arnis.clone(), None) {
            Ok((_, found, probe)) => {
                row.arnis = found.path.display().to_string();
                row.arnis_version = probe.version.to_string();
            }
            Err(e) => row.error = Some(format!("{e:#}")),
        }
        if row.error.is_none() {
            let mut done = None;
            let mut failed = None;
            let mut piece_peak = None;
            let result =
                crate::run_project(&file, arm.arnis.clone(), Some(&[]), say, &mut |id, note| {
                    if id == bench::ID {
                        match note {
                            Note::Event(Event::Done {
                                wall_s,
                                cpu_s,
                                peak_rss_mb,
                                chunks,
                                tree_peak_mb,
                            }) => {
                                done = Some((*wall_s, *cpu_s, *peak_rss_mb, *chunks, *tree_peak_mb))
                            }
                            Note::Event(Event::Piece { peak_rss_mb, .. }) => {
                                piece_peak = piece_peak.max(*peak_rss_mb)
                            }
                            Note::Finished(st) if st.error.is_some() => failed = st.error.clone(),
                            _ => {}
                        }
                    }
                    on(id, note);
                });
            match (result, done) {
                (Err(e), _) => row.error = Some(format!("{e:#}")),
                (Ok(_), Some((w, c, p, n, tree))) => {
                    row.done(w, c, p.max(piece_peak), n, cores);
                    row.tree_peak_mb = tree;
                }
                (Ok(_), None) => {
                    row.error = Some(failed.unwrap_or_else(|| "no done record from Arnis".into()))
                }
            }
            let world = dir.join("saves").join(bench::WORLD);
            let (bytes, regions) = bench::world_size(&world);
            row.disk_mb = bytes as f64 / 1e6;
            row.regions = regions;
            row.manifest = std::fs::read(world.join("arnis_one_world.json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
        }
        if let Some(e) = &row.error {
            say(format!("bench: arm {} failed: {e}", arm.name));
        }
        report.rows.push(row);
        save(out, &report)?;
    }
    if req.ab.is_some() {
        let (a, b) = (&report.rows[0], &report.rows[1]);
        match bench::same_pair(&a.manifest, &b.manifest) {
            Ok(diff) => report.pair_diff = Some(diff),
            Err(e) => report.pair_error = Some(format!("{e:#}")),
        }
    }
    report.finished = Some(unix());
    save(out, &report)?;
    Ok(report)
}

fn save(out: &Path, r: &Report) -> Result<()> {
    let tmp = out.join("bench.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(r)?)?;
    std::fs::rename(&tmp, out.join("bench.json"))?;
    std::fs::write(out.join("bench.csv"), r.csv())?;
    std::fs::write(out.join("bench.md"), r.table())?;
    Ok(())
}
