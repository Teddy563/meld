//! `meld2 serve`: a JSON API and a small status page over HTTP, for a
//! headless server and the coming GUI.
//!
//! Every request needs the token, on loopback too (it also keeps other web
//! pages in the browser out): `Authorization: Bearer <token>`,
//! `X-Meld-Token: <token>`, or `?token=<token>` (for the page and
//! EventSource, which cannot set headers). The token is random per start
//! unless `MELD2_TOKEN` sets it. Plain HTTP: off the machine, use a trusted
//! LAN, an SSH tunnel or a TLS proxy.
//!
//! | Method | Path | |
//! |---|---|---|
//! | GET | `/` | the page |
//! | GET | `/assets/<name>` | its images (no token: public, fixed, built in) |
//! | GET | `/api/projects` | projects in the workspace, with their state |
//! | GET, PUT | `/api/projects/<name>` | `{name, path, toml, model, parts, state}`; PUT takes the TOML or the `model` JSON, checked first |
//! | POST | `/api/projects/<name>/run[?rebuild=a,b\|all]` | starts a run in the server |
//! | POST | `/api/projects/<name>/stop` | asks its run (here or a CLI one) to stop |
//! | GET | `/api/projects/<name>/plan` | pieces, chunks and size per selection, and the disk verdict |
//! | GET | `/api/status` | the state of every project Meld knows |
//! | POST | `/api/projects/<name>/server/setup[?force=1&accept_eula=1&download=0]` | sets its `[server]` folder up |
//! | POST | `/api/projects/<name>/server/start` | runs the server in this process; console lines as events |
//! | POST | `/api/projects/<name>/server/stop` | asks it (here or from the CLI) to save and stop |
//! | GET | `/api/projects/<name>/server[?lines=50]` | running, state, pid and the end of its log |
//! | POST | `/api/projects/<name>/server/send` | one console command (the body) |
//! | POST | `/api/projects/<name>/export?world=W` | zips a built world into `exports/` beside the project |
//! | GET | `/api/options` | the settings form: Arnis's sections, rows and defaults over Meld's keys (`core/src/settings.json`) |
//! | GET | `/api/arnis` | which Arnis Meld would use, its version and capabilities |
//! | POST | `/api/arnis/install` | downloads and verifies the pinned Arnis |
//! | GET | `/api/events` | Server-Sent Events: `{project, id, note, ...}` per note |
//!
//! A project's `arnis` and `[server] java` must be executables Meld installed
//! or found, or listed in `<data>/trusted-executables.txt` (`trust.rs`).

use anyhow::{bail, Result};
use meld_core::export;
use meld_core::install;
use meld_core::plan;
use meld_core::project::Project;
use meld_core::queue::Note;
use meld_core::server::Server as McServer;
use meld_core::state::{self, State};
use meld_core::trust;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use tiny_http::{Header, Method, Request, Response, Server};

const PAGE: &str = include_str!("index.html");
const MAX_BODY: u64 = 1 << 20;

pub struct Ctx {
    /// Holds `<name>/project.toml` for every project the API serves.
    pub workspace: PathBuf,
    pub token: String,
    pub arnis: Option<PathBuf>,
    subs: Mutex<Vec<mpsc::Sender<String>>>,
    running: Mutex<HashSet<String>>,
    servers: Mutex<HashSet<String>>,
}

