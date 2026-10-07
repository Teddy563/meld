//! Size and disk planning before a run. Arnis's `--plan-units N` dry run
//! (one JSON line, nothing written) says how a selection is cut into pieces
//! and how many chunks each holds and already has; Meld turns the chunks
//! still to build into megabytes and holds them against free disk.

use crate::args::{self, Share};
use crate::arnis::Arnis;
use crate::project::{Project, Selection};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::Path;

/// Megabytes per full Java region, as Arnis's GUI estimates them (main.js
/// `EST_MB_PER_REGION`: 984 MB over 256 regions at scale 1, 3.84 MB). The
/// GUI's other constant, 15.4 MB per km² × scale², is the same figure by
/// area; `--plan-units` already counts chunks at the selection's scale.
pub const MB_PER_REGION: f64 = 984.0 / 256.0;
const CHUNKS_PER_REGION: f64 = 1024.0;
/// Margin over the estimate before a run is refused: the plan is good to ±25 %.
pub const MARGIN: f64 = 1.25;

/// One `--plan-units` record (`"type":"plan"`, `"v":1`).
#[derive(Debug, Deserialize)]
pub struct Plan {
    pub unit_regions: u32,
    pub units: Vec<Unit>,
}

#[derive(Debug, Deserialize)]
pub struct Unit {
    pub chunks: u64,
    pub existing_chunks: u64,
    /// Block bounds: min_x, min_z, max_x, max_z.
    pub rect: [i64; 4],
}

impl Plan {
    pub fn chunks(&self) -> u64 {
        self.units.iter().map(|u| u.chunks).sum()
    }

    /// Chunks not in the world yet: what the run will write.
    pub fn todo_chunks(&self) -> u64 {
        self.units
            .iter()
            .map(|u| u.chunks.saturating_sub(u.existing_chunks))
            .sum()
    }

    /// Region files the pieces touch.
    pub fn regions(&self) -> usize {
        let mut set = BTreeSet::new();
        for u in &self.units {
            let [x0, z0, x1, z1] = u.rect.map(|b| b.div_euclid(512));
            for x in x0..=x1 {
                for z in z0..=z1 {
                    set.insert((x, z));
                }
            }
        }
        set.len()
    }

    pub fn todo_mb(&self) -> f64 {
        mb(self.todo_chunks())
    }

    /// The job's block rectangle, the union of its pieces; Arnis keeps a
    /// partial job's state in `arnis_one_world/jobs/<rect>_n<N>/`.
    pub fn job_dir(&self) -> String {
        let r = self
            .units
            .iter()
            .fold([i64::MAX, i64::MAX, i64::MIN, i64::MIN], |a, u| {
                [
                    a[0].min(u.rect[0]),
                    a[1].min(u.rect[1]),
                    a[2].max(u.rect[2]),
                    a[3].max(u.rect[3]),
                ]
            });
        format!("{}_{}_{}_{}_n{}", r[0], r[1], r[2], r[3], self.unit_regions)
    }
}

/// Estimated megabytes of Java regions for `chunks` chunks.
pub fn mb(chunks: u64) -> f64 {
    chunks as f64 / CHUNKS_PER_REGION * MB_PER_REGION
}

/// The plan record in `--plan-units` stdout (the banner comes first).
pub fn parse(stdout: &str) -> Result<Plan> {
    let line = stdout
        .lines()
        .find(|l| l.starts_with('{') && l.contains("\"type\":\"plan\""))
        .context("no plan line in the --plan-units output")?;
    serde_json::from_str(line).context("reading the --plan-units line")
}

/// Every selection's plan, in project order.
pub fn project(project: &Project, arnis: &Arnis) -> Result<Vec<(String, Plan)>> {
    project
        .selections
        .iter()
        .map(|sel| Ok((sel.id.clone(), selection(project, sel, arnis)?)))
        .collect()
}

/// One selection's plan.
pub fn selection(project: &Project, sel: &Selection, arnis: &Arnis) -> Result<Plan> {
    let settings = project.settings_for(sel);
    let mut argv = args::build(sel, &settings, &project.output_dir(), Share::default()).args;
    argv.extend([
        "--plan-units".into(),
        args::unit_regions(&settings).to_string(),
    ]);
    let out = arnis
        .output(&argv)
        .with_context(|| format!("planning selection {}", sel.id))?;
    parse(&out)
}

