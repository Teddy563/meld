//! Runs a project's selections through Arnis: up to `run.jobs` at once,
//! never two on one world (a One World has one writer), each with an even
//! share of the CPU and memory budget. State is saved as it changes, so a
//! later run resumes: finished selections are skipped and a partial one is
//! started again with the same command, which Arnis resumes piece by piece.

use crate::args::{self, Share};
use crate::arnis::{Arnis, Process};
use crate::progress::{self, Event};
use crate::project::{Project, Selection};
use crate::state::{SelState, State, Status};
use anyhow::Result;
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

/// What a run reports as it goes, for the CLI (and later the GUI) to show.
pub enum Note<'a> {
    Skipped(&'a str),
    Refused(&'a str),
    Started { pid: u32, resumed: bool },
    Event(&'a Event),
    Finished(&'a SelState),
    Stopping,
}

#[derive(Debug, Default, PartialEq)]
pub struct Summary {
    pub done: usize,
    pub skipped: usize,
    pub failed: usize,
    pub stopped: usize,
}

#[derive(Debug, PartialEq)]
pub enum Decision {
    Run,
    Skip(&'static str),
    Refuse(&'static str),
}

/// What to do with a selection given its saved state and its command now.
pub fn decide(prev: Option<&SelState>, command: &[String]) -> Decision {
    let Some(prev) = prev else {
        return Decision::Run;
    };
    let same = prev.command == command;
    match prev.status {
        Status::Done if same => Decision::Skip("already built"),
        Status::Done => Decision::Skip(
            "built with other settings; Meld does not rebuild a built area on its own",
        ),
        // Arnis resumes a job by area and piece size, not by settings, so a partial
        // job finished with other settings would mix two worlds.
        Status::Running | Status::Stopped if !same => Decision::Refuse(
            "a partial job was started with other settings; restore them to resume it",
        ),
        _ if !same && prev.pieces_done > 0 => Decision::Refuse(
            "a partial job was started with other settings; restore them to resume it",
        ),
        _ => Decision::Run,
    }
}

enum Msg {
    Event(Event),
    Exit(std::io::Result<std::process::ExitStatus>),
}

struct Job {
    process: Arc<Process>,
    world: String,
    finished: bool,
}

/// Runs every selection that is not built yet. `stop` is polled twice a
/// second; when it says so, running jobs are killed and kept as `stopped`.
pub fn run(
    project: &Project,
    arnis: &Arnis,
    caps: &[String],
    dir: &Path,
    stop: &dyn Fn() -> bool,
    on: &mut dyn FnMut(&str, Note),
) -> Result<Summary> {
    let mut state = State::load(dir)?;
    state.project = project.path.clone();
    state.name = project.name.clone();
    let saves = project.output_dir();
    let logs = dir.join("logs");
    std::fs::create_dir_all(&saves)?;
    std::fs::create_dir_all(&logs)?;

    let jobs = project.run.jobs as usize;
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    let share = Share {
        threads: Some((cores * project.run.cpu_target as usize / 100 / jobs).max(1) as u32),
        ram_budget_mb: project.run.ram_budget_mb.map(|mb| mb / jobs as u64),
    };

    let mut summary = Summary::default();
    let mut queue: VecDeque<&Selection> = VecDeque::new();
    for sel in &project.selections {
        let command = args::build(sel, &project.settings_for(sel), &saves, Share::default()).args;
        match decide(state.selections.get(&sel.id), &command) {
            Decision::Run => queue.push_back(sel),
            Decision::Skip(why) => {
                summary.skipped += 1;
                on(&sel.id, Note::Skipped(why));
            }
            Decision::Refuse(why) => {
                summary.failed += 1;
                let st = state.selections.entry(sel.id.clone()).or_default();
                st.error = Some(why.to_string());
                on(&sel.id, Note::Refused(why));
            }
        }
    }
    state.save(dir)?;

    let (tx, rx) = mpsc::channel::<(String, Msg)>();
    let mut running: HashMap<String, Job> = HashMap::new();
    let mut stopping = false;
    loop {
        while !stopping && running.len() < jobs {
            let free = |s: &&Selection| !running.values().any(|j| j.world == s.world);
            let Some(i) = queue.iter().position(free) else {
                break;
            };
            let sel = queue.remove(i).expect("position is in range");
            let settings = project.settings_for(sel);
            let command = args::build(sel, &settings, &saves, Share::default()).args;
            let inv = args::build(sel, &settings, &saves, share);
            let log = logs.join(format!("{}.log", sel.id));
            let st = state.selections.entry(sel.id.clone()).or_default();
            let resumed = st.status != Status::Pending && st.runs > 0;
            let started = args::require(&inv, caps).and_then(|_| arnis.spawn(&inv.args, &log));
            let (process, stdout) = match started {
                Ok(p) => p,
                Err(e) => {
                    st.status = Status::Failed;
                    st.error = Some(format!("{e:#}"));
                    summary.failed += 1;
                    on(&sel.id, Note::Finished(st));
                    state.save(dir)?;
                    continue;
                }
            };
            *st = SelState {
                status: Status::Running,
                command,
                runs: st.runs + 1,
                ..Default::default()
            };
            let process = Arc::new(process);
            on(
                &sel.id,
                Note::Started {
                    pid: process.id(),
                    resumed,
                },
            );
            state.save(dir)?;

            let (tx, id, p) = (tx.clone(), sel.id.clone(), Arc::clone(&process));
            let mut log = std::fs::OpenOptions::new().append(true).open(&log)?;
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    match progress::parse(&line) {
                        Some(e) => {
                            let _ = tx.send((id.clone(), Msg::Event(e)));
                        }
                        None => {
                            let _ = writeln!(log, "{line}");
                        }
                    }
                }
                let _ = tx.send((id, Msg::Exit(p.wait())));
            });
            running.insert(
                sel.id.clone(),
                Job {
                    process,
                    world: sel.world.clone(),
                    finished: false,
                },
            );
        }
        if running.is_empty() && (stopping || queue.is_empty()) {
            break;
        }

        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok((id, Msg::Event(e))) => {
                let st = state.selections.entry(id.clone()).or_default();
                let save = match &e {
                    Event::Progress { progress } => {
                        st.progress = *progress;
                        false
                    }
                    Event::Phase { progress, .. } => {
                        st.progress = progress.unwrap_or(st.progress);
                        false
                    }
                    Event::Piece { of, state: s, .. } => {
                        st.pieces = *of;
                        if s == "done" || s == "skipped" {
                            st.pieces_done += 1;
                        }
                        true
                    }
                    Event::Error { message } => {
                        st.error = Some(message.clone());
                        true
                    }
                    Event::Done { wall_s, chunks, .. } => {
                        st.wall_s = Some(*wall_s);
                        st.chunks = Some(*chunks);
                        st.progress = 100.0;
                        if let Some(j) = running.get_mut(&id) {
                            j.finished = true;
                        }
                        true
                    }
                    Event::Transfer { .. } | Event::Other => false,
                };
                on(&id, Note::Event(&e));
                if save {
                    state.save(dir)?;
                }
            }
            Ok((id, Msg::Exit(exit))) => {
                let job = running.remove(&id).expect("a running job exits once");
                let st = state.selections.entry(id.clone()).or_default();
                let ok = matches!(&exit, Ok(s) if s.success()) && job.finished;
                if ok {
                    st.status = Status::Done;
                    st.error = None;
                    summary.done += 1;
                } else if stopping {
                    st.status = Status::Stopped;
                    summary.stopped += 1;
                } else {
                    st.status = Status::Failed;
                    let why = st.error.take().unwrap_or_else(|| match &exit {
                        Ok(s) => format!("Arnis ended without finishing ({s})"),
                        Err(e) => format!("waiting for Arnis: {e}"),
                    });
                    st.error = Some(format!("{why}; log: logs/{id}.log"));
                    summary.failed += 1;
                }
                on(&id, Note::Finished(st));
                state.save(dir)?;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !stopping && stop() {
                    stopping = true;
                    on("", Note::Stopping);
                    for job in running.values() {
                        job.process.kill();
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => unreachable!("tx lives in this frame"),
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(status: Status, command: &[&str], pieces_done: u32) -> SelState {
        SelState {
            status,
            command: command.iter().map(|s| s.to_string()).collect(),
            pieces_done,
            runs: 1,
            ..Default::default()
        }
    }

    #[test]
    fn resume_decisions() {
        let now: Vec<String> = vec!["--bbox".into(), "1,2,3,4".into()];
        let same = ["--bbox", "1,2,3,4"];
        let other = ["--bbox", "1,2,3,4", "--caves"];
        let cases = [
            (None, Decision::Run),
            (
                Some(st(Status::Done, &same, 4)),
                Decision::Skip("already built"),
            ),
            (Some(st(Status::Running, &same, 2)), Decision::Run),
            (Some(st(Status::Stopped, &same, 2)), Decision::Run),
            (Some(st(Status::Failed, &same, 0)), Decision::Run),
            (Some(st(Status::Failed, &other, 0)), Decision::Run),
        ];
        for (prev, want) in cases {
            assert_eq!(decide(prev.as_ref(), &now), want, "{prev:?}");
        }
        for prev in [
            st(Status::Running, &other, 0),
            st(Status::Stopped, &other, 1),
            st(Status::Failed, &other, 3),
        ] {
            assert!(matches!(decide(Some(&prev), &now), Decision::Refuse(_)));
        }
        assert!(matches!(
            decide(Some(&st(Status::Done, &other, 4)), &now),
            Decision::Skip(w) if w.contains("other settings")
        ));
    }

    /// The whole loop against a stand-in Arnis: both selections build, a
    /// second run skips them, and one left `running` by a killed Meld reruns.
    #[cfg(windows)]
    #[test]
    fn runs_skips_and_resumes_with_a_fake_arnis() {
        let dir = std::env::temp_dir().join(format!("meld2-queue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("fake-arnis.cmd");
        std::fs::write(
            &fake,
            "@echo off\r\n\
             echo Arnis says hello\r\n\
             echo {\"v\":1,\"type\":\"piece\",\"of\":2,\"piece\":0,\"state\":\"skipped\"}\r\n\
             echo {\"v\":1,\"type\":\"piece\",\"of\":2,\"piece\":1,\"state\":\"done\",\"wall_s\":0.1,\"peak_rss_mb\":5}\r\n\
             echo {\"v\":1,\"type\":\"done\",\"wall_s\":0.2,\"cpu_s\":0.1,\"peak_rss_mb\":5,\"chunks\":7}\r\n",
        )
        .unwrap();
        let mut project = Project::parse(
            r#"
format = 1
name = "Fake"
output = "saves"
run = { jobs = 2 }
[[selection]]
id = "a"
bbox = [1.0, 2.0, 3.0, 4.0]
world = "W1"
[[selection]]
id = "b"
bbox = [1.0, 2.0, 3.0, 4.0]
world = "W2"
"#,
        )
        .unwrap();
        project.path = dir.join("fake.toml");
        let arnis = Arnis::new(&fake);
        let caps: Vec<String> = ["progress-json", "unit-regions", "threads"]
            .map(String::from)
            .to_vec();
        let state_dir = dir.join("state");
        let go = |notes: &mut Vec<String>| {
            run(
                &project,
                &arnis,
                &caps,
                &state_dir,
                &|| false,
                &mut |id, n| {
                    if let Note::Skipped(_) = n {
                        notes.push(format!("{id} skipped"));
                    }
                },
            )
            .unwrap()
        };

        let mut notes = vec![];
        let first = go(&mut notes);
        assert_eq!(
            first,
            Summary {
                done: 2,
                ..Default::default()
            }
        );
        let s = State::load(&state_dir).unwrap();
        assert_eq!(s.selections["a"].pieces_done, 2);
        assert_eq!(s.selections["b"].chunks, Some(7));
        let log = std::fs::read_to_string(state_dir.join("logs/a.log")).unwrap();
        assert!(log.contains("Arnis says hello") && !log.contains("\"v\":1"));

        let second = go(&mut notes);
        assert_eq!(
            second,
            Summary {
                skipped: 2,
                ..Default::default()
            }
        );

        // A Meld killed mid-run leaves `running` behind; that selection runs again.
        let mut s = State::load(&state_dir).unwrap();
        s.selections.get_mut("b").unwrap().status = Status::Running;
        s.save(&state_dir).unwrap();
        let third = go(&mut notes);
        assert_eq!(
            third,
            Summary {
                done: 1,
                skipped: 1,
                ..Default::default()
            }
        );
        assert_eq!(State::load(&state_dir).unwrap().selections["b"].runs, 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
