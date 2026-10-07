//! B_Linear for One Worlds. Arnis writes Anvil in a One World and refuses
//! `--region-format blinear` there (pieces merge into existing `.mca`), so
//! Meld converts a built world afterwards into a sibling `<World> [BLinear]`
//! for Leaf 1.21.11+. The codec is region-convert's (`region-convert/` in
//! this repo, MIT, the converter Meld 1 bundles), whose B_Linear v3 output is
//! byte-identical to Arnis's own writer on the same chunks
//! (`12-BLINEAR-COMPARISON.md`). The source world is never written.
//!
//! Every region converts into `<dir>.meld-tmp/`; a sample is read back and
//! its chunk NBT compared with the source byte for byte; only then is each
//! folder swapped in. A marker records what was converted, so a later run
//! can tell its own output from a world someone played on since.

use anyhow::{bail, Context, Result};
use region_converter::formats::{encode_region_to_writer, read_region, RegionFormat};
use region_converter::writer::write_region_with_transaction;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// zstd level; Leaf's own default.
pub const LEVEL: i32 = 6;
/// Region files read back and compared after a conversion.
pub const SAMPLE: usize = 8;
const MARKER: &str = "meld-blinear.json";
/// The folders holding region files; Arnis writes `region/` only.
const DIRS: [&str; 3] = ["region", "entities", "poi"];
/// Not carried: the source's One World job state and previews, and its lock.
const SKIP: [&str; 2] = ["arnis_one_world", "session.lock"];

