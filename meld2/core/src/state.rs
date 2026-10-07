//! Resume state: what each selection of a project has done, kept on disk in
//! Meld's data folder so a run killed or stopped picks up where it was.
//! Arnis keeps the per-piece truth inside the world; this keeps the per-selection one.

use crate::project::Project;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Pending,
    Running,
    /// Stopped on request; resumes on the next run.
    Stopped,
    Failed,
    Done,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SelState {
    pub status: Status,
    /// The command line the selection was started with, minus the run's budget.
    /// A partial job is only resumed with the same one.
    pub command: Vec<String>,
    pub pieces: u32,
    pub pieces_done: u32,
    pub progress: f64,
    pub runs: u32,
    pub error: Option<String>,
    /// Wall time and chunks of the run that finished it: a resumed job counts
    /// only the pieces it built itself.
    pub wall_s: Option<f64>,
    pub chunks: Option<u64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub project: PathBuf,
    pub name: String,
    pub selections: BTreeMap<String, SelState>,
}

/// Meld's writable folder: `MELD2_HOME`, else the OS per-user data folder.
pub fn data_dir() -> PathBuf {
    let env = |k| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(home) = env("MELD2_HOME") {
        return home;
    }
    if cfg!(windows) {
        if let Some(d) = env("LOCALAPPDATA") {
            return d.join("Meld2");
        }
    }
    if let Some(d) = env("XDG_DATA_HOME") {
        return d.join("meld2");
    }
    env("HOME")
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/share/meld2")
}

/// The folder a project's state and logs live in: its name plus a hash of
/// its path, so two projects with one name do not share state.
pub fn project_dir(project: &Project) -> PathBuf {
    let path = std::fs::canonicalize(&project.path).unwrap_or_else(|_| project.path.clone());
    // FNV-1a: stable across Rust versions, unlike std's hasher.
    let hash = path
        .to_string_lossy()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
        });
    let slug: String = project
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    data_dir()
        .join("projects")
        .join(format!("{slug}-{:08x}", hash as u32))
}

/// Holds `run.lock` in a project's state folder for as long as the file
/// lives, so one project has one run at a time. The OS drops the lock when
/// the process ends, however it ends.
pub fn lock(dir: &Path) -> Result<std::fs::File> {
    std::fs::create_dir_all(dir)?;
    let file = std::fs::File::create(dir.join("run.lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => anyhow::bail!(
            "this project is already running in another meld2; `meld2 stop` it or wait"
        ),
        Err(std::fs::TryLockError::Error(e)) => Err(e).context("locking run.lock"),
    }
}

impl State {
    pub fn load(dir: &Path) -> Result<Self> {
        let file = dir.join("state.json");
        match std::fs::read_to_string(&file) {
            Ok(text) => {
                serde_json::from_str(&text).with_context(|| format!("reading {}", file.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", file.display())),
        }
    }

    /// Written to a temporary file and renamed over, so a crash never leaves half a file.
    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join("state.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, dir.join("state.json"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_run_is_refused() {
        let dir = std::env::temp_dir().join(format!("meld2-lock-{}", std::process::id()));
        let first = lock(&dir).unwrap();
        assert!(lock(&dir)
            .unwrap_err()
            .to_string()
            .contains("already running"));
        drop(first);
        drop(lock(&dir).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("meld2-state-{}", std::process::id()));
        let mut s = State::load(&dir).unwrap();
        assert!(s.selections.is_empty());
        s.selections.insert(
            "a".into(),
            SelState {
                status: Status::Running,
                command: vec!["--bbox".into(), "1,2,3,4".into()],
                pieces: 4,
                pieces_done: 2,
                runs: 1,
                ..Default::default()
            },
        );
        s.save(&dir).unwrap();
        let back = State::load(&dir).unwrap();
        assert_eq!(back.selections["a"].status, Status::Running);
        assert_eq!(back.selections["a"].pieces_done, 2);
        assert_eq!(back.selections["a"].command[1], "1,2,3,4");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
