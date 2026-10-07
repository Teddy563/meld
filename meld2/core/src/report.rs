//! The JSON report of one run, built from the notes the run sends: every
//! step with its share, times, result and pieces, then the final check.

use crate::progress::Event;
use crate::queue::{Note, Summary};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Serialize)]
pub struct Report {
    pub project: PathBuf,
    pub name: String,
    pub meld: &'static str,
    /// Unix seconds; every other time is seconds since this.
    pub started_unix: u64,
    pub wall_s: f64,
    pub summary: Summary,
    pub steps: BTreeMap<String, StepReport>,
    /// Chunks each built selection still lacks, from `--plan-units` after the run.
    pub missing_chunks: BTreeMap<String, u64>,
    #[serde(skip)]
    t0: Instant,
}

#[derive(Default, Serialize)]
pub struct StepReport {
    pub status: String,
    pub reason: Option<String>,
    pub start_s: Option<f64>,
    pub end_s: Option<f64>,
    pub pid: Option<u32>,
    pub threads: Option<u32>,
    pub ram_budget_mb: Option<u64>,
    pub wall_s: Option<f64>,
    pub cpu_s: Option<f64>,
    pub peak_rss_mb: Option<u64>,
    pub chunks: Option<u64>,
    pub error: Option<String>,
    pub pieces: Vec<PieceReport>,
}

#[derive(Serialize)]
pub struct PieceReport {
    pub piece: u32,
    pub of: u32,
    pub state: String,
    pub at_s: f64,
    pub wall_s: Option<f64>,
    pub peak_rss_mb: Option<u64>,
}

impl Report {
    pub fn new(project: &Path, name: &str) -> Self {
        Self {
            project: project.to_path_buf(),
            name: name.to_string(),
            meld: env!("CARGO_PKG_VERSION"),
            started_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            wall_s: 0.0,
            summary: Summary::default(),
            steps: BTreeMap::new(),
            missing_chunks: BTreeMap::new(),
            t0: Instant::now(),
        }
    }

    /// Takes in one note of the run.
    pub fn note(&mut self, id: &str, note: &Note) {
        let now = self.t0.elapsed().as_secs_f64();
        if id.is_empty() {
            return;
        }
        let step = self.steps.entry(id.to_string()).or_default();
        match note {
            Note::Skipped(why) | Note::Refused(why) => {
                step.status = if matches!(note, Note::Skipped(_)) {
                    "skipped"
                } else {
                    "refused"
                }
                .into();
                step.reason = Some(why.to_string());
            }
            Note::Started { pid, share, .. } => {
                step.status = "running".into();
                step.start_s = Some(now);
                step.pid = Some(*pid);
                step.threads = share.threads;
                step.ram_budget_mb = share.ram_budget_mb;
            }
            Note::Finished(st) => {
                step.status = format!("{:?}", st.status).to_lowercase();
                step.end_s = Some(now);
                step.error = st.error.clone();
            }
            Note::Event(Event::Piece {
                piece,
                of,
                state,
                wall_s,
                peak_rss_mb,
            }) => step.pieces.push(PieceReport {
                piece: *piece,
                of: *of,
                state: state.clone(),
                at_s: now,
                wall_s: *wall_s,
                peak_rss_mb: *peak_rss_mb,
            }),
            Note::Event(Event::Done {
                wall_s,
                cpu_s,
                peak_rss_mb,
                chunks,
                ..
            }) => {
                step.wall_s = Some(*wall_s);
                step.cpu_s = *cpu_s;
                step.peak_rss_mb = *peak_rss_mb;
                step.chunks = Some(*chunks);
            }
            Note::Event(_) | Note::Stopping => {}
        }
    }

    /// Writes `reports/run-<unix>.json` under `dir` and returns its path.
    pub fn write(&mut self, dir: &Path, summary: Summary) -> std::io::Result<PathBuf> {
        self.wall_s = self.t0.elapsed().as_secs_f64();
        self.summary = summary;
        let reports = dir.join("reports");
        std::fs::create_dir_all(&reports)?;
        let path = reports.join(format!("run-{}.json", self.started_unix));
        std::fs::write(&path, serde_json::to_vec_pretty(self)?)?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::Share;
    use crate::state::{SelState, Status};

    #[test]
    fn report_lists_steps_and_every_piece() {
        let mut r = Report::new(Path::new("p.toml"), "P");
        r.note("a", &Note::Skipped("already built"));
        let share = Share {
            threads: Some(4),
            ram_budget_mb: Some(1000),
            workers_auto: true,
        };
        r.note(
            "b",
            &Note::Started {
                pid: 7,
                resumed: false,
                share,
            },
        );
        for (piece, state) in [(0, "start"), (0, "done"), (1, "skipped")] {
            let e = Event::Piece {
                piece,
                of: 2,
                state: state.into(),
                wall_s: None,
                peak_rss_mb: None,
            };
            r.note("b", &Note::Event(&e));
        }
        let done = SelState {
            status: Status::Done,
            ..Default::default()
        };
        r.note("b", &Note::Finished(&done));
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["steps"]["a"]["status"], "skipped");
        assert_eq!(v["steps"]["b"]["status"], "done");
        assert_eq!(v["steps"]["b"]["threads"], 4);
        assert_eq!(v["steps"]["b"]["pieces"].as_array().unwrap().len(), 3);
    }
}