impl Ctx {
    pub fn new(workspace: PathBuf, token: String, arnis: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            workspace,
            token,
            arnis,
            subs: Mutex::default(),
            running: Mutex::default(),
            servers: Mutex::default(),
        })
    }

    /// Whether a run or a Minecraft server is going in this process.
    pub fn busy(&self) -> bool {
        !lock(&self.running).is_empty() || !lock(&self.servers).is_empty()
    }

    /// Sends one event to every open stream, dropping the closed ones.
    fn publish(&self, project: &str, id: &str, mut event: Value) {
        event["project"] = project.into();
        event["id"] = id.into();
        let line = event.to_string();
        lock(&self.subs).retain(|s| s.send(line.clone()).is_ok());
    }

    fn project_file(&self, name: &str) -> PathBuf {
        self.workspace.join(name).join("project.toml")
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A token from the OS's random source: 32 hex digits, 128 bits.
pub fn new_token() -> Result<String> {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b)
        .map_err(|e| anyhow::anyhow!("reading the OS random source: {e}"))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Whether a request carries `token`, compared in constant time.
pub fn authorized(headers: &[(String, String)], query: &str, token: &str) -> bool {
    let same = |given: &str| {
        given.len() == token.len()
            && given
                .bytes()
                .zip(token.bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
    };
    let header = headers.iter().any(|(k, v)| {
        (k.eq_ignore_ascii_case("authorization") && v.strip_prefix("Bearer ").is_some_and(same))
            || (k.eq_ignore_ascii_case("x-meld-token") && same(v))
    });
    header || param(query, "token").is_some_and(|t| same(&t))
}

/// A query parameter, `%XX`-decoded.
fn param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| {
            let b = v.as_bytes();
            let mut out = Vec::with_capacity(b.len());
            let mut i = 0;
            while i < b.len() {
                let hex = b
                    .get(i + 1..i + 3)
                    .and_then(|h| std::str::from_utf8(h).ok());
                match (b[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
                    (b'%', Some(x)) => {
                        out.push(x);
                        i += 3;
                    }
                    (c, _) => {
                        out.push(c);
                        i += 1;
                    }
                }
            }
            String::from_utf8_lossy(&out).into_owned()
        })
    })
}

/// Project names in paths: a folder of the workspace, nothing else.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Serves until the process ends.
pub fn serve(server: Server, ctx: Arc<Ctx>) {
    for req in server.incoming_requests() {
        let ctx = Arc::clone(&ctx);
        std::thread::spawn(move || handle(&ctx, req));
    }
}

type Reply = (u16, Value);

fn handle(ctx: &Arc<Ctx>, mut req: Request) {
    let url = req.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .map(|h| (h.field.to_string(), h.value.to_string()))
        .collect();
    // The page's images are public and fixed: no token, so <img> can load them.
    if let Some(bytes) = path.strip_prefix("/assets/").and_then(crate::assets::get) {
        let h = [
            Header::from_bytes("Content-Type", "image/webp").unwrap(),
            Header::from_bytes("Cache-Control", "max-age=86400").unwrap(),
        ];
        let mut r = Response::from_data(bytes);
        for x in h {
            r.add_header(x);
        }
        let _ = req.respond(r);
        return;
    }
    if !authorized(&headers, query, &ctx.token) {
        let _ = req.respond(reply((401, json!({"error": "missing or wrong token"}))));
        return;
    }
    let method = req.method().clone();
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    let result = match (&method, parts.as_slice()) {
        (Method::Get, [""]) => {
            let html = Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap();
            let _ = req.respond(Response::from_string(PAGE).with_header(html));
            return;
        }
        (Method::Get, ["api", "events"]) => return events(ctx, req),
        (Method::Get, ["api", "status"]) => status(ctx),
        (Method::Get, ["api", "projects"]) => list(ctx),
        (Method::Get, ["api", "options"]) => serde_json::from_str(meld_core::args::SCHEMA)
            .map(|v| (200, v))
            .map_err(Into::into),
        (Method::Get, ["api", "arnis"]) => arnis(ctx),
        (Method::Post, ["api", "arnis", "install"]) => arnis_install(),
        (_, ["api", "projects", name, ..]) if !valid_name(name) => Ok((
            400,
            json!({"error": "project names use letters, digits, - and _"}),
        )),
        (m, ["api", "projects", name, ..])
            if *m != Method::Put && !ctx.project_file(name).is_file() =>
        {
            Ok((404, json!({"error": format!("no project {name:?}")})))
        }
        (Method::Get, ["api", "projects", name]) => read(ctx, name),
        (Method::Put, ["api", "projects", name]) => match body(&mut req) {
            Ok(b) => write(ctx, name, &b),
            Err(r) => Ok(r),
        },
        (Method::Post, ["api", "projects", name, "server", "send"]) => match body(&mut req) {
            Ok(b) => server_send(ctx, name, &b),
            Err(r) => Ok(r),
        },
        (Method::Post, ["api", "projects", name, "export"]) => export_world(ctx, name, query),
        (Method::Post, ["api", "projects", name, "run"]) => run(ctx, name, param(query, "rebuild")),
        (Method::Post, ["api", "projects", name, "stop"]) => stop(ctx, name),
        (Method::Get, ["api", "projects", name, "plan"]) => plan_of(ctx, name),
        (Method::Get, ["api", "projects", name, "server"]) => server_status(ctx, name, query),
        (Method::Post, ["api", "projects", name, "server", "setup"]) => {
            server_setup(ctx, name, query)
        }
        (Method::Post, ["api", "projects", name, "server", "start"]) => server_start(ctx, name),
        (Method::Post, ["api", "projects", name, "server", "stop"]) => server_stop(ctx, name),
        _ => Ok((404, json!({"error": "no such endpoint"}))),
    };
    let r = result.unwrap_or_else(|e| (500, json!({"error": format!("{e:#}")})));
    let _ = req.respond(reply(r));
}

