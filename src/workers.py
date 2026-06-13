"""
Bounded worker pool — runs N concurrent Arnis jobs. Ported from the experimental
meld/worker_pool.py. Threads are sufficient because each job's hot path is a
blocking subprocess that releases the GIL.

The caller supplies a runner(job, worker_state) -> bool that does the real work
and updates worker_state for UI polling.
"""

from __future__ import annotations

import threading
from collections import deque
from typing import Any, Callable

JobRunner = Callable[[dict, dict], bool]
JobCompleteCb = Callable[[dict, bool, dict], None]


class WorkerPool:
    MAX_WORKERS_HARD_CAP = 16   # generation is disk-write + RAM bound at the save phase,
                                # not CPU bound; >16 concurrent saves peg the SSD and can
                                # OOM-kill workers mid-save. Aim ~8 by default.

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

                ok, err = False, {}
                try:
                    if runner is None:
                        raise RuntimeError("worker pool has no runner configured")
                    ok = bool(runner(job, state))
                except Exception as ex:  # noqa: BLE001
                    err = {"error": str(ex)}
                    ok = False
                finally:
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
