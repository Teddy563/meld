//! Which Arnis Meld drives: find one, or download the pinned release, and
//! check that it is new enough before anything runs.
//!
//! Lookup order: `--arnis`, the project's `arnis`, `MELD2_ARNIS`, an `arnis`
//! next to the meld2 binary (a bundle), the download cached in the data dir,
//! and last a fresh download of the pinned release, verified by SHA-256.

use crate::arnis::Arnis;
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256, Sha512};
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The Arnis release Meld downloads. Becomes `louis-e/arnis` once upstream ships at-Scale.
pub const REPO: &str = "Teddy563/arnis";
pub const VERSION: &str = "3.4.0-beta.1";
/// The oldest Arnis Meld drives: the first with pieces, `--plan-units` and NDJSON progress.
pub const MIN_VERSION: &str = "3.4.0-beta.1";
/// What Meld itself uses on every run, whatever the project sets.
pub const REQUIRED_CAPS: &[&str] = &[
    "progress-json",
    "unit-regions",
    "one-world-workers",
    "plan-units",
    "threads",
    "ram-budget",
];

/// A release asset: its name, its SHA-256, and the file inside it for a `.tar.gz`.
pub struct Asset {
    pub name: &'static str,
    pub sha256: &'static str,
    pub member: Option<&'static str>,
}

/// The pinned assets of `VERSION`, hashed when they were pinned.
/// `arnis-linux-appimage.tar.gz` (29a02b22…dd2) is not used: the plain binary needs no FUSE.
pub fn asset() -> Result<Asset> {
    let a = |name, sha256, member| Asset {
        name,
        sha256,
        member,
    };
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => a(
            "arnis-windows.exe",
            "4218e2394707ff360b9743e44a567f6f0364bb2a1c9d76508cc2293011c68207",
            None,
        ),
        ("linux", "x86_64") => a(
            "arnis-linux.tar.gz",
            "b457f88e0efd55d05808ed9f0ecb4bd6b90f030e383e8c5208385298ebbdc058",
            Some("arnis-linux"),
        ),
        ("macos", _) => a(
            "arnis-mac-universal.tar.gz",
            "5d35ccde76c17d9803c2f51aca522410cd0f92794aeb0374bd4920d1987cdc64",
            Some("arnis-mac-universal"),
        ),
        (os, arch) => bail!(
            "Arnis {VERSION} has no build for {os}/{arch}; build Arnis and pass --arnis <path>"
        ),
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Source {
    Flag,
    Project,
    Env,
    Bundled,
    Cached,
    Downloaded,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Source::Flag => "--arnis",
            Source::Project => "the project's `arnis`",
            Source::Env => "MELD2_ARNIS",
            Source::Bundled => "bundled next to meld2",
            Source::Cached => "downloaded earlier",
            Source::Downloaded => "downloaded now",
        })
    }
}

#[derive(Debug, PartialEq)]
pub struct Found {
    pub path: PathBuf,
    pub source: Source,
}

pub const EXE: &str = if cfg!(windows) { "arnis.exe" } else { "arnis" };

/// Where the pinned release is kept.
pub fn cached(data: &Path) -> PathBuf {
    data.join("arnis").join(VERSION).join(EXE)
}

/// The lookup order, short of downloading. A path the user gave is taken as
/// is, so a wrong one fails loudly instead of falling through to another Arnis.
pub fn find(
    flag: Option<PathBuf>,
    project: Option<PathBuf>,
    env: Option<PathBuf>,
    exe_dir: Option<&Path>,
    data: &Path,
) -> Option<Found> {
    let found = |path, source| Some(Found { path, source });
    if let Some(p) = flag {
        return found(p, Source::Flag);
    }
    if let Some(p) = project {
        return found(p, Source::Project);
    }
    if let Some(p) = env {
        return found(p, Source::Env);
    }
    if let Some(p) = exe_dir.map(|d| d.join(EXE)).filter(|p| p.is_file()) {
        return found(p, Source::Bundled);
    }
    Some(cached(data))
        .filter(|p| p.is_file())
        .map(|path| Found {
            path,
            source: Source::Cached,
        })
}