/// The request body, at most 1 MB.
fn body(req: &mut Request) -> std::result::Result<String, Reply> {
    let mut body = String::new();
    match req.as_reader().take(MAX_BODY + 1).read_to_string(&mut body) {
        Ok(n) if n as u64 > MAX_BODY => Err((413, json!({"error": "over 1 MB"}))),
        Ok(_) => Ok(body),
        Err(e) => Err((400, json!({"error": format!("reading the body: {e}")}))),
    }
}

fn reply((code, body): Reply) -> Response<std::io::Cursor<Vec<u8>>> {
    let ct = Header::from_bytes("Content-Type", "application/json").unwrap();
    Response::from_string(body.to_string())
        .with_status_code(code)
        .with_header(ct)
}

/// SSE: one `data:` line per event, a comment every 15 s to keep proxies open.
fn events(ctx: &Ctx, req: Request) {
    let (tx, rx) = mpsc::channel();
    lock(&ctx.subs).push(tx);
    let mut w = req.into_writer();
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n: meld2\n\n";
    if w.write_all(head.as_bytes())
        .and_then(|_| w.flush())
        .is_err()
    {
        return;
    }
    loop {
        let chunk = match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(line) => format!("data: {line}\n\n"),
            Err(mpsc::RecvTimeoutError::Timeout) => ": ping\n\n".into(),
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        if w.write_all(chunk.as_bytes())
            .and_then(|_| w.flush())
            .is_err()
        {
            return;
        }
    }
}

fn names(ctx: &Ctx) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(&ctx.workspace)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| valid_name(n) && ctx.project_file(n).is_file())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn load(ctx: &Ctx, name: &str) -> Result<Project> {
    let file = ctx.project_file(name);
    if !file.is_file() {
        bail!("no project {name:?}");
    }
    let p = Project::load(&file)?;
    trust::project(&p, &state::data_dir())?;
    Ok(p)
}

fn state_of(p: &Project) -> Value {
    State::load(&state::project_dir(p))
        .map(|s| json!(s.selections))
        .unwrap_or(Value::Null)
}

fn list(ctx: &Ctx) -> Result<Reply> {
    let running = lock(&ctx.running).clone();
    let projects: Vec<Value> = names(ctx)
        .into_iter()
        .map(|n| match load(ctx, &n) {
            Ok(p) => json!({
                "name": n, "title": p.name, "running": running.contains(&n),
                "selections": p.selections.iter().map(|s| &s.id).collect::<Vec<_>>(),
                "state": state_of(&p),
            }),
            Err(e) => json!({"name": n, "error": format!("{e:#}")}),
        })
        .collect();
    Ok((200, json!(projects)))
}

fn read(ctx: &Ctx, name: &str) -> Result<Reply> {
    let file = ctx.project_file(name);
    let Ok(toml) = std::fs::read_to_string(&file) else {
        return Ok((404, json!({"error": format!("no project {name:?}")})));
    };
    let model = toml
        .parse::<toml::Table>()
        .map_or(Value::Null, |t| json!(t));
    let (state, parts, error) = match load(ctx, name) {
        Ok(p) => {
            let parts: Vec<Value> = p
                .selections
                .iter()
                // Polygon parts, and bbox selections snapped to whole pieces.
                .filter(|s| s.part_of.is_some() || p.settings_for(s).contains_key("snap"))
                .map(|s| json!({"id": s.id, "bbox": s.bbox, "part_of": s.part_of}))
                .collect();
            (state_of(&p), json!(parts), Value::Null)
        }
        Err(e) => (Value::Null, json!([]), json!(format!("{e:#}"))),
    };
    Ok((
        200,
        json!({"name": name, "path": file, "toml": toml, "model": model, "parts": parts,
               "state": state, "error": error, "running": lock(&ctx.running).contains(name)}),
    ))
}