/// The B_Linear sibling of a world folder.
pub fn sibling(world: &Path) -> PathBuf {
    let name = world.file_name().map(|n| n.to_string_lossy().into_owned());
    world.with_file_name(format!("{} [BLinear]", name.unwrap_or_default()))
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Converted {
    /// What the source's region files were: count, bytes, newest change.
    pub source: Vec<String>,
    pub regions: usize,
    pub chunks: usize,
    /// Regions read back and compared chunk by chunk.
    pub verified: usize,
    pub mca_bytes: u64,
    pub blinear_bytes: u64,
}

fn region_files(dir: &Path, ext: &str) -> Result<Vec<PathBuf>> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd {
            let p = e?.path();
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            if name.starts_with("r.") && name.ends_with(ext) {
                out.push(p);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// The source's region files, as `(folder, file)`.
fn sources(world: &Path) -> Result<Vec<(&'static str, PathBuf)>> {
    let mut out = vec![];
    for d in DIRS {
        for f in region_files(&world.join(d), ".mca")? {
            out.push((d, f));
        }
    }
    Ok(out)
}

/// What the source's region files are now: a later conversion of an
/// unchanged world is skipped.
pub fn fingerprint(world: &Path) -> Result<Vec<String>> {
    let (mut bytes, mut newest) = (0u64, 0u64);
    let files = sources(world)?;
    for (_, f) in &files {
        let m = std::fs::metadata(f)?;
        bytes += m.len();
        let t = m.modified()?.duration_since(SystemTime::UNIX_EPOCH)?;
        newest = newest.max(t.as_secs());
    }
    Ok(vec![
        files.len().to_string(),
        bytes.to_string(),
        newest.to_string(),
    ])
}

/// What an earlier conversion into `dest` recorded.
pub fn previous(dest: &Path) -> Option<Converted> {
    serde_json::from_slice(&std::fs::read(dest.join(MARKER)).ok()?).ok()
}

/// Newest change under `dir`, the marker aside.
fn newest_change(dir: &Path) -> Result<SystemTime> {
    let mut newest = SystemTime::UNIX_EPOCH;
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        if e.file_name() == MARKER {
            continue;
        }
        let t = if e.file_type()?.is_dir() {
            newest_change(&e.path())?
        } else {
            e.metadata()?.modified()?
        };
        newest = newest.max(t);
    }
    Ok(newest)
}

/// Fails when `dest` is not Meld's own output, or changed since Meld wrote it
/// (a server played on it, say), unless `force`.
fn check_dest(dest: &Path, force: bool) -> Result<()> {
    if force || !dest.exists() {
        return Ok(());
    }
    let marker = dest.join(MARKER);
    let Ok(written) = std::fs::metadata(&marker).and_then(|m| m.modified()) else {
        bail!(
            "{} exists and Meld did not convert it; move it away, or pass --force to replace its region files",
            dest.display()
        );
    };
    if newest_change(dest)? > written + Duration::from_secs(2) {
        bail!(
            "{} changed since Meld converted it (a server played on it?); back it up, then convert with --force",
            dest.display()
        );
    }
    Ok(())
}

/// Copies the world's other files (level.dat, data/, datapacks/, the
/// manifest, ...) into `dest`, leaving the region folders out.
fn copy_skeleton(from: &Path, to: &Path, top: bool) -> Result<u64> {
    std::fs::create_dir_all(to)?;
    let mut bytes = 0;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let name = e.file_name();
        let n = name.to_string_lossy();
        if top && (SKIP.contains(&n.as_ref()) || DIRS.contains(&n.as_ref())) {
            continue;
        }
        if e.file_type()?.is_dir() {
            bytes += copy_skeleton(&e.path(), &to.join(&name), false)?;
        } else {
            bytes += std::fs::copy(e.path(), to.join(&name))
                .with_context(|| format!("copying {}", e.path().display()))?;
        }
    }
    Ok(bytes)
}

/// Every chunk of `mca` is in `blinear` with the same NBT bytes, and nothing else is.
fn same_chunks(mca: &Path, blinear: &Path) -> Result<usize> {
    let a = read_region(mca, RegionFormat::Mca)?;
    let b = read_region(blinear, RegionFormat::BlinearV3)?;
    if !b.diagnostics.is_empty() || b.discarded_chunks > 0 {
        bail!(
            "{}: {} chunk(s) do not read back: {:?}",
            blinear.display(),
            b.discarded_chunks,
            b.diagnostics.first().map(|d| &d.message)
        );
    }
    let (ca, cb): (Vec<_>, Vec<_>) = (
        a.region.iter_chunks().collect(),
        b.region.iter_chunks().collect(),
    );
    if ca.len() != cb.len() {
        bail!(
            "{}: {} chunks, the source has {}",
            blinear.display(),
            cb.len(),
            ca.len()
        );
    }
    for ((ia, x), (ib, y)) in ca.iter().zip(&cb) {
        if ia != ib || x.raw_nbt != y.raw_nbt {
            bail!("{}: chunk {ia} differs from the source", blinear.display());
        }
    }
    Ok(ca.len())
}

/// Converts `world` into `dest` with `threads` workers. `progress(done, of)`
/// is called as regions finish; `stop()` ends it early, leaving `dest` as it was.
pub fn convert(
    world: &Path,
    dest: &Path,
    threads: usize,
    force: bool,
    stop: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Converted> {
    if !world.join("level.dat").is_file() {
        bail!("{} is not a built world (no level.dat)", world.display());
    }
    check_dest(dest, force)?;
    let files = sources(world)?;
    let mca_bytes: u64 = files
        .iter()
        .map(|(_, f)| std::fs::metadata(f).map_or(0, |m| m.len()))
        .sum();
    // B_Linear is about a quarter of Anvil (12-BLINEAR-COMPARISON §2); half is the margin.
    let parent = dest.parent().unwrap_or(Path::new("."));
    let need_mb = mca_bytes / 2 / (1 << 20) + 64;
    let free = crate::plan::free_mb(parent)?;
    if free < need_mb {
        bail!(
            "not enough disk for B_Linear: ~{need_mb} MB needed, {free} MB free on {}",
            parent.display()
        );
    }
    if stop() {
        bail!("stopped before the conversion started");
    }
    let source = fingerprint(world)?;
    let tmp = |d: &str| dest.join(format!("{d}.meld-tmp"));
    for d in DIRS {
        let _ = std::fs::remove_dir_all(tmp(d));
    }

    let (next, done, chunks) = (
        AtomicUsize::new(0),
        AtomicUsize::new(0),
        AtomicUsize::new(0),
    );
    let (halt, failed) = (AtomicBool::new(false), Mutex::new(None::<anyhow::Error>));
    let out_file = |(d, f): &(&str, PathBuf)| {
        let name = f.file_name().unwrap_or_default().to_string_lossy();
        tmp(d).join(name.replace(".mca", ".b_linear"))
    };
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| {
                while !halt.load(Ordering::Relaxed) {
                    let Some(job) = files.get(next.fetch_add(1, Ordering::Relaxed)) else {
                        return;
                    };
                    let one = || -> Result<usize> {
                        // Arnis can leave a region file without chunks: nothing to carry.
                        if std::fs::metadata(&job.1)?.len() < 8192 {
                            return Ok(0);
                        }
                        let read = read_region(&job.1, RegionFormat::Mca)?;
                        if read.discarded_chunks > 0 {
                            bail!(
                                "{} chunk(s) of the source do not read: {:?}",
                                read.discarded_chunks,
                                read.diagnostics.first().map(|d| &d.message)
                            );
                        }
                        write_region_with_transaction(
                            RegionFormat::BlinearV3,
                            &out_file(job),
                            |t| {
                                encode_region_to_writer(
                                    &read.region,
                                    RegionFormat::BlinearV3,
                                    LEVEL,
                                    t,
                                )
                                .map(drop)
                            },
                        )?;
                        Ok(read.region.chunk_count())
                    };
                    match one().with_context(|| format!("converting {}", job.1.display())) {
                        Ok(n) => {
                            chunks.fetch_add(n, Ordering::Relaxed);
                            done.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => {
                            halt.store(true, Ordering::Relaxed);
                            *failed.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
                        }
                    }
                }
            });
        }
        // Progress and stop, from the calling thread.
        let mut shown = usize::MAX;
        loop {
            let d = done.load(Ordering::Relaxed);
            if d != shown {
                progress(d, files.len());
                shown = d;
            }
            if halt.load(Ordering::Relaxed) || d == files.len() {
                break;
            }
            if stop() {
                halt.store(true, Ordering::Relaxed);
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    });
    let cleanup = || {
        DIRS.iter()
            .for_each(|d| drop(std::fs::remove_dir_all(tmp(d))))
    };
    if let Some(e) = failed.into_inner().unwrap_or_else(|e| e.into_inner()) {
        cleanup();
        return Err(e);
    }
    if done.load(Ordering::Relaxed) < files.len() {
        cleanup();
        bail!(
            "stopped before every region was converted; {} is as it was",
            dest.display()
        );
    }

    // Read a spread of regions back before anything is swapped in.
    let written: Vec<_> = files.iter().filter(|j| out_file(j).is_file()).collect();
    let step = written.len().div_ceil(SAMPLE).max(1);
    let mut verified = 0;
    for job in written.iter().step_by(step) {
        if let Err(e) = same_chunks(&job.1, &out_file(job)) {
            cleanup();
            return Err(e.context("the B_Linear read-back differs; nothing was swapped in"));
        }
        verified += 1;
    }

    // The other files, then each region folder: the old one aside, the new
    // one in place, the old one gone.
    copy_skeleton(world, dest, true)?;
    for d in DIRS {
        let (new, live, old) = (tmp(d), dest.join(d), dest.join(format!("{d}.meld-old")));
        if !new.exists() {
            continue;
        }
        let _ = std::fs::remove_dir_all(&old);
        if live.exists() {
            std::fs::rename(&live, &old)
                .with_context(|| format!("moving {} aside", live.display()))?;
        }
        std::fs::rename(&new, &live).with_context(|| format!("swapping in {}", live.display()))?;
        let _ = std::fs::remove_dir_all(&old);
    }
    let blinear_bytes = written
        .iter()
        .map(|j| {
            std::fs::metadata(
                dest.join(j.0)
                    .join(out_file(j).file_name().unwrap_or_default()),
            )
            .map_or(0, |m| m.len())
        })
        .sum();
    let out = Converted {
        source,
        regions: written.len(),
        chunks: chunks.into_inner(),
        verified,
        mca_bytes,
        blinear_bytes,
    };
    let marker = dest.join(format!("{MARKER}.tmp"));
    std::fs::write(&marker, serde_json::to_vec_pretty(&out)?)?;
    std::fs::rename(&marker, dest.join(MARKER))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use region_converter::model::{ChunkData, Region};

    /// A tiny Anvil world converts, reads back chunk for chunk, swaps in
    /// atomically, and is refused once something else wrote to it.
    #[test]
    fn converts_verifies_and_guards_the_destination() {
        let root = std::env::temp_dir().join(format!("meld2-convert-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let world = root.join("W");
        std::fs::create_dir_all(world.join("region")).unwrap();
        std::fs::create_dir_all(world.join("arnis_one_world/jobs")).unwrap();
        std::fs::write(world.join("level.dat"), b"level").unwrap();
        std::fs::write(world.join("arnis_one_world.json"), b"{}").unwrap();
        for (rx, n) in [(0, 3usize), (-1, 2)] {
            let mut r = Region::new(rx, 0);
            for i in 0..n {
                // A minimal NBT compound per chunk, different in each.
                let nbt = vec![10, 0, 0, 1, 0, 1, b'a', i as u8, 0];
                r.set_chunk(
                    i * 7,
                    ChunkData {
                        timestamp: 1,
                        raw_nbt: nbt,
                    },
                )
                .unwrap();
            }
            let file = world.join(format!("region/r.{rx}.0.mca"));
            write_region_with_transaction(RegionFormat::Mca, &file, |t| {
                encode_region_to_writer(&r, RegionFormat::Mca, 6, t).map(drop)
            })
            .unwrap();
        }
        let dest = sibling(&world);
        assert!(dest.ends_with("W [BLinear]"));
        let mut seen = vec![];
        let out = convert(&world, &dest, 2, false, &|| false, &mut |d, of| {
            seen.push((d, of))
        })
        .unwrap();
        assert_eq!((out.regions, out.chunks, out.verified), (2, 5, 2));
        assert_eq!(seen.last(), Some(&(2, 2)));
        assert!(dest.join("region/r.0.0.b_linear").is_file());
        assert!(dest.join("level.dat").is_file() && dest.join("arnis_one_world.json").is_file());
        assert!(!dest.join("arnis_one_world").exists() && !dest.join("region.meld-tmp").exists());
        assert_eq!(
            same_chunks(
                &world.join("region/r.0.0.mca"),
                &dest.join("region/r.0.0.b_linear")
            )
            .unwrap(),
            3
        );
        assert_eq!(
            previous(&dest).unwrap().source,
            fingerprint(&world).unwrap()
        );

        // Meld's own output converts again; once something writes to it, only --force does.
        convert(&world, &dest, 1, false, &|| false, &mut |_, _| {}).unwrap();
        std::thread::sleep(Duration::from_millis(2100));
        std::fs::write(dest.join("level.dat"), b"played").unwrap();
        let err = convert(&world, &dest, 1, false, &|| false, &mut |_, _| {}).unwrap_err();
        assert!(err.to_string().contains("changed since"), "{err}");
        // A stop leaves the destination as it was.
        let err = convert(&world, &dest, 1, true, &|| true, &mut |_, _| {}).unwrap_err();
        assert!(err.to_string().contains("stopped"), "{err}");
        assert_eq!(std::fs::read(dest.join("level.dat")).unwrap(), b"played");
        assert!(dest.join("region/r.0.0.b_linear").is_file());
        std::fs::remove_dir_all(root).unwrap();
    }
}
