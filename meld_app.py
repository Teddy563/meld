#!/usr/bin/env python3
"""Meld - the background app.

Double-click it and nothing opens: it goes to the tray, the server comes up behind it, and the
browser lands on the UI. Close the window and the render keeps going, because the window was
never the thing doing the work.

Layout of the process:

    main thread   the tray icon (macOS refuses to run AppKit anywhere else)
    worker thread the HTTP server + the arnis worker pool

Two entry points are built from this one file. `Meld.exe` is frozen with --noconsole, so it
has no console and no taskbar button - just the tray icon. `meld.exe` keeps its console and
prints the banner, for anyone who wants to watch it work or script it. Same code, one flag apart.

    python meld_app.py                 tray + server, UI in its own window
    python meld_app.py --browser       open the UI in the normal browser instead
    python meld_app.py --no-tray       server only, banner in the terminal
    python meld_app.py --console       open a console window too (single-file builds)
    python meld_app.py --check         report what is installed and exit

The UI is a local web app either way - the difference is only the frame around it. By default it
gets its own window (no tabs, no address bar, its own taskbar entry); MELD_UI=browser makes the
browser the default permanently, and the tray always offers both.

Quitting is the tray's Quit item. Closing the browser tab, the preview window or a console does
not stop it - none of them are doing the work, and a render that died because someone tidied
their taskbar would be the whole point of this app thrown away.
"""
from __future__ import annotations

import os
import socket
import sys
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

from src import applog, banner, childproc, paths, preview, tray  # noqa: E402
from src.appguard import new_token  # noqa: E402
from src.single_instance import SingleInstance, running_url  # noqa: E402

DEFAULT_PORT = 5630
PORT_TRIES = 20


