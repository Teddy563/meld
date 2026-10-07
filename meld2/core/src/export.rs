//! A world as a zip or a tar.zst: a backup, or a world to hand to someone.
//! The disk is checked first against the world's full size (region files
//! are already compressed, so a zip stores them), the archive is read back
//! (every entry and byte counted), and it appears only when complete.

use crate::plan::{self, Disk};
use anyhow::{bail, Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

#[derive(Debug)]
pub struct Exported {
    pub files: usize,
    pub bytes: u64,
    pub zip_bytes: u64,
}

/// The archive kinds, by their file extension.
pub const FORMATS: &[&str] = &["zip", "tar.zst"];

/// `<dir>/<World>-<unix time>.<ext>`.
pub fn name_in(dir: &Path, world: &Path, ext: &str) -> PathBuf {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let w = world
        .file_name()
        .map_or("world".into(), |n| n.to_string_lossy());
    dir.join(format!("{w}-{t}.{ext}"))
}

fn files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if e.file_type()?.is_dir() {
            files(&e.path(), out)?;
        } else if e.file_name() != "session.lock" {
            out.push(e.path());
        }
    }
    Ok(())
}

/// Packs `world` (a folder with level.dat) into `to`, a `.zip` or a
/// `.tar.zst`, keeping `reserve_mb` free.
pub fn export(world: &Path, to: &Path, reserve_mb: u64) -> Result<Exported> {
    let name = to.to_string_lossy();
    let zst = name.ends_with(".tar.zst");
    if !zst && !name.ends_with(".zip") {
        bail!("{} must end in .zip or .tar.zst", to.display());
    }
    if !world.join("level.dat").is_file() {
        bail!("{} is not a world (no level.dat)", world.display());
    }
    if to.exists() {
        bail!("{} exists", to.display());
    }
    let mut list = vec![];
    files(world, &mut list)?;
    let mut bytes = 0;
    for f in &list {
        bytes += f.metadata()?.len();
    }
    let dir = to.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let need = bytes as f64 / (1024.0 * 1024.0);
    let free = plan::free_mb(dir)?;
    if plan::verdict(need, free, reserve_mb) == Disk::Short {
        bail!(
            "not enough disk: the world is {need:.0} MB (+25 % margin), {free} MB free on {}, keeping {reserve_mb} MB free",
            dir.display()
        );
    }
    let part = PathBuf::from(format!("{name}.part"));
    let rel = |f: &Path| -> Result<String> {
        Ok(f.strip_prefix(world)?.to_string_lossy().replace('\\', "/"))
    };
    let result = (|| -> Result<u64> {
        if zst {
            let enc = zstd::Encoder::new(std::fs::File::create(&part)?, 3)?;
            let mut tar = tar::Builder::new(enc);
            for f in &list {
                tar.append_path_with_name(f, rel(f)?)
                    .with_context(|| format!("reading {}", f.display()))?;
            }
            tar.into_inner()?.finish()?.flush()?;
        } else {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&part)?);
            for f in &list {
                let packed = matches!(
                    f.extension().and_then(|x| x.to_str()),
                    Some("mca" | "b_linear" | "linear" | "zip" | "png")
                );
                let opts = SimpleFileOptions::default()
                    .large_file(true)
                    .compression_method(if packed {
                        CompressionMethod::Stored
                    } else {
                        CompressionMethod::Deflated
                    });
                zip.start_file(rel(f)?, opts)?;
                std::io::copy(&mut std::fs::File::open(f)?, &mut zip)
                    .with_context(|| format!("reading {}", f.display()))?;
            }
            zip.finish()?.flush()?;
        }
        let (n, b) = read_back(&part, zst)?;
        if (n, b) != (list.len(), bytes) {
            bail!(
                "read back {n} file(s) / {b} bytes, wrote {} / {bytes}",
                list.len()
            );
        }
        Ok(part.metadata()?.len())
    })();
    match result {
        Ok(zip_bytes) => {
            std::fs::rename(&part, to)?;
            Ok(Exported {
                files: list.len(),
                bytes,
                zip_bytes,
            })
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

/// Files and bytes in an archive, every entry read through.
fn read_back(archive: &Path, zst: bool) -> Result<(usize, u64)> {
    let (mut n, mut b) = (0, 0);
    let f = std::fs::File::open(archive)?;
    if zst {
        let mut tar = tar::Archive::new(zstd::Decoder::new(f)?);
        for e in tar.entries()? {
            b += std::io::copy(&mut e?, &mut std::io::sink())?;
            n += 1;
        }
    } else {
        let mut z = zip::ZipArchive::new(f)?;
        for i in 0..z.len() {
            b += std::io::copy(&mut z.by_index(i)?, &mut std::io::sink())?;
            n += 1;
        }
    }
    Ok((n, b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn zips_a_world_and_refuses_a_short_disk() {
        let d = std::env::temp_dir().join(format!("meld2-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let w = d.join("Vaduz");
        std::fs::create_dir_all(w.join("region")).unwrap();
        std::fs::write(w.join("level.dat"), b"nbt").unwrap();
        std::fs::write(w.join("session.lock"), b"x").unwrap();
        std::fs::write(w.join("region/r.0.0.mca"), vec![7u8; 5000]).unwrap();

        let to = d.join("out/Vaduz.zip");
        assert!(export(&w, &to, u64::MAX / 4)
            .unwrap_err()
            .to_string()
            .contains("not enough disk"));
        assert!(!to.exists());
        let e = export(&w, &to, 0).unwrap();
        assert_eq!((e.files, e.bytes), (2, 5003));
        let mut z = zip::ZipArchive::new(std::fs::File::open(&to).unwrap()).unwrap();
        let mut got = vec![];
        z.by_name("region/r.0.0.mca")
            .unwrap()
            .read_to_end(&mut got)
            .unwrap();
        assert_eq!(got, vec![7u8; 5000]);
        assert!(z.by_name("session.lock").is_err());
        assert!(export(&w, &to, 0).is_err(), "never over an existing zip");
        assert!(
            export(&d.join("out"), &d.join("x.zip"), 0).is_err(),
            "not a world"
        );
        assert!(export(&w, &d.join("x.rar"), 0).is_err());
        let tz = d.join("out/Vaduz.tar.zst");
        let e = export(&w, &tz, 0).unwrap();
        assert_eq!((e.files, e.bytes), (2, 5003));
        let mut tar =
            tar::Archive::new(zstd::Decoder::new(std::fs::File::open(&tz).unwrap()).unwrap());
        let names: Vec<String> = tar
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["level.dat", "region/r.0.0.mca"]);
        std::fs::remove_dir_all(d).unwrap();
    }
}
