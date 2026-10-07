//! Driving one Arnis executable: probe it, start runs, stop them with their
//! whole process tree.
//!
//! Windows: every run gets its own Job Object with KILL_ON_JOB_CLOSE. Stopping
//! terminates the job, and if Meld itself dies the kernel closes the handle and
//! kills the run. Arnis's piece processes sit in Arnis's own nested job, so
//! they go with it.
//! Unix: Arnis starts in its own process group, watched by a `sh` that
//! holds the read end of a pipe from Meld. When Meld ends, however it ends,
//! the pipe closes and the watchdog kills the group. The coordinator's death
//! closes its pieces' stdin, which they watch and exit on (each piece has a
//! group of its own). Stopping a run closes the pipe on purpose.

use anyhow::{bail, Context, Result};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::Mutex;

pub struct Arnis {
    pub path: PathBuf,
}

impl Arnis {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// A command for this Arnis. Its caches go under Meld's data dir
    /// (`<data>/cache`), unless the caller already set `ARNIS_CACHE_ROOT`.
    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.path);
        if std::env::var_os("ARNIS_CACHE_ROOT").is_none_or(|v| v.is_empty()) {
            cmd.env("ARNIS_CACHE_ROOT", crate::state::data_dir().join("cache"));
        }
        cmd
    }

    /// `arnis --version`, e.g. "arnis 3.4.0-beta.1".
    pub fn version(&self) -> Result<String> {
        let out = self.output(&["--version"])?;
        Ok(out.trim().to_string())
    }

    /// The feature names `arnis --capabilities` lists.
    pub fn capabilities(&self) -> Result<Vec<String>> {
        let out = self.output(&["--capabilities"])?;
        let line = out
            .lines()
            .find(|l| l.starts_with('['))
            .context("no capabilities line: this Arnis predates 3.4 (Arnis at Scale)")?;
        Ok(serde_json::from_str(line)?)
    }

    /// Runs Arnis to the end and returns its stdout. A failure carries the
    /// last line of stderr, where Arnis puts its `Error: ...`.
    pub fn output<S: AsRef<std::ffi::OsStr>>(&self, args: &[S]) -> Result<String> {
        let out = self
            .command()
            .args(args)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("starting {}", self.path.display()))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            bail!(
                "{} failed ({}): {}",
                self.path.display(),
                out.status,
                err.lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Starts a run. Its stderr goes to `log`; its stdout is returned for the
    /// progress reader, which should copy the lines that are not records to `log`.
    pub fn spawn(&self, args: &[String], log: &Path) -> Result<(Process, ChildStdout)> {
        let log = File::options()
            .create(true)
            .append(true)
            .open(log)
            .with_context(|| format!("opening {}", log.display()))?;
        let mut cmd = self.command();
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(log);
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let mut child = cmd
            .spawn()
            .with_context(|| format!("starting {}", self.path.display()))?;
        let stdout = child.stdout.take().expect("piped");
        let process = Process::adopt(child)?;
        Ok((process, stdout))
    }
}

/// A started run, shareable between the thread reading it and the one that may stop it.
pub struct Process {
    child: Mutex<Child>,
    #[cfg(windows)]
    job: job::Job,
    /// `sh` that kills the run's process group once its stdin closes.
    #[cfg(unix)]
    watchdog: Mutex<Child>,
}

impl Process {
    #[cfg(windows)]
    pub fn adopt(mut child: Child) -> Result<Self> {
        // ponytail: the child runs a few ms before it joins the job. Arnis starts
        // its pieces far later; a CREATE_SUSPENDED start needs the raw thread handle
        // std does not give, so add it if anything ever spawns earlier.
        match job::Job::new().and_then(|job| job.assign(&child).map(|_| job)) {
            Ok(job) => Ok(Self {
                child: Mutex::new(child),
                job,
            }),
            Err(e) => {
                let _ = child.kill();
                Err(e)
            }
        }
    }