/// Saves a project from its TOML, or from its `model` as JSON (the forms;
/// comments are not kept). It is checked where it will live, executables
/// included, before it replaces the file.
fn write(ctx: &Ctx, name: &str, body: &str) -> Result<Reply> {
    let text = if body.trim_start().starts_with('{') {
        let model: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return Ok((400, json!({"error": format!("not JSON: {e}")}))),
        };
        match toml::to_string(&model) {
            Ok(t) => t,
            Err(e) => return Ok((400, json!({"error": format!("not a project: {e}")}))),
        }
    } else {
        body.to_string()
    };
    let file = ctx.project_file(name);
    std::fs::create_dir_all(file.parent().unwrap_or(Path::new(".")))?;
    let tmp = file.with_extension("toml.tmp");
    std::fs::write(&tmp, &text)?;
    let checked = Project::load(&tmp).and_then(|p| trust::project(&p, &state::data_dir()));
    if let Err(e) = checked {
        let _ = std::fs::remove_file(&tmp);
        return Ok((400, json!({"error": format!("{e:#}")})));
    }
    std::fs::rename(&tmp, &file)?;
    Ok((200, json!({"ok": true, "path": file})))
}

fn run(ctx: &Arc<Ctx>, name: &str, rebuild: Option<String>) -> Result<Reply> {
    let path = ctx.project_file(name);
    load(ctx, name)?;
    if !lock(&ctx.running).insert(name.to_string()) {
        return Ok((409, json!({"error": "already running in this server"})));
    }
    let rebuild: Option<Vec<String>> = rebuild.map(|r| match r.as_str() {
        "all" | "" => vec![],
        _ => r.split(',').map(String::from).collect(),
    });
    let (ctx, name) = (Arc::clone(ctx), name.to_string());
    std::thread::spawn(move || {
        let result = crate::run_project(
            &path,
            ctx.arnis.clone(),
            rebuild.as_deref(),
            &mut |line| ctx.publish(&name, "", json!({"note": "log", "line": line})),
            &mut |id, note| ctx.publish(&name, id, note_json(note)),
        );
        let end = match result {
            Ok(summary) => json!({"note": "end", "summary": summary}),
            Err(e) => json!({"note": "end", "error": format!("{e:#}")}),
        };
        lock(&ctx.running).remove(&name);
        ctx.publish(&name, "", end);
    });
    Ok((202, json!({"ok": true, "started": true})))
}

fn note_json(note: &Note) -> Value {
    match note {
        Note::Skipped(why) => json!({"note": "skipped", "why": why}),
        Note::Refused(why) => json!({"note": "refused", "why": why}),
        Note::Started {
            pid,
            resumed,
            share,
        } => json!({"note": "started", "pid": pid, "resumed": resumed, "share": share}),
        Note::Event(e) => json!({"note": "event", "event": e}),
        Note::Finished(st) => json!({"note": "finished", "state": st}),
        Note::Stopping => json!({"note": "stopping"}),
    }
}

fn stop(ctx: &Ctx, name: &str) -> Result<Reply> {
    let p = load(ctx, name)?;
    let dir = state::project_dir(&p);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("stop"), b"")?;
    Ok((202, json!({"ok": true})))
}

