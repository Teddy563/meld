//! Driving one Arnis executable: probe it, start runs, stop them with their
//! whole process tree.
//!
//! Windows: every run gets its own Job Object with KILL_ON_JOB_CLOSE. Stopping
//! terminates the job, and if Meld itself dies the kernel closes the handle and
//! kills the run. Arnis's piece processes sit in Arnis's own nested job, so
//! they go with it.
//! Unix: killing the Arnis coordinator closes its pieces' stdin, which they
//! watch and exit on.

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
        let out = Command::new(&self.path)
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
        let mut child = Command::new(&self.path)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(log)
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
}

impl Process {
    #[cfg(windows)]
    fn adopt(mut child: Child) -> Result<Self> {
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

    #[cfg(not(windows))]
    fn adopt(child: Child) -> Result<Self> {
        Ok(Self {
            child: Mutex::new(child),
        })
    }

    pub fn id(&self) -> u32 {
        self.lock().id()
    }

    /// Stops the run and everything it started.
    pub fn kill(&self) {
        #[cfg(windows)]
        self.job.terminate();
        let _ = self.lock().kill();
    }

    /// Waits for the run to end. Call it once its stdout is closed.
    pub fn wait(&self) -> std::io::Result<ExitStatus> {
        self.lock().wait()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Child> {
        self.child.lock().unwrap_or_else(|e| e.into_inner())
    }
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