/// `find` with this process's environment and binary location.
pub fn find_here(flag: Option<PathBuf>, project: Option<PathBuf>, data: &Path) -> Option<Found> {
    let env = std::env::var_os("MELD2_ARNIS")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let exe = std::env::current_exe().ok();
    find(
        flag,
        project,
        env,
        exe.as_deref().and_then(Path::parent),
        data,
    )
}

/// `find_here`, else download the pinned release.
pub fn locate(flag: Option<PathBuf>, project: Option<PathBuf>, data: &Path) -> Result<Found> {
    match find_here(flag, project, data) {
        Some(f) => Ok(f),
        None => Ok(Found {
            path: install(data, VERSION)?,
            source: Source::Downloaded,
        }),
    }
}

/// Downloads the pinned release into the data dir, verifies its hash and
/// unpacks it. Returns the executable.
pub fn install(data: &Path, version: &str) -> Result<PathBuf> {
    if version != VERSION {
        bail!("this Meld pins Arnis {VERSION} only; for {version}, pass --arnis <path>");
    }
    let asset = asset()?;
    let exe = cached(data);
    let dir = exe.parent().expect("cached() has a parent");
    std::fs::create_dir_all(dir)?;
    let url = format!(
        "https://github.com/{REPO}/releases/download/v{VERSION}/{}",
        asset.name
    );
    eprintln!("downloading {url}");
    let part = dir.join(format!("{}.part", asset.name));
    download(&url, &part, Sum::Sha256(asset.sha256))?;
    let tmp = dir.join(format!("{EXE}.tmp"));
    match asset.member {
        None => std::fs::rename(&part, &tmp)?,
        Some(member) => {
            unpack(&part, member, &tmp)?;
            std::fs::remove_file(&part)?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, &exe)?;
    eprintln!(
        "verified sha256 {}; installed {}",
        asset.sha256,
        exe.display()
    );
    Ok(exe)
}

/// The hash a download must have.
#[derive(Clone, Copy)]
pub enum Sum<'a> {
    Sha256(&'a str),
    Sha512(&'a str),
}

/// Downloads `url` to `to` through a `.part` file, kept only if its hash matches.
pub fn download(url: &str, to: &Path, sum: Sum) -> Result<()> {
    let part = to.with_extension("part");
    let mut body = ureq::get(url)
        .header("User-Agent", "Meld2")
        .call()
        .with_context(|| format!("downloading {url}"))?
        .into_body()
        .into_reader();
    std::io::copy(&mut body, &mut std::fs::File::create(&part)?)
        .with_context(|| format!("downloading {url}"))?;
    if let Err(e) = verify(&part, sum) {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    std::fs::rename(&part, to)?;
    Ok(())
}

fn digest<D: Digest>(file: &Path) -> Result<String> {
    let mut f = std::fs::File::open(file).with_context(|| format!("opening {}", file.display()))?;
    let mut hash = D::new();
    let mut buf = vec![0; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Fails unless the file has the expected hash.
pub fn verify(file: &Path, sum: Sum) -> Result<()> {
    let (name, expected, got) = match sum {
        Sum::Sha256(e) => ("sha256", e, digest::<Sha256>(file)?),
        Sum::Sha512(e) => ("sha512", e, digest::<Sha512>(file)?),
    };
    if !got.eq_ignore_ascii_case(expected) {
        bail!(
            "{} has {name} {got}, expected {expected}: the download is corrupt or not the pinned file",
            file.display()
        );
    }
    Ok(())
}

/// Copies one file out of a `.tar.gz`.
fn unpack(archive: &Path, member: &str, to: &Path) -> Result<()> {
    let gz = flate2::read::GzDecoder::new(std::fs::File::open(archive)?);
    let mut tar = tar::Archive::new(gz);
    for entry in tar.entries()? {
        let mut entry = entry?;
        if entry.path()?.file_name() == Some(member.as_ref()) {
            std::io::copy(&mut entry, &mut std::fs::File::create(to)?)?;
            return Ok(());
        }
    }
    bail!("{member} is not in {}", archive.display())
}

/// What `--version` and `--capabilities` said, once they pass `check`.
pub struct Probe {
    pub version: semver::Version,
    pub caps: Vec<String>,
}

/// Runs `--version` and `--capabilities` and checks them.
pub fn probe(arnis: &Arnis) -> Result<Probe> {
    let fix = "run `meld2 arnis install` for the pinned release, or pass --arnis <Arnis 3.4+>";
    let line = arnis
        .version()
        .with_context(|| format!("running {} --version; {fix}", arnis.path.display()))?;
    let caps = arnis
        .capabilities()
        .with_context(|| format!("{}; {fix}", arnis.path.display()))?;
    check(&line, caps).with_context(|| format!("{}; {fix}", arnis.path.display()))
}

/// Requires `MIN_VERSION` and every `REQUIRED_CAPS`.
pub fn check(version_line: &str, caps: Vec<String>) -> Result<Probe> {
    let text = version_line.split_whitespace().last().unwrap_or_default();
    let version = semver::Version::parse(text.trim_start_matches('v'))
        .with_context(|| format!("unreadable Arnis version {version_line:?}"))?;
    let min = semver::Version::parse(MIN_VERSION).expect("MIN_VERSION parses");
    if version < min {
        bail!("Arnis {version} is older than {MIN_VERSION}, the oldest Meld 2 drives");
    }
    let missing: Vec<_> = REQUIRED_CAPS
        .iter()
        .filter(|c| !caps.iter().any(|a| a == *c))
        .collect();
    if !missing.is_empty() {
        bail!("Arnis {version} lacks {missing:?}, which Meld 2 needs");
    }
    Ok(Probe { version, caps })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("meld2-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn lookup_order() {
        let root = tmp("lookup");
        let (bin, data) = (root.join("bin"), root.join("data"));
        std::fs::create_dir_all(&bin).unwrap();
        let p = |s: &str| Some(PathBuf::from(s));
        let src = |f: Option<Found>| f.map(|f| f.source);

        assert_eq!(src(find(None, None, None, Some(&bin), &data)), None);
        std::fs::create_dir_all(cached(&data).parent().unwrap()).unwrap();
        std::fs::write(cached(&data), b"").unwrap();
        assert_eq!(
            src(find(None, None, None, Some(&bin), &data)),
            Some(Source::Cached)
        );
        std::fs::write(bin.join(EXE), b"").unwrap();
        assert_eq!(
            src(find(None, None, None, Some(&bin), &data)),
            Some(Source::Bundled)
        );
        assert_eq!(
            src(find(None, None, p("e"), Some(&bin), &data)),
            Some(Source::Env)
        );
        assert_eq!(
            src(find(None, p("p"), p("e"), Some(&bin), &data)),
            Some(Source::Project)
        );
        let f = find(p("f"), p("p"), p("e"), Some(&bin), &data).unwrap();
        assert_eq!((f.path, f.source), (PathBuf::from("f"), Source::Flag));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hash_verify_rejects_a_tampered_file() {
        let d = tmp("hash");
        let f = d.join("asset");
        std::fs::write(&f, b"abc").unwrap();
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        verify(&f, Sum::Sha256(abc)).unwrap();
        std::fs::write(&f, b"abd").unwrap();
        assert!(verify(&f, Sum::Sha256(abc))
            .unwrap_err()
            .to_string()
            .contains("expected"));
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn version_and_capability_gate() {
        let all: Vec<String> = REQUIRED_CAPS.iter().map(|c| c.to_string()).collect();
        assert_eq!(
            check("arnis 3.4.0-beta.1", all.clone()).unwrap().version,
            semver::Version::parse("3.4.0-beta.1").unwrap()
        );
        check("arnis 3.4.0", all.clone()).unwrap();
        let old = check("arnis 3.3.0", all.clone()).err().unwrap();
        assert!(old.to_string().contains("older"), "{old}");
        let alpha = check("arnis 3.4.0-alpha.9", all).err().unwrap();
        assert!(alpha.to_string().contains("older"));
        let few = check("arnis 3.4.0", vec!["progress-json".into()])
            .err()
            .unwrap();
        assert!(few.to_string().contains("plan-units"), "{few}");
    }
}