fn plan_of(ctx: &Ctx, name: &str) -> Result<Reply> {
    let p = load(ctx, name)?;
    let (arnis, _, _) = crate::resolve(ctx.arnis.clone(), Some(&p))?;
    let plans = plan::project(&p, &arnis)?;
    let need: f64 = plans.iter().map(|(_, pl)| pl.todo_mb()).sum();
    let free = plan::free_mb(&p.output_dir())?;
    let verdict = format!("{:?}", plan::verdict(need, free, p.run.min_free_mb)).to_lowercase();
    let selections: Vec<Value> = plans
        .iter()
        .map(|(id, pl)| {
            json!({"id": id, "pieces": pl.units.len(), "regions": pl.regions(),
                   "chunks": pl.chunks(), "todo_chunks": pl.todo_chunks(), "est_mb": pl.todo_mb()})
        })
        .collect();
    Ok((
        200,
        json!({"selections": selections, "need_mb": need, "free_mb": free,
               "min_free_mb": p.run.min_free_mb, "disk": verdict}),
    ))
}

fn server_status(ctx: &Ctx, name: &str, query: &str) -> Result<Reply> {
    let p = load(ctx, name)?;
    let lines = param(query, "lines")
        .and_then(|n| n.parse().ok())
        .unwrap_or(50);
    Ok((200, json!(McServer::of(&p)?.status(lines))))
}

fn server_setup(ctx: &Ctx, name: &str, query: &str) -> Result<Reply> {
    let p = load(ctx, name)?;
    let on = |k: &str| param(query, k).is_some_and(|v| v == "1" || v == "true");
    let download = param(query, "download").is_none_or(|v| v != "0" && v != "false");
    let mut lines = vec![];
    match McServer::of(&p)?.setup(on("force"), on("accept_eula"), download, &mut |l| {
        lines.push(l)
    }) {
        Ok(()) => Ok((200, json!({"ok": true, "lines": lines}))),
        Err(e) => Ok((400, json!({"error": format!("{e:#}"), "lines": lines}))),
    }
}

/// Starts the server in a thread of this process. An error in its first
/// two seconds (no jar, no EULA, port taken, no Java) is the reply.
fn server_start(ctx: &Arc<Ctx>, name: &str) -> Result<Reply> {
    let p = load(ctx, name)?;
    if McServer::of(&p)?.status(0).running {
        return Ok((409, json!({"error": "the server is already running"})));
    }
    let (tx, rx) = mpsc::channel();
    let (ctx, name) = (Arc::clone(ctx), name.to_string());
    lock(&ctx.servers).insert(name.clone());
    std::thread::spawn(move || {
        let result = McServer::of(&p).and_then(|srv| {
            srv.start(None, &|| false, &mut |line| {
                ctx.publish(&name, "server", json!({"note": "console", "line": line}))
            })
        });
        let end = match &result {
            Ok(code) => json!({"note": "server-exit", "code": code}),
            Err(e) => json!({"note": "server-exit", "error": format!("{e:#}")}),
        };
        lock(&ctx.servers).remove(&name);
        ctx.publish(&name, "server", end);
        let _ = tx.send(result);
    });
    match rx.recv_timeout(Duration::from_secs(2)) {
        Ok(Err(e)) => Ok((400, json!({"error": format!("{e:#}")}))),
        Ok(Ok(code)) => Ok((200, json!({"ok": true, "exited": code}))),
        Err(_) => Ok((202, json!({"ok": true, "started": true}))),
    }
}

fn server_stop(ctx: &Ctx, name: &str) -> Result<Reply> {
    let p = load(ctx, name)?;
    match McServer::of(&p)?.request_stop() {
        Ok(()) => Ok((202, json!({"ok": true}))),
        Err(e) => Ok((409, json!({"error": format!("{e:#}")}))),
    }
}

fn server_send(ctx: &Ctx, name: &str, command: &str) -> Result<Reply> {
    let p = load(ctx, name)?;
    match McServer::of(&p)?.send(command) {
        Ok(()) => Ok((202, json!({"ok": true}))),
        Err(e) => Ok((409, json!({"error": format!("{e:#}")}))),
    }
}