/// A selection's One World against its frame settings: what Generate will
/// do about a world created with other ones (`frame::check_world`).
pub fn world_check(project: &Project, sel: &Selection) -> Option<crate::frame::WorldCheck> {
    let dir = project.output_dir().join(&sel.world);
    crate::frame::check_world(&dir, &project.settings_for(sel))
        .ok()
        .flatten()
}

#[derive(Debug, PartialEq)]
pub enum Disk {
    Ok,
    /// Fits, but with less than twice the estimate to spare.
    Tight,
    /// The estimate plus margin plus the reserve does not fit.
    Short,
}

/// `need_mb` against `free_mb`, keeping `reserve_mb` free after the build.
pub fn verdict(need_mb: f64, free_mb: u64, reserve_mb: u64) -> Disk {
    let room = free_mb.saturating_sub(reserve_mb) as f64;
    if need_mb * MARGIN > room {
        Disk::Short
    } else if need_mb * 2.0 > room {
        Disk::Tight
    } else {
        Disk::Ok
    }
}

/// Free megabytes on the volume holding `path` (or its nearest existing parent).
pub fn free_mb(path: &Path) -> Result<u64> {
    space_mb(path).map(|(free, _)| free)
}

/// Free and total megabytes on the volume holding `path` (or its nearest existing parent).
pub fn space_mb(path: &Path) -> Result<(u64, u64)> {
    let dir = path
        .ancestors()
        .find(|p| p.exists())
        .unwrap_or(Path::new("."));
    space(dir)
        .map(|(f, t)| (f / (1024 * 1024), t / (1024 * 1024)))
        .with_context(|| format!("reading free space of {}", dir.display()))
}

#[cfg(windows)]
fn space(dir: &Path) -> std::io::Result<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
    let (mut free, mut total) = (0u64, 0u64);
    // SAFETY: a NUL-terminated path and live out-pointers; the last may be null.
    let ok =
        unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok((free, total))
}

#[cfg(unix)]
fn space(dir: &Path) -> std::io::Result<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes())?;
    // SAFETY: a NUL-terminated path and a zeroed struct statvfs fills in.
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut st) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        #[allow(clippy::unnecessary_cast)] // the field widths differ between Linux and macOS
        Ok((
            st.f_bavail as u64 * st.f_frsize as u64,
            st.f_blocks as u64 * st.f_frsize as u64,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--plan-units 1` of the e2e Schaan selection, recorded from the
    /// 3.4.0-beta.1 exe (banner included).
    #[test]
    fn parses_recorded_plan_units_output() {
        let out = include_str!("../tests/fixtures/plan-units.stdout");
        let p = parse(out).unwrap();
        assert_eq!(p.unit_regions, 1);
        assert_eq!(p.units.len(), 16);
        assert_eq!(p.chunks(), 4 * 1024 + 4 * 96 + 4 * 128 + 4 * 12);
        assert_eq!(p.todo_chunks(), p.chunks());
        assert_eq!(p.regions(), 16);
        assert_eq!(p.job_dir(), "-576_-560_575_559_n1");
        assert!(parse("banner only\n").is_err());
    }

    #[test]
    fn size_math_and_disk_verdict() {
        // One full region is the GUI's 3.84 MB; Schaan's 5040 chunks came to
        // 18.9 MB against 20.8 MB of region files on disk.
        assert!((mb(1024) - 3.84375).abs() < 1e-9);
        assert!((mb(5040) - 18.92).abs() < 0.01);
        let built = Plan {
            unit_regions: 1,
            units: vec![Unit {
                chunks: 1024,
                existing_chunks: 1000,
                rect: [-512, -512, -1, -1],
            }],
        };
        assert_eq!(built.todo_chunks(), 24);
        assert_eq!(built.regions(), 1);

        assert_eq!(verdict(100.0, 10_000, 1024), Disk::Ok);
        assert_eq!(verdict(100.0, 1024 + 150, 1024), Disk::Tight);
        assert_eq!(verdict(100.0, 1024 + 120, 1024), Disk::Short);
        assert_eq!(verdict(100.0, 500, 1024), Disk::Short);
    }
}
