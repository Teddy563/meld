"""
Build + render the end-of-run benchmark report.

Pure functions, no global state: server.py collects the raw timing while a run is live
(per-cell queued/started/ended, a per-bucket activity + CPU/RAM timeline, machine specs +
run settings), then at run end calls build_report() to assemble a dict and write_report()
to drop two files into the world folder:

  meld-report.json   the raw data (every cell, the timeline) — the full list lives here
  meld-report.html   a self-contained, themed, paginated report (opens anywhere, offline)

The HTML matches the Meld theme (dark + gold) and is laid out for both screen and PDF:
  page 1  header + summary tiles + machine & run settings
  page 2  cell timeline (Gantt + merge playback) + CPU/RAM + activity graphs
  page 3+ the per-cell table, capped so the PDF stays small (full list = the JSON)
"Save as PDF" uses the browser print dialog (no extra deps). On screen there is a merge
playback; it (and the toolbar) hide in the printed copy, the static graphs stay.
"""

from __future__ import annotations

import base64
import html as _html
import json
import time
from pathlib import Path

REPORT_JSON_NAME = "meld-report.json"
REPORT_HTML_NAME = "meld-report.html"
SCHEMA = "meld-run-report/3"
MAX_CELL_ROWS = 200   # cap the printed table (~5 pages); the full list is in the JSON

_C = {
    "bg": "#0b0a08", "panel": "#16130d", "card": "#120f0a", "line": "#2e2a20",
    "fg": "#f0e9da", "mut": "#9a9079", "acc": "#e3a417", "acc2": "#f0bf3a",
    "ok": "#86b45a", "run": "#cf9f3c", "bad": "#cf5a3e", "plan": "#6b6253",
}
_FAILED_STATUS = ("failed", "drift", "collision")
_ICON_CACHE = {"uri": None}


def fmt_dur(s) -> str:
    try:
        s = float(s)
    except (TypeError, ValueError):
        return "-"
    if s < 0:
        return "-"
    if s < 60:
        return f"{s:.0f}s"
    m, sec = divmod(int(round(s)), 60)
    if m < 60:
        return f"{m}m {sec:02d}s"
    h, m = divmod(m, 60)
    return f"{h}h {m:02d}m"


def _iso(ts) -> str:
    try:
        return time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(float(ts)))
    except (TypeError, ValueError):
        return "-"


def _icon_data_uri() -> str:
    if _ICON_CACHE["uri"] is not None:
        return _ICON_CACHE["uri"]
    uri = ""
    try:
        p = Path(__file__).resolve().parent.parent / "web" / "meld_icon.png"
        uri = "data:image/png;base64," + base64.b64encode(p.read_bytes()).decode("ascii")
    except Exception:
        uri = ""
    _ICON_CACHE["uri"] = uri
    return uri