/// Zips one of the project's built worlds into `exports/` beside the project file.
fn export_world(ctx: &Ctx, name: &str, query: &str) -> Result<Reply> {
    let p = load(ctx, name)?;
    let Some(world) = param(query, "world").filter(|w| p.worlds().contains(w)) else {
        return Ok((
            400,
            json!({"error": "?world= must name one of the project's worlds"}),
        ));
    };
    let dir = p.path.parent().unwrap_or(Path::new(".")).join("exports");
    let src = p.output_dir().join(&world);
    let to = export::name_in(&dir, &src);
    match export::export(&src, &to, p.run.min_free_mb) {
        Ok(e) => Ok((
            200,
            json!({"ok": true, "path": to, "files": e.files, "bytes": e.bytes, "zip_bytes": e.zip_bytes}),
        )),
        Err(e) => Ok((400, json!({"error": format!("{e:#}")}))),
    }
}

/// Which Arnis a run would use (never downloads), with its version and capabilities.
fn arnis(ctx: &Ctx) -> Result<Reply> {
    let data = state::data_dir();
    let pinned = json!({"repo": install::REPO, "version": install::VERSION, "min": install::MIN_VERSION,
                        "required": install::REQUIRED_CAPS, "cached": install::cached(&data)});
    let Some(found) = install::find_here(ctx.arnis.clone(), None, &data) else {
        return Ok((200, json!({"found": false, "pinned": pinned})));
    };
    let mut out = json!({"found": true, "path": found.path, "source": found.source.to_string(), "pinned": pinned});
    match install::probe(&meld_core::arnis::Arnis::new(&found.path)) {
        Ok(pr) => {
            out["version"] = pr.version.to_string().into();
            out["caps"] = json!(pr.caps);
        }
        Err(e) => out["error"] = format!("{e:#}").into(),
    }
    Ok((200, out))
}

fn arnis_install() -> Result<Reply> {
    match install::install(&state::data_dir(), install::VERSION) {
        Ok(exe) => Ok((200, json!({"ok": true, "path": exe}))),
        Err(e) => Ok((400, json!({"error": format!("{e:#}")}))),
    }
}

