//! Runs a project's selections through Arnis: up to `run.jobs` at once,
//! never two on one world (a One World has one writer), each with an even
//! share of the CPU and memory budget. State is saved as it changes, so a
//! later run resumes: finished selections are skipped and a partial one is
//! started again with the same command, which Arnis resumes piece by piece.

use crate::args::{self, Invocation, Share};
use crate::arnis::{Arnis, Process};
use crate::progress::{self, Event};
use crate::project::{Bake, Project, Selection};
use crate::state::{SelState, State, Status};
use anyhow::Result;
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

/// What a run reports as it goes, for the CLI (and later the GUI) to show.
pub enum Note<'a> {
    Skipped(&'a str),
    Refused(&'a str),
    Started {
        pid: u32,
        resumed: bool,
        share: Share,
    },
    Event(&'a Event),
    Finished(&'a SelState),
    Stopping,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
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

const CHANGED: &str = "a partial job was started with other settings; restore them to resume it, or start over with `meld2 run --rebuild <id>`";

/// What to do with a selection given its saved state and its command now.
pub fn decide(prev: Option<&SelState>, command: &[String]) -> Decision {
    let Some(prev) = prev else {
        return Decision::Run;
    };
    let same = prev.command == command;
    match prev.status {
        Status::Done if same => Decision::Skip("already built"),
        Status::Done => Decision::Skip(
            "built with other settings; Meld does not rebuild a built area unless asked (--rebuild <id>)",
        ),
        // Arnis resumes a job by area and piece size, not by settings, so a partial
        // job finished with other settings would mix two worlds.
        Status::Running | Status::Stopped if !same => Decision::Refuse(CHANGED),
        _ if !same && prev.pieces_done > 0 => Decision::Refuse(CHANGED),
        _ => Decision::Run,
    }
}

/// One job's part of the run's budget when `slots` jobs share it: threads
/// from `cpu_target` % of the cores, and MB of `ram_mb`.
pub fn share(cores: usize, cpu_target: u32, ram_mb: Option<u64>, slots: usize) -> Share {
    let slots = slots.max(1);
    Share {
        threads: Some((cores * cpu_target as usize / 100 / slots).max(1) as u32),
        ram_budget_mb: ram_mb.map(|mb| mb / slots as u64),
        workers_auto: true,
    }
}

/// Jobs that can run at once: one per world (a One World has one writer),
/// at most `jobs`.
pub fn slots<'a>(jobs: usize, worlds: impl Iterator<Item = &'a str>) -> usize {
    let distinct: std::collections::HashSet<_> = worlds.collect();
    distinct.len().min(jobs)
}

/// 80 % of the memory free now, for the jobs to split. `None` where Meld
/// cannot read it (macOS): each Arnis then reads free memory itself.
// ponytail: read once per run; next to other heavy programs, set run.ram_budget_mb.
fn available_ram_mb() -> Option<u64> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        // SAFETY: a zeroed struct with its length set, as the call requires.
        unsafe {
            let mut m: MEMORYSTATUSEX = std::mem::zeroed();
            m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
            (GlobalMemoryStatusEx(&mut m) != 0).then(|| m.ullAvailPhys / (1024 * 1024) * 4 / 5)
        }
    }
    #[cfg(not(windows))]
    {
        let info = std::fs::read_to_string("/proc/meminfo").ok()?;
        let line = info.lines().find(|l| l.starts_with("MemAvailable:"))?;
        let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb / 1024 * 4 / 5)
    }
}

/// Keeps the machine awake while a run builds, until the guard drops.
/// Windows: a flag on this thread, which also lapses when the thread ends.
/// Linux: `systemd-inhibit`; macOS: `caffeinate`. Each holds until Meld closes
/// its stdin or dies, so it never outlives Meld. Missing tool: no inhibit.
struct Awake(Option<std::process::Child>);