def _port_free(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.settimeout(0.4)
        return s.connect_ex(("127.0.0.1", port)) != 0


def pick_port(preferred: int = DEFAULT_PORT) -> int:
    """First free port at or after `preferred`.

    Reached only when we already hold the single-instance lock, so a busy 5630 is somebody
    else's program, not another Meld. Refusing to start because an unrelated app took the
    port would be a bad trade for a tool people leave running all day.
    """
    for i in range(PORT_TRIES):
        if _port_free(preferred + i):
            return preferred + i
    return preferred


def _arg_value(argv: list[str], flag: str) -> str:
    """--flag=value or --flag value, whichever the caller used."""
    for i, a in enumerate(argv):
        if a == flag and i + 1 < len(argv):
            return argv[i + 1]
        if a.startswith(flag + "="):
            return a.split("=", 1)[1]
    return ""


def version() -> str:
    """Read the version out of the changelog heading; cosmetic, never fatal."""
    for name in ("CHANGELOG.md",):
        try:
            for line in (paths.resource_dir() / name).read_text(encoding="utf-8",
                                                                errors="replace").splitlines():
                line = line.strip()
                if line.startswith("## "):
                    return line[3:].split()[0].strip("[]v")
        except Exception:
            pass
    return ""


# The generator options Meld knows how to drive that arrived after the 3.1.8 fork, so
# `--arnis-caps` can say in one screen which generation a binary belongs to. Listing them
# here rather than probing every flag Meld emits keeps the report about the question people
# actually have - "will my facades/Voxy/Moon toggles do anything with this binary?".
_CAP_FLAGS_3_2 = (
    "--voxy-lod", "--body", "--overture-source", "--building-facades",
    "--building-facades-dir", "--facade-detail", "--facade-px",
    "--mapillary-facades", "--mapillary-facade-mode", "--mapillary-probe",
)


def arnis_caps() -> int:
    """`--arnis-caps`: which arnis 3.2.0 options the resolved generator advertises.

    Meld gates every new flag on the binary's own --help, so a toggle that appears to do
    nothing is almost always a binary that predates the flag. This prints that answer
    directly instead of leaving it to be inferred from a silent render.
    """
    import server  # noqa: PLC0415
    from src import arnis_cmd  # noqa: PLC0415
    applog.setup()
    exe = server.resolve_arnis_exe()
    if not exe:
        print("arnis NOT FOUND")
        return 1
    print(f"arnis       {exe}")
    ver = arnis_cmd.arnis_version(exe)
    print(f"version     {'.'.join(str(p) for p in ver) if ver else '(unreported)'}")
    have = [f for f in _CAP_FLAGS_3_2 if arnis_cmd.arnis_supports(exe, f)]
    for flag in _CAP_FLAGS_3_2:
        print(f"  {'yes' if flag in have else ' no'}  {flag}")
    print(f"\n{len(have)}/{len(_CAP_FLAGS_3_2)} of the 3.2.0 options are available.")
    return 0


def print_arnis_cmd(argv: list[str]) -> int:
    """`--print-arnis-cmd`: the exact command line one cell of the current project runs.

    The thing people reach for when a render comes out wrong and the question is whether
    Meld asked for what they ticked. It builds the real command through the real builder -
    a reimplementation here would be able to disagree with what actually runs, which is the
    one thing this command must not do.

    No token can leak: build_arnis_cmd never puts MAPILLARY_TOKEN on the command line, and
    this prints nothing but the command line.
    """
    import shlex  # noqa: PLC0415

    import server  # noqa: PLC0415
    from src import arnis_cmd  # noqa: PLC0415
    applog.setup()
    exe = server.resolve_arnis_exe()
    if not exe:
        print("arnis NOT FOUND")
        return 1
    data = server.PROJECT.load()
    settings = server.PROJECT.settings()
    sel = server.PROJECT.load_selection()
    bbox = (sel or {}).get("bbox") or {}
    if any(bbox.get(k) is None for k in ("south", "west", "north", "east")):
        print("No area selected in this project - open Meld and pick one first.")
        return 1
    out = _arg_value(argv, "--output-dir") or str(server.PROJECT.cells_dir / "cli-preview")
    cmd = arnis_cmd.build_arnis_cmd(
        exe, bbox, out, settings, data.get("origin") or {},
        data.get("elevation"), int((data.get("elevation") or {}).get("seed") or 0))
    print(" ".join(shlex.quote(a) for a in cmd))
    return 0


def check() -> int:
    """`--check`: say what is present without starting or installing anything."""
    import platform
    # A single-file build is windowed, so it has no stdout to print this report to. Borrow the
    # terminal it was launched from (or open one), and mirror to the log file either way -
    # a diagnostic command that produces nothing visible is worse than no diagnostic command.
    if paths.is_frozen():
        applog.attach_console()
        applog.setup()
        # Unpack first, then look. --check ran BEFORE the unpack step in main(), so a
        # single-file build carrying the generator inside itself reported "arnis NOT FOUND" and
        # exited 1 - a diagnostic that failed the thing it was diagnosing.
        paths.unpack_embedded_arnis()
    import server  # noqa: PLC0415 - imported late; it is the expensive import
    bi = paths.build_info()
    print(f"Meld {version() or '(unknown version)'}")
    print(f"build       {bi.get('built') or 'source'}"
          + (f"  ({bi['commit']})" if bi.get("commit") else ""))
    print(f"python      {platform.python_version()} ({'frozen' if paths.is_frozen() else 'source'})")
    for k, v in paths.describe().items():
        print(f"{k:<11} {v}")
    exe = server.resolve_arnis_exe()
    print(f"arnis       {exe or 'NOT FOUND'}")
    print(f"tray        {'available' if tray.available() else 'unavailable (pystray/Pillow missing)'}")
    print(f"preview     {preview.describe()}")
    print(f"port {DEFAULT_PORT}   {'free' if _port_free(DEFAULT_PORT) else 'in use'}")
    return 0 if exe else 1


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if "--check" in argv:
        return check()
    # Both are read-only reports about the generator, and both sit above the single-instance
    # lock for the same reason --pick-folder does: asking what the binary supports must work
    # while a render is running, which is exactly when the question comes up.
    if "--arnis-caps" in argv:
        return arnis_caps()
    if "--print-arnis-cmd" in argv:
        return print_arnis_cmd(argv)
    if "--pick-folder" in argv:
        # A folder picker, as a mode of THIS executable.
        #
        # The server used to spawn [sys.executable, "-c", <tkinter source>], which is correct from
        # source and completely wrong once frozen: sys.executable is Meld.exe, the bootloader
        # ignores -c, and what actually launched was a SECOND Meld. That copy hit the
        # single-instance lock, printed "Meld is already running: <url>?t=<token>" and exited 0 -
        # so every Browse button in the shipped exe returned that sentence as the chosen path, and
        # printed the session token into a text field. Users screenshotted it.
        #
        # Handled here, above the lock, so this mode never takes it or trips the guard.
        import tkinter as tk
        import tkinter.filedialog as fd
        applog.setup()
        title = _arg_value(argv, "--title") or "Select a folder"
        try:
            r = tk.Tk()
            r.withdraw()
            r.attributes("-topmost", True)
            picked = fd.askdirectory(title=title) or ""
            r.destroy()
        except Exception:
            picked = ""
        # Sentinel-prefixed so the caller can never mistake an unrelated line - a warning, a
        # bootloader notice, anything - for a path. Nothing without this prefix is accepted.
        sys.stdout.write(f"\nMELD_PICKED_PATH:{picked}\n")
        sys.stdout.flush()
        return 0
    if "--statusbar" in argv:
        # A second process on purpose: tkinter and the tray icon both want the main thread, and
        # macOS *requires* AppKit to have it. Separate processes is the only arrangement where
        # neither has to give way - and a crash in the HUD cannot take a running render with it.
        from src import statusbar
        from src.single_instance import read_session
        # It is launched with pythonw / a windowed exe, so sys.stdout is None and the first
        # print() would take it down before it drew anything. Same guard the main app uses.
        applog.setup()
        sess = read_session() or {}
        url = _arg_value(argv, "--url") or sess.get("url") or f"http://127.0.0.1:{DEFAULT_PORT}"
        token = _arg_value(argv, "--token") or sess.get("token") or ""
        return statusbar.main(url, token)
    want_tray = "--no-tray" not in argv and tray.available()

    # UTF-8 everywhere, for this process and every child. Without it a single accented place
    # name in arnis output decodes against the machine's code page and kills a cell.
    os.environ.setdefault("PYTHONUTF8", "1")

    # Before anything else: a windowed build has no stdout at all, so print() would raise on
    # the very first line. This gives it one, and gives the console view something to show.
    log_path = applog.setup()
    if "--console" in argv:
        applog.attach_console()

    # A single-file build carries the generator inside itself; write it out once so the OS has
    # a real file to execute. A no-op for every other layout.
    paths.unpack_embedded_arnis()

    inst = SingleInstance()
    # Started by an update hand-off: the old build is quitting right now, so wait for it to let
    # go of the lock instead of concluding it is already running and exiting. Set by the updater
    # on the process it spawns; absent for every normal launch, which takes the plain path below.
    _wait = float(os.environ.get("MELD_WAIT_FOR_LOCK") or 0)
    if not (inst.acquire_waiting(_wait) if _wait > 0 else inst.acquire()):
        # Already running. Open that copy instead of dying on a busy port - a second
        # double-click should feel like "bring it up", not like an error.
        other = running_url()
        print(f"Meld is already running: {other or 'http://127.0.0.1:5630'}")
        if other:
            preview.open_in_browser(other)
        return 0

    try:
        port = pick_port(int(os.environ.get("PORT") or DEFAULT_PORT))
        token = new_token()
        url = f"http://127.0.0.1:{port}"
        # The tray always opens URLs carrying the token, so enforcement costs the user nothing
        # here and shuts out other native programs on the machine.
        os.environ.setdefault("MELD_REQUIRE_TOKEN", "1")
        childproc.install()

        bi = paths.build_info()
        stamp = bi.get("built") or "source"
        if bi.get("commit"):
            stamp += f" ({bi['commit']})"
        banner.show(version=version(), url=url, data_dir=str(paths.data_dir()),
                    extra=[f"build: {stamp}", f"log: {log_path}"] +
                          (["tray icon active, right-click it for controls"] if want_tray
                           else ["no tray (headless), Ctrl+C to stop"]))

        import server  # noqa: PLC0415 - after the banner, so the slow import is visible as a pause

        opened = threading.Event()

        def on_ready(u: str) -> None:
            if opened.is_set():
                return
            opened.set()
            if "--no-open" in argv:
                return
            target = f"{u}/?t={token}"
            # Its own window by default; --browser (or MELD_UI=browser) puts it in a tab.
            if "--browser" in argv:
                preview.open_in_browser(target)
            else:
                preview.open_main_window(target)

        def serve() -> None:
            try:
                server.run_server(port, token=token, require_token=True, on_ready=on_ready)
            except Exception as ex:                       # noqa: BLE001
                print(f"[meld] server stopped: {type(ex).__name__}: {ex}")
            finally:
                if want_tray and t is not None and t._icon is not None:
                    try:
                        t._icon.stop()                    # server died -> take the tray with it
                    except Exception:
                        pass

        # More than one thing can decide Meld is finished - the tray Quit, run() returning,
        # KeyboardInterrupt - and they can arrive together. Idempotent so the second one is
        # silent instead of printing a second "shutting down…" over the first one's work.
        _down = {"done": False}

        def shutdown() -> None:
            if _down["done"]:
                return
            _down["done"] = True
            print("[meld] shutting down…")
            try:
                server.stop_server()
            except Exception:
                pass
            # Hooks first (the .pbf bake pool stops itself here), then every child. Anything
            # still alive after this is inside the job object, which closes with us.
            n = childproc.kill_all()
            if n:
                print(f"[meld] stopped {n} running child process(es)")

        t = tray.Tray(url, on_quit=shutdown, token=token) if want_tray else None
        worker = threading.Thread(target=serve, name="meld-server", daemon=True)
        worker.start()

        if t is not None:
            # The overlay is Meld's standing presence, not an extra you go and find: it comes up
            # with the app so that closing the big window still leaves something on screen
            # telling you the render is alive. Hiding it is remembered, though - if you hid it
            # last time, it stays hidden until you ask for it from the tray.
            from src import statusbar as _sb
            wanted = _sb.load_settings().get("visible", True)
            if "--no-statusbar" not in argv and wanted:
                threading.Timer(1.5, t.show_statusbar).start()
            t.run()             # blocks on the main thread until Quit
            shutdown()
            worker.join(timeout=10)
        else:
            try:
                while worker.is_alive():
                    time.sleep(0.5)
            except KeyboardInterrupt:
                shutdown()
                worker.join(timeout=10)
        return 0
    finally:
        childproc.kill_all()
        inst.release()
        applog.close()


if __name__ == "__main__":
    # MUST run before anything else - before the single-instance lock above all. The parallel
    # .pbf bake uses a multiprocessing spawn pool, and in a frozen build spawn re-executes
    # Meld.exe from the top. Without this call each pool worker booted as a full Meld: it found
    # the single-instance lock held, opened a browser tab at the running instance and exited -
    # so "pressing bake opened 2 Meld tabs" (one per planned worker), the pool saw its workers
    # die ("parallel pool failed ... falling back to sequential"), and every frozen bake has
    # silently run sequential since the first exe. freeze_support() recognises the spawn
    # sentinel in argv and diverts the child into worker mode instead; everywhere else it is a
    # no-op, including from source.
    import multiprocessing
    multiprocessing.freeze_support()
    raise SystemExit(main())