fn status(ctx: &Ctx) -> Result<Reply> {
    let root = state::data_dir().join("projects");
    let mut all = vec![];
    for e in std::fs::read_dir(&root).into_iter().flatten().flatten() {
        if let Ok(s) = State::load(&e.path()) {
            all.push(json!({"name": s.name, "project": s.project, "selections": s.selections}));
        }
    }
    let running: Vec<String> = lock(&ctx.running).iter().cloned().collect();
    Ok((200, json!({"projects": all, "running_here": running})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpStream;

    #[test]
    fn token_check() {
        let t = "0123456789abcdef0123456789abcdef";
        let h = |k: &str, v: &str| vec![(k.to_string(), v.to_string())];
        assert!(authorized(
            &h("Authorization", &format!("Bearer {t}")),
            "",
            t
        ));
        assert!(authorized(&h("x-meld-token", t), "", t));
        assert!(authorized(&[], &format!("a=1&token={t}"), t));
        assert!(!authorized(&[], "", t));
        assert!(!authorized(&h("Authorization", t), "", t));
        assert!(!authorized(&h("X-Meld-Token", &t[1..]), "", t));
        assert!(!authorized(&[], &format!("token={}0", &t[..31]), t));
        assert!(!authorized(&[], "token=", t));
        assert_eq!(new_token().unwrap().len(), 32);
        assert_ne!(new_token().unwrap(), new_token().unwrap());
    }

    /// Sends one request and returns the status code and body.
    fn call(
        addr: &str,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: &str,
    ) -> (u16, String) {
        let mut s = TcpStream::connect(addr).unwrap();
        let auth = token.map_or(String::new(), |t| format!("X-Meld-Token: {t}\r\n"));
        write!(
            s,
            "{method} {path} HTTP/1.1\r\nHost: x\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut raw = vec![];
        s.read_to_end(&mut raw).unwrap();
        let out = String::from_utf8_lossy(&raw);
        let code = out[9..12].parse().unwrap();
        let body = out
            .split_once("\r\n\r\n")
            .map_or("", |(_, b)| b)
            .to_string();
        (code, body)
    }

    #[test]
    fn api_round_trip_and_events() {
        let ws = std::env::temp_dir().join(format!("meld2-serve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws);
        let server = Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap().to_string();
        let token = new_token().unwrap();
        let ctx = Ctx::new(ws.clone(), token.clone(), None);
        let c2 = Arc::clone(&ctx);
        std::thread::spawn(move || serve(server, c2));
        let t = Some(token.as_str());

        assert_eq!(call(&addr, "GET", "/api/projects", None, "").0, 401);
        assert_eq!(call(&addr, "GET", "/", None, "").0, 401);
        assert_eq!(call(&addr, "GET", "/assets/hero-m.webp", None, "").0, 200);
        assert_eq!(
            call(&addr, "GET", "/assets/..%2Fsrc%2Fserve.rs", None, "").0,
            401
        );
        assert_eq!(
            call(&addr, "GET", "/api/projects", t, ""),
            (200, "[]".into())
        );
        let toml = "format = 1\nname = \"Api\"\noutput = \"saves\"\n[[selection]]\nid = \"a\"\nbbox = [47.139, 9.52, 47.141, 9.523]\nworld = \"W\"\n";
        let (code, body) = call(&addr, "PUT", "/api/projects/api", t, toml);
        assert_eq!(code, 200, "{body}");
        let (code, body) = call(&addr, "PUT", "/api/projects/api", t, "format = 1\n");
        assert_eq!(code, 400, "{body}");
        // An executable Meld did not install or find, by TOML or by model: refused.
        let evil = toml.replace(
            "[[selection]]",
            "arnis = \"C:/Windows/System32/calc.exe\"\n[[selection]]",
        );
        let (code, body) = call(&addr, "PUT", "/api/projects/api", t, &evil);
        assert!(
            code == 400 && body.contains("trusted-executables.txt"),
            "{body}"
        );
        let model = r#"{"format":1,"name":"Api","output":"saves","server":{"java":"C:/Windows/System32/calc.exe"}}"#;
        let (code, body) = call(&addr, "PUT", "/api/projects/api", t, model);
        assert!(code == 400 && body.contains("[server] java"), "{body}");
        // The forms' JSON model becomes TOML.
        let model = r#"{"format":1,"name":"Json","output":"saves","selection":[{"id":"b","world":"W","bbox":[47.1,9.5,47.2,9.6],"settings":{"scale":1,"caves":true}}]}"#;
        assert_eq!(call(&addr, "PUT", "/api/projects/json", t, model).0, 200);
        let (_, body) = call(&addr, "GET", "/api/projects/json", t, "");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["model"]["selection"][0]["settings"]["caves"], true);
        assert!(
            v["toml"].as_str().unwrap().contains("[[selection]]"),
            "{body}"
        );
        let (code, body) = call(&addr, "GET", "/api/options", t, "");
        assert!(code == 200 && body.contains(r#""k":"caves""#), "{body}");
        assert_eq!(call(&addr, "GET", "/api/projects/..%2Fx", t, "").0, 400);
        assert_eq!(call(&addr, "POST", "/api/projects/nope/run", t, "").0, 404);
        let (code, body) = call(&addr, "GET", "/api/projects/api", t, "");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!((code, v["toml"].as_str()), (200, Some(toml)));
        let (_, body) = call(&addr, "GET", "/api/projects", t, "");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v[0]["title"], "Api");
        assert_eq!(v[0]["selections"][0], "a");
        let (code, _) = call(&addr, "GET", "/", t, "");
        assert_eq!(code, 200);

        // The event stream carries what a run publishes.
        let mut s = TcpStream::connect(&addr).unwrap();
        write!(
            s,
            "GET /api/events?token={token} HTTP/1.1\r\nHost: x\r\n\r\n"
        )
        .unwrap();
        let mut head = [0u8; 256];
        let n = s.read(&mut head).unwrap();
        assert!(String::from_utf8_lossy(&head[..n]).contains("text/event-stream"));
        std::thread::sleep(Duration::from_millis(100));
        ctx.publish("api", "a", json!({"note": "log", "line": "hello"}));
        let mut buf = vec![0u8; 512];
        let mut got = String::new();
        while !got.contains("hello") {
            let n = s.read(&mut buf).unwrap();
            assert!(n > 0, "stream closed: {got}");
            got.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
        assert!(
            got.contains(r#"data: {"id":"a","line":"hello","note":"log","project":"api"}"#),
            "{got}"
        );
        std::fs::remove_dir_all(ws).unwrap();
    }
}