fn keep_awake() -> Awake {
    #[cfg(windows)]
    // SAFETY: a plain flag call with no pointers.
    unsafe {
        use windows_sys::Win32::System::Power::{
            SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
        };
        SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED);
        Awake(None)
    }
    #[cfg(not(windows))]
    {
        use std::process::{Command, Stdio};
        let mut cmd = if cfg!(target_os = "macos") {
            let mut c = Command::new("caffeinate");
            c.args(["-i", "-w", &std::process::id().to_string()]);
            c
        } else {
            let mut c = Command::new("systemd-inhibit");
            c.args([
                "--what=sleep:idle",
                "--who=meld2",
                "--why=building Minecraft worlds",
                "--mode=block",
                "cat",
            ]);
            c
        };
        let child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        Awake(child.ok())
    }
}

impl Drop for Awake {
    fn drop(&mut self) {
        if let Some(c) = self.0.as_mut() {
            drop(c.stdin.take());
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// One Arnis process of a run.
#[derive(Clone, Copy, Debug)]
pub enum Step<'a> {
    /// An `.osm.pbf` cut once for the selections that read it.
    Bake(&'a Bake),
    /// The selection's command with `--prewarm`: caches only, no world.
    Prewarm(&'a Selection),
    Build(&'a Selection),
}

impl Step<'_> {
    /// The CPU share it runs at: `bake_cpu` for bakes and prewarms when set.
    pub fn cpu_target(&self, run: &crate::project::Budget) -> u32 {
        match self {
            Step::Bake(_) | Step::Prewarm(_) => run.bake_cpu.unwrap_or(run.cpu_target),
            Step::Build(_) => run.cpu_target,
        }
    }

    /// Its key in `state.json` and in notes: `bake:<id>`, `prewarm:<id>`, or the selection id.
    pub fn key(&self) -> String {
        match self {
            Step::Bake(b) => format!("bake:{}", b.id),
            Step::Prewarm(s) => format!("prewarm:{}", s.id),
            Step::Build(s) => s.id.clone(),
        }
    }

    /// What it holds while it runs: its One World, or for a bake itself.
    fn world(&self) -> String {
        match self {
            Step::Bake(_) => self.key(),
            Step::Prewarm(s) | Step::Build(s) => s.world.clone(),
        }
    }

    pub fn invocation(&self, project: &Project, saves: &Path, share: Share) -> Invocation {
        match self {
            Step::Bake(b) => args::bake(
                project.bake_bbox(b),
                &b.osm_pbf,
                b.osm_pbf_url.as_deref(),
                share,
            ),
            Step::Prewarm(s) => {
                args::prewarm(args::build(s, &project.settings_for(s), saves, share))
            }
            Step::Build(s) => args::build(s, &project.settings_for(s), saves, share),
        }
    }
}

/// Every step of a project in run order: the bakes, one at a time, then per
/// selection a lane of its prewarm (when `prewarm = true`) and its build, which
/// run back to back on that selection's world.
pub fn lanes(project: &Project) -> (Vec<Step<'_>>, Vec<Vec<Step<'_>>>) {
    let bakes = project.bakes.iter().map(Step::Bake).collect();
    let sels = project
        .selections
        .iter()
        .map(|s| {
            let prewarm =
                project.settings_for(s).get(args::PREWARM) == Some(&toml::Value::Boolean(true));
            let mut lane = vec![];
            if prewarm {
                lane.push(Step::Prewarm(s));
            }
            lane.push(Step::Build(s));
            lane
        })
        .collect();
    (bakes, sels)
}

enum Msg {
    Event(Event),
    Exit(std::io::Result<std::process::ExitStatus>),
}

struct Job<'a> {
    process: Arc<Process>,
    world: String,
    finished: bool,
    /// The steps of its lane still to run after this one.
    rest: VecDeque<Step<'a>>,
}

type Lane<'a> = VecDeque<Step<'a>>;

/// One run's shared parts, for the bakes and then the selections.
struct Runner<'a, 'p> {
    project: &'p Project,
    arnis: &'a Arnis,
    caps: &'a [String],
    dir: &'a Path,
    saves: PathBuf,
    logs: PathBuf,
    cores: usize,
    ram_mb: Option<u64>,
    state: State,
    summary: Summary,
    stop: &'a dyn Fn() -> bool,
    on: &'a mut dyn FnMut(&str, Note),
    stopping: bool,
}

/// Runs every step that is not done yet: the bakes first, then the
/// selections. `stop` is polled twice a second; when it says so, running
/// jobs are killed and kept as `stopped`. The caller holds
/// `state::lock(dir)` around it.
pub fn run(
    project: &Project,
    arnis: &Arnis,
    caps: &[String],
    dir: &Path,
    stop: &dyn Fn() -> bool,
    on: &mut dyn FnMut(&str, Note),
) -> Result<Summary> {
    let _awake = keep_awake();
    let mut state = State::load(dir)?;
    state.project = project.path.clone();
    state.name = project.name.clone();
    let saves = project.output_dir();
    let logs = dir.join("logs");
    std::fs::create_dir_all(&saves)?;
    std::fs::create_dir_all(&logs)?;
    let mut r = Runner {
        project,
        arnis,
        caps,
        dir,
        saves,
        logs,
        cores: std::thread::available_parallelism().map_or(4, |n| n.get()),
        ram_mb: project.run.ram_budget_mb.or_else(available_ram_mb),
        state,
        summary: Summary::default(),
        stop,
        on,
        stopping: false,
    };
    let (bakes, sels) = lanes(project);
    r.run_lanes(bakes.into_iter().map(|b| vec![b]).collect(), 1)?;
    r.run_lanes(sels, project.run.jobs as usize)?;
    if !r.stopping {
        r.convert_worlds()?;
    }
    Ok(r.summary)
}

impl<'p> Runner<'_, 'p> {
    /// The steps of `lane` that still have to run, after the saved state.
    fn pending(&mut self, lane: Vec<Step<'p>>) -> Lane<'p> {
        let mut out = VecDeque::new();
        // The build first: a built or refused selection needs no prewarm.
        for step in lane.into_iter().rev() {
            let key = step.key();
            let command = step
                .invocation(self.project, &self.saves, Share::default())
                .args;
            let prev = self.state.selections.get(&key);
            let decision = match step {
                Step::Build(sel) => match self.failed_bake(sel) {
                    Some(why) => Decision::Refuse(why),
                    None => decide(prev, &command),
                },
                // A cache step is redone unless it finished with this very command.
                _ if prev.is_some_and(|p| p.status == Status::Done && p.command == command) => {
                    Decision::Skip("already done")
                }
                _ => Decision::Run,
            };
            match decision {
                Decision::Run => out.push_front(step),
                Decision::Skip(why) => {
                    self.summary.skipped += 1;
                    (self.on)(&key, Note::Skipped(why));
                    if matches!(step, Step::Build(_)) {
                        return VecDeque::new();
                    }
                }
                Decision::Refuse(why) => {
                    self.summary.failed += 1;
                    let st = self.state.selections.entry(key.clone()).or_default();
                    st.error = Some(why.to_string());
                    (self.on)(&key, Note::Refused(why));
                    return VecDeque::new();
                }
            }
        }
        out
    }

    /// `[server] format = "blinear"`: each served world whose selections are
    /// all built converts to its B_Linear copy (`convert:<world>`), unless its
    /// regions did not change since the last conversion.
    fn convert_worlds(&mut self) -> Result<()> {
        let Ok(server) = crate::server::Server::of(self.project) else {
            return Ok(());
        };
        if server.conf.format != crate::server::Format::Blinear {
            return Ok(());
        }
        for world in &server.conf.worlds {
            let key = format!("convert:{world}");
            let built = self
                .project
                .selections
                .iter()
                .filter(|s| &s.world == world)
                .all(|s| {
                    self.state
                        .selections
                        .get(&s.id)
                        .is_some_and(|st| st.status == Status::Done)
                });
            let src = self.saves.join(world);
            let dest = crate::convert::sibling(&src);
            let source = crate::convert::fingerprint(&src)?;
            let why = if !built {
                Some("not every selection of the world is built")
            } else if crate::convert::previous(&dest).is_some_and(|c| c.source == source) {
                Some("already converted, and the world did not change since")
            } else {
                None
            };
            if let Some(why) = why {
                self.summary.skipped += 1;
                (self.on)(&key, Note::Skipped(why));
                continue;
            }
            let phase = Event::Phase {
                name: format!("converting {world} to B_Linear in {}", dest.display()),
                progress: Some(0.0),
            };
            (self.on)(&key, Note::Event(&phase));
            let threads = (self.cores * self.project.run.cpu_target as usize / 100).max(1);
            let (stop, on) = (self.stop, &mut *self.on);
            let result =
                crate::convert::convert(&src, &dest, threads, false, stop, &mut |d, of| {
                    let progress = 100.0 * d as f64 / of.max(1) as f64;
                    on(&key, Note::Event(&Event::Progress { progress }));
                });
            let st = self.state.selections.entry(key.clone()).or_default();
            st.runs += 1;
            st.command = source;
            match result {
                Ok(c) => {
                    st.status = Status::Done;
                    st.error = None;
                    st.progress = 100.0;
                    st.chunks = Some(c.chunks as u64);
                    self.summary.done += 1;
                }
                Err(_) if stop() => {
                    st.status = Status::Stopped;
                    self.summary.stopped += 1;
                    self.stopping = true;
                }
                Err(e) => {
                    st.status = Status::Failed;
                    st.error = Some(format!("{e:#}"));
                    self.summary.failed += 1;
                }
            }
            (self.on)(&key, Note::Finished(st));
            self.state.save(self.dir)?;
            if self.stopping {
                break;
            }
        }
        Ok(())
    }

    /// Why `sel` cannot build: a bake of its extract did not finish.
    fn failed_bake(&self, sel: &Selection) -> Option<&'static str> {
        let failed = self.project.bakes.iter().any(|b| {
            self.project.reads(sel, b)
                && self
                    .state
                    .selections
                    .get(&Step::Bake(b).key())
                    .is_none_or(|st| st.status != Status::Done)
        });
        failed.then_some("the bake of its osm_pbf did not finish")
    }

    fn run_lanes(&mut self, lanes: Vec<Vec<Step<'p>>>, jobs: usize) -> Result<()> {
        let mut queue: VecDeque<Lane<'p>> = VecDeque::new();
        for lane in lanes {
            let lane = self.pending(lane);
            if !lane.is_empty() {
                queue.push_back(lane);
            }
        }
        self.state.save(self.dir)?;

        let (tx, rx) = mpsc::channel::<(String, Msg)>();
        let mut running: HashMap<String, Job<'p>> = HashMap::new();
        loop {
            while !self.stopping && running.len() < jobs {
                let free = |l: &Lane| !running.values().any(|j| j.world == l[0].world());
                let Some(i) = queue.iter().position(free) else {
                    break;
                };
                let lane = queue.remove(i).expect("position is in range");
                self.start(lane, &mut running, &queue, jobs, &tx)?;
            }
            if running.is_empty() && (self.stopping || queue.is_empty()) {
                return Ok(());
            }

            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok((id, Msg::Event(e))) => self.event(&id, e, &mut running)?,
                Ok((id, Msg::Exit(exit))) => {
                    let job = running.remove(&id).expect("a running job exits once");
                    let st = self.state.selections.entry(id.clone()).or_default();
                    let ok = matches!(&exit, Ok(s) if s.success()) && job.finished;
                    if ok {
                        st.status = Status::Done;
                        st.error = None;
                        self.summary.done += 1;
                    } else if self.stopping {
                        st.status = Status::Stopped;
                        self.summary.stopped += 1;
                    } else {
                        st.status = Status::Failed;
                        let why = st.error.take().unwrap_or_else(|| match &exit {
                            Ok(s) => format!("Arnis ended without finishing ({s})"),
                            Err(e) => format!("waiting for Arnis: {e}"),
                        });
                        st.error = Some(format!("{why}; log: logs/{}.log", log_name(&id)));
                        self.summary.failed += 1;
                    }
                    (self.on)(&id, Note::Finished(st));
                    self.state.save(self.dir)?;
                    // The lane goes on in the slot it holds, on the same world.
                    if ok && !self.stopping && !job.rest.is_empty() {
                        self.start(job.rest, &mut running, &queue, jobs, &tx)?;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if !self.stopping && (self.stop)() {
                        self.stopping = true;
                        (self.on)("", Note::Stopping);
                        for job in running.values() {
                            job.process.kill();
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    unreachable!("tx lives in this frame")
                }
            }
        }
    }

    /// Starts the first step of `lane` with its share of the budget.
    fn start(
        &mut self,
        mut lane: Lane<'p>,
        running: &mut HashMap<String, Job<'p>>,
        queue: &VecDeque<Lane<'p>>,
        jobs: usize,
        tx: &mpsc::Sender<(String, Msg)>,
    ) -> Result<()> {
        let step = lane.pop_front().expect("lanes are not empty");
        let (key, world) = (step.key(), step.world());
        // Rebalanced at every start: the budget is split by the jobs that
        // can run together from now on, so the last ones get all of it.
        let worlds: Vec<String> = running
            .values()
            .map(|j| j.world.clone())
            .chain(queue.iter().map(|l| l[0].world()))
            .chain([world.clone()])
            .collect();
        let share = share(
            self.cores,
            step.cpu_target(&self.project.run),
            self.ram_mb,
            slots(jobs, worlds.iter().map(String::as_str)),
        );
        let command = step
            .invocation(self.project, &self.saves, Share::default())
            .args;
        let inv = step.invocation(self.project, &self.saves, share);
        let log = self.logs.join(format!("{}.log", log_name(&key)));
        let st = self.state.selections.entry(key.clone()).or_default();
        let resumed = st.status != Status::Pending && st.runs > 0;
        let started =
            args::require(&inv, self.caps).and_then(|_| self.arnis.spawn(&inv.args, &log));
        let (process, stdout) = match started {
            Ok(p) => p,
            Err(e) => {
                st.status = Status::Failed;
                st.error = Some(format!("{e:#}"));
                self.summary.failed += 1;
                (self.on)(&key, Note::Finished(st));
                return self.state.save(self.dir);
            }
        };
        *st = SelState {
            status: Status::Running,
            command,
            runs: st.runs + 1,
            ..Default::default()
        };
        let process = Arc::new(process);
        (self.on)(
            &key,
            Note::Started {
                pid: process.id(),
                resumed,
                share,
            },
        );
        self.state.save(self.dir)?;

        let (tx, id, p) = (tx.clone(), key.clone(), Arc::clone(&process));
        let mut log = std::fs::OpenOptions::new().append(true).open(&log)?;
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                match progress::parse(&line) {
                    Some(mut e) => {
                        // Pieces are processes of their own; count them all.
                        if let (
                            Event::Done {
                                cpu_s,
                                tree_peak_mb,
                                ..
                            },
                            Some((cpu, peak)),
                        ) = (&mut e, p.tree_usage())
                        {
                            *cpu_s = Some(cpu);
                            *tree_peak_mb = Some(peak);
                        }
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
            key,
            Job {
                process,
                world,
                finished: false,
                rest: lane,
            },
        );
        Ok(())
    }

    fn event(&mut self, id: &str, e: Event, running: &mut HashMap<String, Job>) -> Result<()> {
        let st = self.state.selections.entry(id.to_string()).or_default();
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
                if let Some(j) = running.get_mut(id) {
                    j.finished = true;
                }
                true
            }
            // A bake's only measure of progress.
            Event::Transfer { percent, .. } => {
                st.progress = *percent;
                false
            }
            Event::Other => false,
        };
        (self.on)(id, Note::Event(&e));
        if save {
            self.state.save(self.dir)?;
        }
        Ok(())
    }
}

/// A step's log file name: its key, with `:` (not allowed on Windows) as `-`.
pub fn log_name(key: &str) -> String {
    key.replace(':', "-")
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
    fn budget_split_follows_the_jobs_that_can_run() {
        // 3 selections, 2 in one world, jobs = 4: only 2 run together.
        let worlds = ["A", "A", "B"];
        assert_eq!(slots(4, worlds.into_iter()), 2);
        assert_eq!(slots(1, worlds.into_iter()), 1);
        let two = share(24, 90, Some(32_000), 2);
        assert_eq!(
            two,
            Share {
                threads: Some(10),
                ram_budget_mb: Some(16_000),
                workers_auto: true
            }
        );
        // Once one has ended, the last job gets the whole budget.
        let last = share(24, 90, Some(32_000), slots(4, ["B"].into_iter()));
        assert_eq!((last.threads, last.ram_budget_mb), (Some(21), Some(32_000)));
        assert_eq!(share(2, 10, None, 8).threads, Some(1));
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

    #[test]
    fn data_steps_run_before_the_builds_that_need_them() {
        let p = Project::parse(
            r#"
format = 1
name = "Data"
output = "saves"
[defaults]
osm_pbf = "geofabrik"
osm_pbf_url = "https://download.geofabrik.de/europe/liechtenstein-latest.osm.pbf"
[[selection]]
id = "a"
bbox = [47.10, 9.50, 47.12, 9.52]
world = "W"
settings = { prewarm = true, offline = true, scale = 0.5 }
[[selection]]
id = "b"
bbox = [47.20, 9.55, 47.22, 9.58]
world = "W"
[[bake]]
id = "li"
osm_pbf = "geofabrik"
osm_pbf_url = "https://download.geofabrik.de/europe/liechtenstein-latest.osm.pbf"
"#,
        )
        .unwrap();
        let (bakes, sels) = lanes(&p);
        let keys = |l: &[Step]| l.iter().map(Step::key).collect::<Vec<_>>();
        assert_eq!(keys(&bakes), ["bake:li"]);
        let sels: Vec<_> = sels.iter().map(|l| keys(l)).collect();
        assert_eq!(sels, [vec!["prewarm:a", "a"], vec!["b"]]);
        // Bake CPU: bakes and prewarms at their own share, builds at cpu_target.
        let run = crate::project::Budget {
            bake_cpu: Some(40),
            ..Default::default()
        };
        let (b2, s2) = lanes(&p);
        let cpu: Vec<u32> = b2
            .iter()
            .chain(&s2[0])
            .map(|s| s.cpu_target(&run))
            .collect();
        assert_eq!(cpu, [40, 40, 90]);

        // A prewarm is the build's command with --prewarm, and without --offline.
        let saves = Path::new("saves");
        let inv = |s: Step| s.invocation(&p, saves, Share::default());
        let build = inv(Step::Build(&p.selections[0])).args;
        let warm = inv(Step::Prewarm(&p.selections[0])).args;
        let mut want: Vec<_> = build
            .iter()
            .filter(|a| *a != "--offline")
            .cloned()
            .collect();
        want.push("--prewarm".into());
        assert!(build.len() == want.len() && warm == want, "{warm:?}");

        // The bake covers both selections, with room for the smallest scale's pad.
        let [s, w, n, e] = p.bake_bbox(&p.bakes[0]);
        assert!(s < 47.10 - 0.002 && w < 9.50 - 0.003 && n > 47.22 + 0.002 && e > 9.58 + 0.003);
        let bake = inv(Step::Bake(&p.bakes[0])).args;
        assert!(bake.windows(2).any(|w| w == ["--osm-pbf", "geofabrik"]));
        assert!(bake
            .iter()
            .any(|a| a.ends_with("liechtenstein-latest.osm.pbf")));
        assert!(bake.iter().any(|a| a == "--prewarm"));
    }

    /// `[server] format = "blinear"`: a built world converts after the
    /// builds, and is skipped while it does not change.
    #[test]
    fn built_worlds_convert_to_blinear_once() {
        let dir = std::env::temp_dir().join(format!("meld2-qconv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        crate::convert::tiny_world(&dir.join("saves/W"));
        let mut project = Project::parse(
            "format = 1
name = \"C\"
output = \"saves\"
[server]
format = \"blinear\"
[[selection]]
id = \"a\"
bbox = [1.0, 2.0, 3.0, 4.0]
world = \"W\"
",
        )
        .unwrap();
        project.path = dir.join("c.toml");
        let state_dir = dir.join("state");
        // `a` is built already, so no Arnis runs.
        let mut st = State::default();
        let command = Step::Build(&project.selections[0])
            .invocation(&project, &project.output_dir(), Share::default())
            .args;
        st.selections.insert("a".into(), st_done(command));
        st.save(&state_dir).unwrap();
        let go = || {
            let mut keys = vec![];
            let s = run(
                &project,
                &Arnis::new("none"),
                &[],
                &state_dir,
                &|| false,
                &mut |id, n| {
                    if let Note::Finished(_) | Note::Skipped(_) = n {
                        keys.push(id.to_string());
                    }
                },
            )
            .unwrap();
            (s, keys)
        };
        let (s, keys) = go();
        assert_eq!((s.done, s.skipped), (1, 1), "{keys:?}");
        assert!(dir
            .join("saves/W [BLinear]/region/r.0.0.b_linear")
            .is_file());
        assert_eq!(
            State::load(&state_dir).unwrap().selections["convert:W"].chunks,
            Some(5)
        );
        let (s, keys) = go();
        assert_eq!((s.done, s.skipped), (0, 2), "{keys:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn st_done(command: Vec<String>) -> SelState {
        SelState {
            status: Status::Done,
            command,
            runs: 1,
            ..Default::default()
        }
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
             echo %* >> \"%~dp0calls.txt\"\r\n\
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
settings = { prewarm = true }
[[selection]]
id = "b"
bbox = [1.0, 2.0, 3.0, 4.0]
world = "W2"
"#,
        )
        .unwrap();
        project.path = dir.join("fake.toml");
        let arnis = Arnis::new(&fake);
        let caps: Vec<String> = [
            "progress-json",
            "unit-regions",
            "threads",
            "ram-budget",
            "one-world-workers",
            "prewarm",
        ]
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
                    // Meld measures the whole process tree where it can (Windows).
                    if let Note::Event(Event::Done { tree_peak_mb, .. }) = n {
                        notes.push(format!("{id} tree {}", tree_peak_mb.is_some()));
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
                done: 3,
                ..Default::default()
            }
        );
        // a's prewarm ran first, on its world, before its build.
        let calls = std::fs::read_to_string(dir.join("calls.txt")).unwrap();
        let w1: Vec<_> = calls.lines().filter(|l| l.contains(" W1 ")).collect();
        assert_eq!(w1.len(), 2, "{calls}");
        assert!(w1[0].ends_with("--prewarm ") && !w1[1].contains("--prewarm"));
        assert!(notes.contains(&format!("b tree {}", cfg!(windows))), "{notes:?}");
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