def build_report(*, world_name: str, meld_version: str, run: dict, timing: dict,
                 timeline: list, grid: dict, prefetch_timings: dict, settings: dict,
                 actual_mb, max_workers: int, machine: dict | None = None) -> dict:
    grid = grid or {}
    timing = timing or {}
    timeline = list(timeline or [])
    machine = machine or {}
    started = run.get("started")
    ended = run.get("ended") or time.time()
    elapsed = (ended - started) if started else 0.0

    cells = []
    for ck, t in timing.items():
        cells.append({
            "cell": ck, "status": t.get("status") or grid.get(ck) or "unknown",
            "worker": t.get("worker"), "queued": t.get("queued"), "started": t.get("started"),
            "ended": t.get("ended"), "duration_s": t.get("duration"),
            "attempts": int(t.get("attempts", 1) or 1), "reason": t.get("reason") or None,
        })
    seen = {c["cell"] for c in cells}
    for ck, status in grid.items():
        if ck not in seen:
            cells.append({"cell": ck, "status": status, "worker": None, "queued": None,
                          "started": None, "ended": None, "duration_s": None,
                          "attempts": 0, "reason": None})

    merged = [c for c in cells if c["status"] == "merged"]
    failed = [c for c in cells if c["status"] in _FAILED_STATUS]
    incomplete = [c for c in cells if c["status"] not in ("merged",) + _FAILED_STATUS]
    durs = sorted(c["duration_s"] for c in merged if c.get("duration_s"))
    n = len(durs)
    workers_peak = max((b.get("active", 0) for b in timeline), default=0) or \
        (max((c["worker"] for c in cells if c.get("worker") is not None), default=-1) + 1)
    cpu_vals = [b["cpu"] for b in timeline if b.get("cpu") is not None]
    ram_vals = [b["ram"] for b in timeline if b.get("ram") is not None]
    cfg_keys = ["scale", "job_size_regions", "buildings", "max_workers", "min_threads_per_worker",
                "cpu_target_pct", "cpu_stagger_enabled", "cpu_stagger_seconds", "prefetch_enabled",
                "elevation_zoom", "bake_lighting", "fill_ground"]

    return {
        "schema": SCHEMA, "world": world_name, "meld_version": meld_version,
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "summary": {
            "started": started, "ended": ended, "elapsed_s": round(elapsed, 1),
            "total": run.get("total", len(cells)),
            "merged": len(merged), "failed": len(failed), "incomplete": len(incomplete),
            "regions": run.get("est_regions", 0), "on_disk_mb": actual_mb,
            "workers_peak": workers_peak, "workers_setting": max_workers,
            "retries": sum(max(0, (c.get("attempts") or 0) - 1) for c in cells),
            "cell_fastest_s": durs[0] if n else None, "cell_slowest_s": durs[-1] if n else None,
            "cell_median_s": durs[n // 2] if n else None,
            "cell_avg_s": round(sum(durs) / n, 1) if n else None,
            "cpu_avg": round(sum(cpu_vals) / len(cpu_vals)) if cpu_vals else None,
            "cpu_peak": max(cpu_vals) if cpu_vals else None,
            "ram_peak": max(ram_vals) if ram_vals else None,
            "cores": machine.get("cores"),
            "scale": settings.get("scale"), "cell_size": settings.get("job_size_regions"),
            "buildings": bool(settings.get("buildings")),
        },
        "machine": machine,
        "config": {k: settings.get(k) for k in cfg_keys},
        "prefetch": dict(prefetch_timings or {}),
        "cells": cells, "timeline": timeline,
    }


# ── HTML helpers ───────────────────────────────────────────────────────────────

def _e(s) -> str:
    return _html.escape(str(s)) if s is not None else ""


def _status_color(status: str) -> str:
    return {"merged": _C["ok"], "failed": _C["bad"], "drift": _C["bad"], "collision": _C["bad"],
            "running": _C["run"], "queued": _C["acc"], "planned": _C["plan"]}.get(status, _C["mut"])


def _tiles(sm: dict) -> str:
    def tile(label, value, sub=""):
        sub_html = f'<div class="tsub">{_e(sub)}</div>' if sub else ""
        return (f'<div class="tile"><div class="tval">{_e(value)}</div>'
                f'<div class="tlab">{_e(label)}</div>{sub_html}</div>')
    disk = f"{sm['on_disk_mb'] / 1024:.2f} GB" if sm.get("on_disk_mb") else "-"
    sub = []
    if sm.get("failed"):
        sub.append(f"{sm['failed']} failed")
    if sm.get("incomplete"):
        sub.append(f"{sm['incomplete']} incomplete")
    cores = sm.get("cores")
    return "".join([
        tile("total time", fmt_dur(sm.get("elapsed_s"))),
        tile("cells merged", f"{sm.get('merged', 0)} / {sm.get('total', 0)}", " · ".join(sub)),
        tile("on disk", disk, f"{sm.get('regions', 0)} regions"),
        tile("peak workers", sm.get("workers_peak", 0),
             f"set to {sm.get('workers_setting', 0)}" + (f" · {cores} threads" if cores else "")),
        tile("median cell", fmt_dur(sm.get("cell_median_s")), f"avg {fmt_dur(sm.get('cell_avg_s'))}"),
        tile("slowest cell", fmt_dur(sm.get("cell_slowest_s")),
             f"fastest {fmt_dur(sm.get('cell_fastest_s'))}"),
    ])


def _kv(rows) -> str:
    return "".join(f'<tr><td class="k">{_e(k)}</td><td class="v">{v}</td></tr>' for k, v in rows)


def _machine_block(m: dict, sm: dict) -> str:
    cores = m.get("cores")          # logical CPUs / hardware threads
    phys = m.get("cores_phys")      # physical cores
    if phys and cores and phys != cores:
        spec = f"{phys} cores &middot; {cores} threads"
    elif cores:
        spec = f"{cores} cores ({cores} threads)"
    else:
        spec = ""
    if m.get("cpu_model"):
        cpu = f"{_e(m['cpu_model'])}" + (f" <span class='dim'>({spec})</span>" if spec else "")
    else:
        cpu = _e(spec or "-")
    ram_bits = [f"{_e(m.get('ram_gb') or '?')} GB"]
    kind = " ".join(x for x in [m.get("ram_kind"), (f"@ {m['ram_speed']} MHz" if m.get("ram_speed") else None)] if x)
    if kind:
        ram_bits.append(kind)
    if m.get("ram_modules"):
        ram_bits.append(_e(m["ram_modules"]))
    drive = m.get("drive") or "-"
    dtype = m.get("drive_type")
    disk = (f"{m.get('disk_free_gb')}/{m.get('disk_total_gb')} GB free" if m.get("disk_total_gb") else "-")
    drive_v = " &middot; ".join(x for x in [_e(drive), (_e(dtype) if dtype else None), _e(disk)] if x and x != "-") or "-"
    rows = [("CPU", cpu), ("RAM", " &middot; ".join(ram_bits)), ("Save drive", drive_v)]
    if sm.get("cpu_avg") is not None:
        rows.append(("CPU during run", f"avg {_e(sm['cpu_avg'])}% &middot; peak {_e(sm.get('cpu_peak'))}%"))
    if sm.get("ram_peak") is not None:
        rows.append(("RAM peak", f"{_e(sm['ram_peak'])}%"))
    return f'<table class="kv">{_kv(rows)}</table>'


def _config_block(cfg: dict, sm: dict, prefetch: dict) -> str:
    scale = cfg.get("scale")
    workers, threads, cores = cfg.get("max_workers"), cfg.get("min_threads_per_worker"), sm.get("cores")
    if isinstance(workers, int) and isinstance(threads, int):
        prod_n = workers * threads
        wt = f"{workers} &times; {threads} = {prod_n} threads"
        if cores:
            r = prod_n / cores
            if r <= 1.001:
                wt += f' <span style="color:{_C["ok"]}">(within your {cores} threads)</span>'
            else:
                col = _C["run"] if r <= 1.5 else _C["bad"]
                wt += f' <span style="color:{col}">({r:.1f}&times; your {cores} threads)</span>'
    else:
        wt = "-"
    stag = "off" if not cfg.get("cpu_stagger_enabled") else f"{_e(cfg.get('cpu_stagger_seconds'))}s"
    pf = " · ".join(f"{k} {fmt_dur(v)}" for k, v in (prefetch or {}).items() if v) or "none"
    rows = [
        ("Scale", f"1:{_e(round(1 / scale)) if scale else '?'}"),
        ("Cell size", f"{_e(cfg.get('job_size_regions'))} regions"),
        ("Buildings", "on" if cfg.get("buildings") else "off"),
        ("Workers", _e(workers)), ("Threads / worker", _e(threads)),
        ("Workers &times; threads", wt),
        ("CPU budget", f"{_e(cfg.get('cpu_target_pct'))}%"), ("Stagger", stag),
        ("Prefetch", ("on" if cfg.get("prefetch_enabled") else "off") + f" &middot; {_e(pf)}"),
        ("Elevation zoom", _e(cfg.get("elevation_zoom"))),
        ("Lighting bake", "on" if cfg.get("bake_lighting") else "off"),
    ]
    return f'<table class="kv">{_kv(rows)}</table>'


# ── SVG charts (static, printable) ─────────────────────────────────────────────

def _poly(points) -> str:
    return " ".join(f"{x:.1f},{y:.1f}" for x, y in points)


def _dots(points, color) -> str:
    return "".join(f'<circle cx="{x:.1f}" cy="{y:.1f}" r="2.1" fill="{color}"/>' for x, y in points)


def _svg_cpu_ram(timeline: list, w: int = 640, h: int = 150) -> str:
    """Filled line + area, same look as the live left-rail sparkline: RAM green area behind, CPU
    gold line on top. Values are the per-20s averages (smooth, honest), not a spiky raw line."""
    cpu = [b.get("cpu") for b in timeline]
    ram = [b.get("ram") for b in timeline]
    if not any(v is not None for v in cpu) and not any(v is not None for v in ram):
        return '<div class="muted">No CPU/RAM samples recorded (a very short run logs none).</div>'
    pad_l, pad_b, pad_t = 28, 14, 8
    cw, ch = w - pad_l - 6, h - pad_b - pad_t
    n = max(1, len(timeline))
    base = pad_t + ch
    xs = lambda i: pad_l + (cw * (i / (n - 1)) if n > 1 else cw / 2)
    ys = lambda v: pad_t + ch - (max(0, min(100, v)) / 100) * ch

    def ser(vals, color, fill):
        pts = [(xs(i), ys(v)) for i, v in enumerate(vals) if v is not None]
        if not pts:
            return ""
        if len(pts) == 1:   # a single sample: draw it as a flat line so it's visible
            pts = [(pad_l, pts[0][1]), (pad_l + cw, pts[0][1])]
        area = f"{pts[0][0]:.1f},{base} " + _poly(pts) + f" {pts[-1][0]:.1f},{base}"
        return (f'<polygon points="{area}" fill="{fill}"/>'
                f'<polyline points="{_poly(pts)}" fill="none" stroke="{color}" stroke-width="1.8" stroke-linejoin="round"/>')
    grid = "".join(f'<line x1="{pad_l}" y1="{ys(g):.0f}" x2="{w - 6}" y2="{ys(g):.0f}" stroke="rgba(255,255,255,.06)"/>'
                   f'<text x="3" y="{ys(g) + 3:.0f}" class="ax">{g}</text>' for g in (25, 50, 75, 100))
    series = ser(ram, _C["ok"], "rgba(134,180,90,.14)") + ser(cpu, _C["acc"], "rgba(227,164,23,.18)")
    defs = f'<defs><clipPath id="clip_cr"><rect id="rev_cr" x="{pad_l}" y="0" width="{cw:.1f}" height="{h}"/></clipPath></defs>'
    sweep = (f'<line id="sweep_cr" x1="{pad_l}" y1="{pad_t}" x2="{pad_l}" y2="{base}" '
             f'stroke="{_C["acc2"]}" stroke-width="1.5" style="opacity:0"/>')
    return (f'<svg viewBox="0 0 {w} {h}" class="chart" data-px="{pad_l}" data-pw="{cw:.1f}">{defs}{grid}'
            f'<g clip-path="url(#clip_cr)">{series}</g>{sweep}</svg>'
            f'<div class="cleg"><span class="dot" style="background:{_C["acc"]}"></span>CPU %'
            f'<span class="dot" style="background:{_C["ok"]};margin-left:14px"></span>RAM %'
            f'<span class="dim" style="margin-left:auto">average per 20s</span></div>')


def _svg_activity(timeline: list, w: int = 640, h: int = 150, peak: int = 1) -> str:
    if not timeline:
        return '<div class="muted">No activity recorded.</div>'
    pad_l, pad_b, pad_t = 28, 14, 8
    cw, ch = w - pad_l - 6, h - pad_b - pad_t
    n = len(timeline)
    comp, prev = [], None
    for b in timeline:
        d = b.get("done", 0) or 0
        comp.append(0 if prev is None else max(0, d - prev)); prev = d
    maxc = max(comp) or 1
    peak = max(1, peak, max((b.get("active", 0) or 0) for b in timeline))
    bw = cw / n
    base = pad_t + ch
    bars = ""
    for i, cc in enumerate(comp):
        if cc <= 0:
            continue
        bh = (cc / maxc) * ch
        x = pad_l + i * bw
        bars += (f'<rect x="{x + 1:.1f}" y="{base - bh:.1f}" width="{max(1.4, bw - 2):.1f}" '
                 f'height="{bh:.1f}" fill="rgba(134,180,90,.6)" rx="1.5"><title>{cc} merged</title></rect>')
    # workers active as a STEP area (discrete count), gold.
    step = [(pad_l, base)]
    for i, b in enumerate(timeline):
        y = base - ((b.get("active", 0) or 0) / peak) * ch
        step.append((pad_l + i * bw, y))
        step.append((pad_l + (i + 1) * bw, y))
    step.append((pad_l + cw, base))
    line_pts = step[1:-1]
    grid = "".join(f'<line x1="{pad_l}" y1="{base - ch * g / 4:.0f}" x2="{w - 6}" y2="{base - ch * g / 4:.0f}" '
                   f'stroke="rgba(255,255,255,.05)"/>' for g in range(1, 4))
    marks = (f'{bars}<polygon points="{_poly(step)}" fill="rgba(227,164,23,.12)"/>'
             f'<polyline points="{_poly(line_pts)}" fill="none" stroke="{_C["acc"]}" stroke-width="1.8"/>')
    defs = f'<defs><clipPath id="clip_act"><rect id="rev_act" x="{pad_l}" y="0" width="{cw:.1f}" height="{h}"/></clipPath></defs>'
    sweep = (f'<line id="sweep_act" x1="{pad_l}" y1="{pad_t}" x2="{pad_l}" y2="{base}" '
             f'stroke="{_C["acc2"]}" stroke-width="1.5" style="opacity:0"/>')
    return (f'<svg viewBox="0 0 {w} {h}" class="chart" data-px="{pad_l}" data-pw="{cw:.1f}">{defs}{grid}'
            f'<g clip-path="url(#clip_act)">{marks}</g>{sweep}'
            f'<text x="3" y="{pad_t + 8}" class="ax">{peak}w</text></svg>'
            f'<div class="cleg"><span class="dot" style="background:{_C["acc"]}"></span>workers active'
            f'<span class="dot" style="background:rgba(134,180,90,.75);margin-left:14px"></span>cells merged per step</div>')


def _gantt(cells: list, started, ended, w: int = 660) -> str:
    runnable = [c for c in cells if c.get("started") and c.get("ended") and c.get("worker") is not None]
    if not runnable or not started:
        return '<div class="muted">No per-cell timing to chart.</div>'
    span = max(1e-6, (ended or time.time()) - started)
    workers = sorted({c["worker"] for c in runnable})
    lane = {wid: i for i, wid in enumerate(workers)}
    # Lane height shrinks as worker count grows, so the whole chart stays within a bounded height
    # (~260) and the graphs page keeps to one printed sheet even with 12+ workers. Up to ~9 workers
    # keep the comfortable 24px lane; past that they compress to fit.
    gut, top, bot = 30, 22, 18
    lane_h = max(8, min(24, (260 - top - bot) // max(1, len(workers))))
    bar_h = max(4, lane_h - 8)
    bar_off = (lane_h - bar_h) / 2
    cw = w - gut - 6
    height = top + len(workers) * lane_h + bot
    xt = lambda t: gut + ((t - started) / span) * cw
    stripes = "".join(
        f'<rect x="{gut}" y="{top + i * lane_h}" width="{cw}" height="{lane_h}" '
        f'fill="{"rgba(255,255,255,.02)" if i % 2 else "transparent"}"/>' for i in range(len(workers)))
    ticks = ""
    for g in range(6):
        tx = gut + cw * g / 5
        ticks += (f'<line x1="{tx:.0f}" y1="{top}" x2="{tx:.0f}" y2="{top + len(workers) * lane_h}" '
                  f'stroke="rgba(255,255,255,.05)"/>'
                  f'<text x="{tx:.0f}" y="{top - 6}" class="ax" text-anchor="middle">{fmt_dur(span * g / 5)}</text>')
    labels = "".join(f'<text x="3" y="{top + lane[w_] * lane_h + lane_h - 5:.0f}" class="ax">#{w_ + 1}</text>'
                     for w_ in workers)
    bars = ""
    for c in sorted(runnable, key=lambda c: c["started"]):
        ly = top + lane[c["worker"]] * lane_h
        x0, x1 = xt(c["started"]), xt(c["ended"])
        bw = max(1.5, x1 - x0)
        bars += (f'<rect class="gbar" data-endx="{(x0 + bw) - gut:.1f}" x="{x0:.1f}" y="{ly + bar_off:.1f}" '
                 f'width="{bw:.1f}" height="{bar_h}" rx="{min(3, bar_h / 2):.1f}" fill="{_status_color(c["status"])}">'
                 f'<title>{_e(c["cell"])} · {fmt_dur(c.get("duration_s"))} · {_e(c["status"])}</title></rect>')
    sweep = (f'<line id="gsweep" x1="{gut}" y1="{top}" x2="{gut}" y2="{top + len(workers) * lane_h}" '
             f'stroke="{_C["acc2"]}" stroke-width="1.5" style="opacity:0"/>')
    leg = (f'<div class="cleg"><span class="dot" style="background:{_C["ok"]}"></span>merged'
           f'<span class="dot" style="background:{_C["bad"]};margin-left:12px"></span>failed'
           f'<span class="dim" style="margin-left:auto">each bar = one cell on its worker, width = its time</span></div>')
    return (f'<svg id="gantt" viewBox="0 0 {w} {height}" class="gantt" data-gut="{gut}" data-cw="{cw}">'
            f'{stripes}{ticks}{labels}{bars}{sweep}</svg>{leg}')


def _cell_rows(cells: list):
    def key(c):
        bad = 0 if c["status"] in _FAILED_STATUS else (1 if c["status"] != "merged" else 2)
        return (bad, -(c.get("duration_s") or 0))
    ordered = sorted(cells, key=key)
    shown = ordered[:MAX_CELL_ROWS]
    rows = []
    for c in shown:
        col = _status_color(c["status"])
        worker = f"#{c['worker'] + 1}" if c.get("worker") is not None else "-"
        att = c.get("attempts") or 0
        att_html = f'<span class="retry">&times;{att}</span>' if att > 1 else ""
        reason = f'<span class="reason">{_e(c["reason"])}</span>' if c.get("reason") else ""
        rows.append(
            f'<tr><td class="mono">{_e(c["cell"])}</td>'
            f'<td><span class="chip" style="color:{col};border-color:{col}">{_e(c["status"])}</span>{att_html}</td>'
            f'<td class="mono">{worker}</td><td class="mono num">{fmt_dur(c.get("duration_s"))}</td>'
            f'<td>{reason}</td></tr>')
    return "".join(rows), len(shown), len(ordered)


_PLAYBACK_JS = """
(function(){
  var btn=document.getElementById('playBtn'); if(!btn) return;
  var DUR=6000;
  // clip-reveal charts (CPU/RAM + Activity): a clip rect grows left to right, a sweep line tracks it.
  var clips=[];
  ['cr','act'].forEach(function(k){
    var rev=document.getElementById('rev_'+k); if(!rev) return;
    var svg=rev.ownerSVGElement, sw=document.getElementById('sweep_'+k);
    clips.push({rev:rev, sw:sw, px:parseFloat(svg.getAttribute('data-px'))||0, pw:parseFloat(svg.getAttribute('data-pw'))||1});
  });
  // cell timeline: reveal each bar as the cursor passes its end.
  var gsvg=document.getElementById('gantt');
  var bars=gsvg?[].slice.call(gsvg.querySelectorAll('.gbar')):[];
  var gsweep=document.getElementById('gsweep');
  var gut=gsvg?(parseFloat(gsvg.getAttribute('data-gut'))||30):30;
  var gcw=gsvg?(parseFloat(gsvg.getAttribute('data-cw'))||1):1;
  if(!clips.length && !bars.length){ btn.style.display='none'; return; }
  function done(){
    clips.forEach(function(c){ c.rev.setAttribute('width',c.pw); if(c.sw) c.sw.style.opacity=0; });
    bars.forEach(function(b){ b.style.opacity=1; });
    if(gsweep) gsweep.style.opacity=0;
  }
  btn.addEventListener('click',function(){
    clips.forEach(function(c){ c.rev.setAttribute('width',0); if(c.sw) c.sw.style.opacity=1; });
    bars.forEach(function(b){ b.style.opacity=0.12; });
    if(gsweep) gsweep.style.opacity=1;
    var t0=null;
    function step(now){
      if(t0===null) t0=now;
      var f=Math.min(1,(now-t0)/DUR);
      clips.forEach(function(c){
        c.rev.setAttribute('width',(f*c.pw).toFixed(1));
        if(c.sw){ var x=c.px+f*c.pw; c.sw.setAttribute('x1',x); c.sw.setAttribute('x2',x); }
      });
      var gx=f*gcw;
      if(gsweep){ gsweep.setAttribute('x1',gut+gx); gsweep.setAttribute('x2',gut+gx); }
      bars.forEach(function(b){ if(parseFloat(b.getAttribute('data-endx'))<=gx) b.style.opacity=1; });
      if(f<1) requestAnimationFrame(step); else done();
    }
    requestAnimationFrame(step);
  });
})();
"""


def render_html(report: dict) -> str:
    sm = report.get("summary", {})
    c = _C
    icon = _icon_data_uri()
    icon_html = f'<img src="{icon}" alt="Meld" class="logo">' if icon else ""
    pf = report.get("prefetch", {})
    pf_line = ("Prefetch: " + " · ".join(f"{k} {fmt_dur(v)}" for k, v in pf.items() if v)) if pf else ""
    rows_html, shown, total = _cell_rows(report.get("cells", []))
    more = total - shown
    cells_head = (f"Cells (showing {shown} of {total})" if more > 0
                  else f"Cells ({total})")
    more_btn = (f'<div class="bar noprint"><a class="btn ghost" href="/api/report.json" target="_blank">'
                f'Open full list ({total} cells) as JSON &rarr;</a></div>' if more > 0 else "")
    more_note = (f'<div class="dim" style="margin:4px 0 8px">Showing the {shown} most relevant '
                 f'(failures first, then slowest); the full {total} are in {REPORT_JSON_NAME}.</div>' if more > 0 else "")
    peak = sm.get("workers_peak", 1) or 1
    return f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Benchmark · {_e(report.get('world', 'World'))}</title>
<style>
  :root {{ color-scheme: dark; }}
  * {{ box-sizing: border-box; }}
  body {{ margin:0; background:{c['bg']}; color:{c['fg']};
    font-family: ui-sans-serif, system-ui, 'Segoe UI', sans-serif; line-height:1.5;
    -webkit-print-color-adjust:exact; print-color-adjust:exact; }}
  .wrap {{ max-width: 1000px; margin: 0 auto; padding: 26px 20px 50px; }}
  .page {{ margin-bottom: 26px; }}
  header {{ display:flex; align-items:flex-start; justify-content:space-between; gap:16px; }}
  .eyebrow {{ font-family: ui-monospace, Consolas, monospace; font-size:11px; letter-spacing:.18em;
    text-transform:uppercase; color:{c['acc']}; }}
  h1 {{ margin:.2rem 0 .1rem; font-size: 28px; }}
  .sub {{ color:{c['mut']}; font-size:13px; }}
  .logo {{ height:56px; width:auto; opacity:.95; }}
  .bar {{ margin:16px 0 4px; display:flex; gap:8px; flex-wrap:wrap; align-items:center; }}
  .btn {{ background:{c['acc']}; color:#241a02; border:2px solid #9a6a12; font-weight:700;
    font-family:ui-monospace,Consolas,monospace; font-size:12px; padding:7px 14px; cursor:pointer;
    border-radius:2px; text-decoration:none; display:inline-block; }}
  .btn.ghost {{ background:transparent; color:{c['acc2']}; border-color:{c['line']}; }}
  .btn.big {{ font-size:15px; padding:11px 22px; }}
  .tip {{ color:{c['mut']}; font-size:11.5px; margin:8px 0 0; }}
  .tip b {{ color:{c['acc2']}; }}
  .tiles {{ display:grid; grid-template-columns: repeat(3, 1fr); gap:12px; margin:18px 0 24px; }}
  .tile {{ background:{c['card']}; border:1px solid {c['line']}; border-left:3px solid {c['acc']}; padding:18px; }}
  .tval {{ font-size:30px; font-weight:700; color:{c['fg']}; line-height:1.1; }}
  .tlab {{ font-size:11px; letter-spacing:.05em; text-transform:uppercase; color:{c['mut']}; margin-top:4px; }}
  .tsub {{ font-size:12px; color:{c['acc2']}; margin-top:4px; }}
  h2 {{ font-size:14px; letter-spacing:.05em; text-transform:uppercase; color:{c['acc2']};
    border-bottom:1px solid {c['line']}; padding-bottom:6px; margin:26px 0 14px; }}
  .cols {{ display:grid; grid-template-columns: 1fr 1fr; gap:26px; }}
  .colh {{ font-size:11px; letter-spacing:.06em; text-transform:uppercase; color:{c['mut']}; margin-bottom:6px; }}
  table.kv {{ width:100%; border-collapse:collapse; font-size:13px; }}
  table.kv td {{ padding:5px 2px; border-bottom:1px solid rgba(46,42,32,.5); }}
  table.kv td.k {{ color:{c['mut']}; white-space:nowrap; padding-right:12px; }}
  table.kv td.v {{ text-align:right; color:{c['fg']}; font-weight:600; }}
  .dim {{ color:{c['mut']}; font-weight:400; font-size:11px; }}
  .chart, .gantt {{ width:100%; height:auto; display:block; background:{c['card']};
    border:1px solid {c['line']}; border-radius:3px; }}
  .ax {{ fill:{c['mut']}; font-size:9px; font-family:ui-monospace,Consolas,monospace; }}
  .cleg {{ color:{c['mut']}; font-size:11px; margin:6px 0 0; display:flex; align-items:center; gap:5px; }}
  .cleg .dot {{ width:10px; height:10px; border-radius:2px; display:inline-block; }}
  .gbar {{ transition: opacity .12s; }}
  table.cells {{ width:100%; border-collapse:collapse; font-size:13px; }}
  table.cells th {{ text-align:left; color:{c['mut']}; font-weight:600; font-size:11px; letter-spacing:.04em;
    text-transform:uppercase; border-bottom:1px solid {c['line']}; padding:6px 8px; }}
  table.cells td {{ padding:6px 8px; border-bottom:1px solid rgba(46,42,32,.5); }}
  .mono {{ font-family: ui-monospace, Consolas, monospace; }}
  .num {{ text-align:right; }}
  .chip {{ font-size:11px; border:1px solid; border-radius:3px; padding:0 6px; text-transform:uppercase; letter-spacing:.03em; }}
  .retry {{ color:{c['run']}; font-size:11px; margin-left:6px; }}
  .reason {{ color:{c['bad']}; font-size:12px; }}
  .muted {{ color:{c['mut']}; font-size:13px; }}
  .foot {{ color:{c['mut']}; font-size:11px; margin-top:30px; border-top:1px solid {c['line']}; padding-top:12px; }}
  .foot a {{ color:{c['acc2']}; text-decoration:none; }}
  .foot a:hover {{ text-decoration:underline; }}
  @page {{ margin: 0; }}   /* full-bleed dark; .wrap supplies the inner margin so no gray page border */
  @media print {{
    html, body {{ background:{c['bg']} !important; }}
    .noprint {{ display:none !important; }}
    .gbar {{ opacity:1 !important; }}
    .wrap {{ max-width:none; padding:12mm; background:{c['bg']}; }}
    /* dark fills every page edge-to-edge; min-height makes a short page still cover the sheet
       (no white gap at the bottom) — needs "Background graphics" on in the print dialog. */
    .page {{ break-after: page; margin:0; background:{c['bg']}; }}
    .cells-page {{ display:none !important; }}   /* the cells table is screen-only — PDF is 2 pages */
    .graphs-page {{ break-after: auto; min-height: 100vh; }}   /* last page, fills the sheet, no blank trailer */
  }}
</style></head><body><div class="wrap">

  <section class="page specs-page">
    <header>
      <div>
        <div class="eyebrow">// benchmark</div>
        <h1>{_e(report.get('world', 'World'))}</h1>
        <div class="sub">{_e(_iso(sm.get('started')))} &rarr; {_e(_iso(sm.get('ended')))}
          &nbsp;·&nbsp; scale 1:{_e(round(1 / sm['scale'])) if sm.get('scale') else '?'}
          &nbsp;·&nbsp; cell {_e(sm.get('cell_size'))} &nbsp;·&nbsp; buildings {'on' if sm.get('buildings') else 'off'}
          {('&nbsp;·&nbsp; ' + _e(pf_line)) if pf_line else ''}</div>
      </div>
      {icon_html}
    </header>

    <div class="bar noprint">
      <button class="btn big" onclick="window.print()">Save as PDF</button>
    </div>
    <div class="tip noprint">For a clean PDF, in the print dialog turn <b>ON</b> &ldquo;Background graphics&rdquo;
      and turn <b>OFF</b> &ldquo;Headers and footers&rdquo;.</div>

    <div class="tiles">{_tiles(sm)}</div>

    <h2>Machine &amp; run settings</h2>
    <div class="cols">
      <div><div class="colh">This machine</div>{_machine_block(report.get('machine', {}), sm)}</div>
      <div><div class="colh">Run settings</div>{_config_block(report.get('config', {}), sm, pf)}</div>
    </div>
  </section>

  <section class="page graphs-page">
    <h2>CPU &amp; RAM over the run</h2>
    {_svg_cpu_ram(report.get('timeline', []))}
    <h2>Activity over the run</h2>
    {_svg_activity(report.get('timeline', []), peak=peak)}
    <h2>Cell timeline</h2>
    {_gantt(report.get('cells', []), sm.get('started'), sm.get('ended'))}
    <div class="bar noprint"><button class="btn ghost" id="playBtn">&#9654; Replay run</button></div>
    <div class="foot">Generated {_e(report.get('generated_at', ''))} &middot; Meld {_e(report.get('meld_version', ''))}
      &middot; <a href="https://meldmc.com" target="_blank" rel="noopener">meldmc.com</a>
      &middot; <a href="https://github.com/Teddy563/meld" target="_blank" rel="noopener">github.com/Teddy563/meld</a></div>
  </section>

  <section class="page cells-page">
    <h2>{_e(cells_head)}</h2>
    {more_note}{more_btn}
    <table class="cells"><thead><tr><th>Cell</th><th>Status</th><th>Worker</th><th class="num">Wall time</th><th>Note</th></tr></thead>
    <tbody>{rows_html}</tbody></table>
  </section>

</div>
<script>{_PLAYBACK_JS}</script>
</body></html>"""


def write_report(world_dir, report: dict) -> dict:
    out = {"json": None, "html": None}
    try:
        wp = Path(world_dir)
        wp.mkdir(parents=True, exist_ok=True)
        jp = wp / REPORT_JSON_NAME
        jp.write_text(json.dumps(report, indent=2), encoding="utf-8")
        out["json"] = jp
        hp = wp / REPORT_HTML_NAME
        hp.write_text(render_html(report), encoding="utf-8")
        out["html"] = hp
    except Exception:
        pass
    return out
