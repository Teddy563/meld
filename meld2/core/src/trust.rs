//! Which executables a project may name when it comes over the API.
//!
//! A project's `arnis` and `[server] java` are programs Meld runs. From a
//! file the user wrote that is fine; over `meld2 serve` (and the GUI, which
//! uses the same API) it would make the token a way to run any program. So
//! there they must be an executable Meld installed or finds by itself, or one
//! the machine's owner listed in `<data>/trusted-executables.txt`, a file the
//! API never writes.
//!
//! The folders such a project writes to or copies from (`output`, `[server]
//! dir`, `[server] datapacks`) must likewise lie in the workspace the API
//! serves, or in a folder listed in `<data>/trusted-folders.txt`.

use crate::install;
use crate::project::Project;
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// The allow-list in the data dir: one executable path per line, `#` comments.
pub const FILE: &str = "trusted-executables.txt";
/// The folder allow-list: one folder per line; it and everything in it is allowed.
pub const FOLDERS: &str = "trusted-folders.txt";

fn listed(file: &Path) -> Vec<PathBuf> {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(PathBuf::from)
        .collect()
}

/// `p` absolute, with `.` and `..` resolved, and links resolved as far as
/// the folders exist (so a junction cannot lead out).
pub fn real(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in std::path::absolute(p)
        .unwrap_or_else(|_| p.to_path_buf())
        .components()
    {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            c => out.push(c),
        }
    }
    let mut rest = vec![];
    let mut base = out.as_path();
    loop {
        if let Ok(c) = std::fs::canonicalize(base) {
            return rest.iter().rev().fold(c, |acc, r| acc.join(r));
        }
        match (base.parent(), base.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                base = parent;
            }
            _ => return out,
        }
    }
}

/// Fails unless the project's folders are in `workspace` or a listed folder.
pub fn folders(p: &Project, workspace: &Path, data: &Path) -> Result<()> {
    let dir = p.path.parent().unwrap_or(Path::new("."));
    let mut paths = vec![("output", p.output_dir())];
    if let Some(s) = &p.server {
        paths.push(("[server] dir", dir.join(&s.dir)));
        paths.extend(
            s.datapacks
                .iter()
                .map(|d| ("[server] datapacks", dir.join(d))),
        );
    }
    let list = data.join(FOLDERS);
    let roots: Vec<PathBuf> = std::iter::once(workspace.to_path_buf())
        .chain(listed(&list))
        .map(|r| real(&r))
        .collect();
    for (what, path) in paths {
        if !roots.iter().any(|r| real(&path).starts_with(r)) {
            bail!(
                "{what} {} is outside the workspace {}; to allow it, add the folder to {} on this machine",
                path.display(),
                workspace.display(),
                list.display()
            );
        }
    }
    Ok(())
}

fn same(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        // A bare name like `java` is looked up on PATH; compare it as written.
        _ => a == b,
    }
}

/// Fails unless `exe` is one of `known` or listed in `<data>/trusted-executables.txt`.
pub fn check(what: &str, exe: &Path, known: &[PathBuf], data: &Path) -> Result<()> {
    let list = data.join(FILE);
    if known.iter().chain(&listed(&list)).any(|k| same(exe, k)) {
        return Ok(());
    }
    bail!(
        "{what} {} is not an executable Meld installed or found; to allow it, add its path to {} on this machine",
        exe.display(),
        list.display()
    )
}

/// The Arnis builds Meld installs or finds: the pinned download, one bundled
/// next to the running binary, and `MELD2_ARNIS`.
pub fn known_arnis(data: &Path) -> Vec<PathBuf> {
    let mut known = vec![install::cached(data)];
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        known.push(dir.join(install::EXE));
    }
    known.extend(std::env::var_os("MELD2_ARNIS").map(PathBuf::from));
    known
}

/// Checks a project's `arnis` and `[server] java` for the API.
pub fn project(p: &Project, data: &Path) -> Result<()> {
    if let Some(a) = p.arnis_path() {
        check("arnis", &a, &known_arnis(data), data)?;
    }
    if let Some(j) = p.server.as_ref().and_then(|s| s.java.as_ref()) {
        check("[server] java", j, &crate::server::java_candidates(), data)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_installed_found_or_listed_executables() {
        let d = std::env::temp_dir().join(format!("meld2-trust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let cached = install::cached(&d);
        std::fs::create_dir_all(cached.parent().unwrap()).unwrap();
        std::fs::write(&cached, b"").unwrap();
        let other = d.join("calc.exe");
        std::fs::write(&other, b"").unwrap();
        let known = known_arnis(&d);

        // The pinned download, also through `..`.
        check("arnis", &cached, &known, &d).unwrap();
        let dotted = cached
            .parent()
            .unwrap()
            .join("..")
            .join(install::VERSION)
            .join(install::EXE);
        check("arnis", &dotted, &known, &d).unwrap();
        // Anything else, until the owner lists it.
        let e = check("arnis", &other, &known, &d).unwrap_err().to_string();
        assert!(e.contains(FILE), "{e}");
        assert!(check("arnis", Path::new("nope.exe"), &known, &d).is_err());
        std::fs::write(d.join(FILE), format!("# mine\n{}\n", other.display())).unwrap();
        check("arnis", &other, &known, &d).unwrap();

        // A project naming it, as the API sees it.
        let file = d.join("p/project.toml");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            "format = 1\nname = \"p\"\noutput = \"s\"\narnis = \"../evil.exe\"\n",
        )
        .unwrap();
        let p = Project::load(&file).unwrap();
        assert!(project(&p, &d).is_err());
        std::fs::write(
            &file,
            "format = 1\nname = \"p\"\noutput = \"s\"\n[server]\njava = \"C:/x/java.exe\"\n",
        )
        .unwrap();
        let p = Project::load(&file).unwrap();
        assert!(project(&p, &d)
            .unwrap_err()
            .to_string()
            .contains("[server] java"));

        // Folders: inside the workspace, or listed.
        let ws = d.join("ws");
        let put = |text: &str| {
            let f = ws.join("p/project.toml");
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(&f, format!("format = 1\nname = \"p\"\n{text}")).unwrap();
            folders(&Project::load(&f).unwrap(), &ws, &d)
        };
        put("output = \"saves\"\n[server]\ndir = \"../srv\"\n").unwrap();
        let e = put("output = \"../../out\"\n").unwrap_err().to_string();
        assert!(
            e.contains("outside the workspace") && e.contains(FOLDERS),
            "{e}"
        );
        let e = put("output = \"s\"\n[server]\ndatapacks = [\"../../../x.zip\"]\n");
        assert!(e.unwrap_err().to_string().contains("datapacks"));
        std::fs::write(d.join(FOLDERS), format!("{}\n", d.join("out").display())).unwrap();
        put("output = \"../../out/w\"\n").unwrap();
        std::fs::remove_dir_all(d).unwrap();
    }
}