    #[cfg(unix)]
    pub fn adopt(mut child: Child) -> Result<Self> {
        use std::os::unix::process::CommandExt;
        // $0 is the group id, the run's pid (process_group(0) above). The
        // watchdog has a group of its own, so a Ctrl+C meant for Meld leaves
        // it alive to do its job.
        let watchdog = Command::new("sh")
            .args(["-c", "cat >/dev/null; kill -KILL -- -\"$0\""])
            .arg(child.id().to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn();
        match watchdog {
            Ok(w) => Ok(Self {
                child: Mutex::new(child),
                watchdog: Mutex::new(w),
            }),
            Err(e) => {
                let _ = child.kill();
                Err(e).context("starting the sh watchdog")
            }
        }
    }

    pub fn id(&self) -> u32 {
        self.lock().id()
    }

    /// Stops the run and everything it started.
    pub fn kill(&self) {
        #[cfg(windows)]
        self.job.terminate();
        #[cfg(unix)]
        drop(lock(&self.watchdog).stdin.take());
        let _ = self.lock().kill();
    }

    /// Waits for the run to end. Call it once its stdout is closed.
    pub fn wait(&self) -> std::io::Result<ExitStatus> {
        let status = self.lock().wait();
        // The group is gone and its id may be reused: retire the watchdog
        // before it could act.
        #[cfg(unix)]
        {
            let mut w = lock(&self.watchdog);
            let _ = w.kill();
            let _ = w.wait();
        }
        status
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Child> {
        lock(&self.child)
    }
}

fn lock(m: &Mutex<Child>) -> std::sync::MutexGuard<'_, Child> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(windows)]
mod job {
    use anyhow::{bail, Result};
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// A Job Object handle, kept as an integer so it can cross threads.
    pub struct Job(pub(super) isize);

    impl Job {
        pub fn new() -> Result<Self> {
            // SAFETY: plain Win32 calls on a job this value owns; the limit struct
            // is a live local of the size passed.
            unsafe {
                let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if h.is_null() {
                    bail!("CreateJobObject: {}", std::io::Error::last_os_error());
                }
                let job = Self(h as isize);
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    bail!(
                        "SetInformationJobObject: {}",
                        std::io::Error::last_os_error()
                    );
                }
                Ok(job)
            }
        }

        pub fn assign(&self, child: &Child) -> Result<()> {
            // SAFETY: both handles are open for the duration of the call.
            let ok = unsafe { AssignProcessToJobObject(self.0 as _, child.as_raw_handle() as _) };
            if ok == 0 {
                bail!(
                    "AssignProcessToJobObject: {}",
                    std::io::Error::last_os_error()
                );
            }
            Ok(())
        }

        pub fn terminate(&self) {
            // SAFETY: the handle is open until drop.
            unsafe { TerminateJobObject(self.0 as _, 1) };
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: closed once, here. Closing kills whatever is still in the job.
            unsafe { CloseHandle(self.0 as _) };
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A process tree started through the driver dies with `kill`: here a
    /// `cmd` that starts a long `ping`, the shape of Arnis and its pieces.
    #[test]
    fn kill_takes_the_whole_tree() {
        let arnis = Arnis::new("cmd");
        let log = std::env::temp_dir().join(format!("meld2-kill-{}.log", std::process::id()));
        let args: Vec<String> = ["/C", "ping -n 30 127.0.0.1 >NUL & echo late"]
            .map(String::from)
            .to_vec();
        let (p, _stdout) = arnis.spawn(&args, &log).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            active(&p) >= 2,
            "cmd and its ping should both be in the job"
        );
        let t = Instant::now();
        p.kill();
        p.wait().unwrap();
        assert!(t.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(active(&p), 0, "a process outlived kill");
        let _ = std::fs::remove_file(log);
    }

    fn active(p: &Process) -> u32 {
        use windows_sys::Win32::System::JobObjects::{
            JobObjectBasicAccountingInformation, QueryInformationJobObject,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        };
        // SAFETY: the job handle is open while `p` lives; the struct is a live local.
        unsafe {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
            let ok = QueryInformationJobObject(
                p.job.0 as _,
                JobObjectBasicAccountingInformation,
                &mut info as *mut _ as *mut core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            );
            assert!(ok != 0);
            info.ActiveProcesses
        }
    }
}

#[cfg(all(test, unix))]
mod unix_tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn alive(pid: &str) -> bool {
        Command::new("kill")
            .args(["-0", pid])
            .status()
            .is_ok_and(|s| s.success())
    }

    /// What Meld's death does: the run is dropped without a stop or a wait,
    /// the watchdog's pipe closes, and the whole group dies with it.
    #[test]
    fn dropping_the_run_kills_its_group() {
        let dir = std::env::temp_dir().join(format!("meld2-wd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pidfile = dir.join("pid");
        let script = format!("sleep 30 & echo $! > {}; wait", pidfile.display());
        let arnis = Arnis::new("sh");
        let (p, _stdout) = arnis
            .spawn(&["-c".into(), script], &dir.join("log"))
            .unwrap();
        let t = Instant::now();
        while !pidfile.exists() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(100));
        let sleeper = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .to_string();
        assert!(alive(&sleeper));
        drop(p);
        let t = Instant::now();
        while alive(&sleeper) && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(&sleeper), "the run's group outlived Meld");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
