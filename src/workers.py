"""
Bounded worker pool — runs N concurrent Arnis jobs. Ported from the experimental
meld/worker_pool.py. Threads are sufficient because each job's hot path is a
blocking subprocess that releases the GIL.

The caller supplies a runner(job, worker_state) -> bool that does the real work
and updates worker_state for UI polling.
"""

from __future__ import annotations

import threading
import time
from collections import deque
from typing import Any, Callable

JobRunner = Callable[[dict, dict], bool]
JobCompleteCb = Callable[[dict, bool, dict], None]


class WorkerPool:
    MAX_WORKERS_HARD_CAP = 64   # generation is disk-write + RAM bound at the save phase,
                                # not CPU bound; many concurrent saves can peg the SSD and
                                # OOM-kill workers mid-save. High ceiling: the user owns the
                                # trade-off. Aim ~8 by default; push higher on fast NVMe + lots of RAM.

    # Per-worker first-job delay (seconds). Each worker offsets the start of its FIRST
    # job by worker_id * this, so the workers don't all enter the same generation phase
    # (network fetch / CPU tile-build / disk save) at the same wall-clock moment. Without
    # it, N workers launched together sawtooth the CPU (all idle on network, then all peg
    # 100% on tile-build, then all dip on save). Kept SMALL: only the initial lockstep needs
    # breaking; after the first cell, natural per-cell variance keeps the phases spread. A
    # big value just makes generation look slow to start (late workers idle). 0 disables it.
    stagger_seconds: float = 1.5
    stagger_cap_workers: int = 8   # workers past this all use the same max offset (8*1.5=12s)
    # Adaptive: instead of a fixed per-worker step, space the W first-jobs across roughly ONE
    # observed cell-time, so the last worker enters the CPU-heavy phase as the first one frees
    # (cores stay busy, nothing all-at-once). Falls back to the fixed step until there's history.
    stagger_adaptive: bool = True

    def __init__(self, max_workers: int = 1):
        self._max_workers = max(1, min(self.MAX_WORKERS_HARD_CAP, int(max_workers)))
        self._queue: deque[dict] = deque()
        self._lock = threading.Lock()
        self._cv = threading.Condition(self._lock)
        self._threads: dict[int, threading.Thread] = {}   # worker_id -> thread
        self._states: list[dict] = [self._idle_state(i) for i in range(self._max_workers)]
        self._stopped = False
        self._runner: JobRunner | None = None
        self._on_complete: JobCompleteCb | None = None
        self._avg_cell_s = 0.0          # EWMA of recent cell wall-times (0 = no history yet)

    def record_completion(self, duration_s: float) -> None:
        """Feed a finished cell's wall-time into the EWMA used to pace adaptive stagger.
        Damped (alpha 0.3) so one slow/fast cell can't whipsaw the spacing."""
        if duration_s and duration_s > 0:
            a = 0.3
            with self._lock:
                self._avg_cell_s = duration_s if self._avg_cell_s <= 0 else (a * duration_s + (1 - a) * self._avg_cell_s)

    def _first_job_delay(self, worker_id: int) -> float:
        """Seconds worker `worker_id` waits before its first job. Adaptive spaces the first
        jobs across ~one cell-time / workers (clamped to [step, step*1.3]); else fixed step."""
        step = self.stagger_seconds
        if step <= 0:
            return 0.0
        idx = min(worker_id, self.stagger_cap_workers)
        if self.stagger_adaptive and self._avg_cell_s > 0:
            even = self._avg_cell_s / max(1, self._max_workers)   # phase gap for continuous CPU
            # Adapt within +30% of the slider base (per the "+30% from default" intent): slow
            # cells widen the step a little; never below the base, never a long idle start.
            step = max(step, min(even, step * 1.3))
        return idx * step

    def configure(self, runner: JobRunner, on_complete: JobCompleteCb | None = None):
        with self._lock:
            self._runner = runner
            self._on_complete = on_complete

    def submit(self, job: dict) -> None:
        with self._cv:
            self._queue.append(job)
            self._ensure_workers_locked()
            self._cv.notify_all()

    def queue_size(self) -> int:
        with self._lock:
            return len(self._queue)

    def clear(self) -> int:
        with self._lock:
            n = len(self._queue)
            self._queue.clear()
            return n

    def is_running(self) -> bool:
        with self._lock:
            return any(s.get("running") for s in self._states)

    _NON_SERIAL = {"process"}

    def get_states(self) -> list[dict]:
        with self._lock:
            return [{k: v for k, v in s.items() if k not in self._NON_SERIAL}
                    for s in self._states]

    def set_max_workers(self, n: int) -> None:
        n = max(1, min(self.MAX_WORKERS_HARD_CAP, int(n)))
        with self._cv:
            self._max_workers = n
            # Grow the state list to the high-water-mark id, indexed by worker_id.
            # Length-based (not delta-based) so a shrink-then-grow never appends
            # duplicate slots. _states is never shrunk; ids >= n just stay idle.
            while len(self._states) < n:
                self._states.append(self._idle_state(len(self._states)))
            self._ensure_workers_locked()
            self._cv.notify_all()

    @property
    def max_workers(self) -> int:
        return self._max_workers

    def terminate_all(self) -> int:
        n = 0
        with self._lock:
            for s in self._states:
                proc = s.get("process")
                if proc is not None:
                    try:
                        proc.terminate()
                        n += 1
                    except Exception:
                        pass
        return n

    # ── internals ───────────────────────────────────────────────────────────
    def _idle_state(self, worker_id: int) -> dict:
        return {"worker_id": worker_id, "running": False, "progress": 0,
                "message": "Idle", "cell_key": None, "success": None}

    def _ensure_workers_locked(self):
        # Prune dead threads, then fill the lowest-free worker ids up to max.
        # Using lowest-free ids (not len()) means a still-alive high-id worker
        # from a prior shrink can't collide with a freshly spawned one.
        for wid in [w for w, t in self._threads.items() if not t.is_alive()]:
            del self._threads[wid]
        while len(self._threads) < self._max_workers:
            free = next((i for i in range(self._max_workers) if i not in self._threads), None)
            if free is None:
                break
            t = threading.Thread(target=self._worker_loop, args=(free,), daemon=True)
            self._threads[free] = t
            t.start()

    def _worker_loop(self, worker_id: int):
        first_job = True
        try:
            while True:
                with self._cv:
                    while not self._queue and not self._stopped:
                        if worker_id >= self._max_workers:
                            return
                        self._cv.wait()
                    if self._stopped or worker_id >= self._max_workers:
                        return
                    job = self._queue.popleft()
                    state = self._states[worker_id]
                    state.update(running=True, progress=0, message="Starting…",
                                 cell_key=job.get("cell_key", ""), success=None)
                    runner, on_complete = self._runner, self._on_complete

                # Desync the very first job per worker (see stagger_seconds). Done OUTSIDE
                # the lock so it never blocks other workers or the queue. First job only —
                # after that, natural per-cell variance keeps the phases spread out.
                if first_job:
                    first_job = False
                    delay = self._first_job_delay(worker_id)
                    if delay > 0 and not self._stopped:
                        state["message"] = f"Staggered start (+{int(delay)}s)…"
                        time.sleep(delay)

                ok, err = False, {}
                _t0 = time.monotonic()
                try:
                    if runner is None:
                        raise RuntimeError("worker pool has no runner configured")
                    ok = bool(runner(job, state))
                except Exception as ex:  # noqa: BLE001
                    err = {"error": str(ex)}
                    ok = False
                finally:
                    if ok:
                        self.record_completion(time.monotonic() - _t0)   # only successful cells pace the EWMA
                    with self._lock:
                        state.update(running=False, success=ok, process=None)
                        if not ok and "error" in err:
                            state["message"] = err["error"][:160]
                    if on_complete:
                        try:
                            on_complete(job, ok, err)
                        except Exception:
                            pass
        finally:
            # Release this id and reset its slot to idle when the thread exits.
            with self._lock:
                self._threads.pop(worker_id, None)
                if worker_id < len(self._states):
                    self._states[worker_id] = self._idle_state(worker_id)
