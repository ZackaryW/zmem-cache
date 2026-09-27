"""Measure cold admission, background facts, and warm exact-HEAD queries.

Run from the repository root after `cargo build` with
`python benchmarks/tiered_history.py --commits 10000`.
"""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
import json
import os
import shutil
import sqlite3
import statistics
import subprocess
import tempfile
import time
import venv
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SUFFIX = ".exe" if os.name == "nt" else ""
SERVICE = ROOT / "target" / "debug" / f"zmem-svc{SUFFIX}"
HOST = ROOT.parent / "zmem" / ".venv" / ("Scripts" if os.name == "nt" else "bin") / f"zmem-extension-host{SUFFIX}"


def git(repo: Path, *args: str) -> str:
    return subprocess.check_output(["git", "-C", str(repo), *args], text=True).strip()


def make_history(repo: Path, count: int, merge_every: int = 0, annotation_every: int = 0) -> str:
    subprocess.run(["git", "init", "-q", "-b", "main", str(repo)], check=True)
    process = subprocess.Popen(
        ["git", "-C", str(repo), "fast-import", "--quiet"], stdin=subprocess.PIPE
    )
    assert process.stdin is not None
    main_mark = 0
    for index in range(count):
        is_side = merge_every > 0 and index > 0 and index % merge_every == 0 and index + 1 < count
        body = (f"bench commit {index + 1}\n" +
                (f"\nzmem(DECISION): benchmark decision {index + 1}\n" if annotation_every and index % annotation_every == 0 else "")).encode()
        process.stdin.write(b"commit refs/heads/side\n" if is_side else b"commit refs/heads/main\n")
        process.stdin.write(f"mark :{index + 1}\n".encode())
        process.stdin.write(b"author Bench <bench@example.com> 1700000000 +0000\n")
        process.stdin.write(b"committer Bench <bench@example.com> 1700000000 +0000\n")
        process.stdin.write(f"data {len(body)}\n".encode() + body)
        if main_mark:
            process.stdin.write(f"from :{main_mark}\n".encode())
        if not is_side and index and merge_every > 0 and index % merge_every == 1 and index > 1:
            process.stdin.write(f"merge :{index}\n".encode())
        content = f"{index}\n".encode()
        process.stdin.write(b"M 100644 inline memory.txt\n")
        process.stdin.write(f"data {len(content)}\n".encode() + content)
        process.stdin.write(b"\n")
        if not is_side:
            main_mark = index + 1
    process.stdin.close()
    if process.wait() != 0:
        raise RuntimeError("git fast-import failed")
    return git(repo, "rev-parse", "HEAD")


def run_service(environment: dict[str, str], *args: str) -> tuple[float, subprocess.CompletedProcess[str]]:
    started = time.perf_counter()
    result = subprocess.run(
        [str(SERVICE), *args], env=environment, capture_output=True, text=True, timeout=130
    )
    return (time.perf_counter() - started) * 1000, result


def percentile(values: list[float], quantile: float) -> float:
    ordered = sorted(values)
    return round(ordered[max(0, int(len(ordered) * quantile + 0.999999) - 1)], 1)


def main() -> None:
    global SERVICE
    parser = argparse.ArgumentParser()
    parser.add_argument("--service", type=Path, default=SERVICE)
    parser.add_argument("--adaptive-workload", action="store_true")
    parser.add_argument("--utilization-audit", action="store_true", help="separate checkpoint-settled accounting run; not comparable latency samples")
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--commits", type=int, default=10_000)
    parser.add_argument("--requested-commits", type=int, default=1, help="requested trail width")
    parser.add_argument("--warm-samples", type=int, default=30)
    parser.add_argument("--lookup-clients", type=int, default=1, help="parallel warm lookup clients")
    parser.add_argument("--merge-every", type=int, default=0, help="create a side commit and merge at this interval")
    parser.add_argument("--deep-check", action="store_true", help="overlap warm lookups with a 500-commit deep check")
    parser.add_argument("--installed-host", action="store_true", help="assemble a disposable installed host layout")
    parser.add_argument("--trace-host", action="store_true", help="count identity calls in the disposable installed host")
    parser.add_argument("--trace-git", action="store_true")
    parser.add_argument("--exercise-restart", action="store_true")
    parser.add_argument("--exercise-rewrite", action="store_true")
    args = parser.parse_args()
    SERVICE = args.service.resolve()
    if args.adaptive_workload:
        from adaptive_workload import run_workload
        run_workload(args, SERVICE, make_history, percentile)
        return
    if args.commits < 1 or args.warm_samples < 1 or not 1 <= args.lookup_clients <= 128 or args.merge_every < 0 or not 1 <= args.requested_commits <= args.commits:
        parser.error("counts must be positive")
    if args.merge_every == 1:
        parser.error("merge interval must be at least 2")
    if args.exercise_restart and args.exercise_rewrite:
        parser.error("restart and rewrite exercises are separate runs")
    if args.deep_check and (args.exercise_restart or args.exercise_rewrite):
        parser.error("deep-check overlap is a separate run")
    if args.trace_host and not args.installed_host:
        parser.error("host tracing requires --installed-host")
    if args.exercise_rewrite and args.commits < 2:
        parser.error("rewrite exercise needs at least two commits")
    if not SERVICE.is_file() or not HOST.is_file():
        parser.error("build zmem-svc and install the adjacent zmem extension host first")
    with tempfile.TemporaryDirectory(prefix="zmem-tier-bench-") as directory:
        root = Path(directory)
        repo = root / "repo"
        home = root / "home"
        home.mkdir()
        head = make_history(repo, args.commits, args.merge_every)
        environment = os.environ.copy()
        environment["ZMEM_HOME"] = str(home)
        host_trace = root / "host-identity-trace"
        host_count = lambda: len(host_trace.read_text().splitlines()) if host_trace.exists() else 0
        host_events = root / "host-events"
        host_events.mkdir()
        if args.installed_host:
            runtime = root / "runtime"
            binary = runtime / "binary"
            binary.mkdir(parents=True)
            installed_service = binary / SERVICE.name
            shutil.copy2(SERVICE, installed_service)
            host_root = runtime / "host"
            venv.EnvBuilder(with_pip=False).create(host_root)
            python = host_root / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
            purelib = subprocess.check_output(
                [str(python), "-c", "import sysconfig; print(sysconfig.get_path('purelib'))"], text=True
            ).strip()
            package = Path(purelib) / "zmem"
            shutil.copytree(
                ROOT.parent / "zmem" / "src" / "zmem", package,
                ignore=shutil.ignore_patterns("__pycache__", "*.pyc", "_native"),
            )
            if args.trace_host:
                host_source = package / "host.py"
                source = host_source.read_text()
                signature = "def identity_request(payload: dict) -> dict:\n"
                if signature not in source:
                    raise RuntimeError("installed host identity signature changed")
                source = source.replace(
                    signature,
                    signature + "    with open(os.environ['ZMEM_HOST_TRACE'], 'a') as trace:\n"
                    "        trace.write('identity\\n')\n",
                    1,
                )
                main_signature = "def main() -> None:\n"
                failure_signature = "        raise SystemExit(4) from exc\n"
                if main_signature not in source or failure_signature not in source:
                    raise RuntimeError("installed host main signature changed")
                source = source.replace(
                    main_signature,
                    main_signature + "    import time\n"
                    "    with open(os.path.join(os.environ['ZMEM_HOST_EVENTS'], f'{os.getpid()}.log'), 'a') as events:\n"
                    "        events.write(f'{time.monotonic_ns()} start {os.getpid()}\\n')\n",
                    1,
                )
                source = source.replace(
                    failure_signature,
                    failure_signature + "    finally:\n"
                    "        with open(os.path.join(os.environ['ZMEM_HOST_EVENTS'], f'{os.getpid()}.log'), 'a') as events:\n"
                    "            events.write(f'{time.monotonic_ns()} end {os.getpid()}\\n')\n",
                    1,
                )
                host_source.write_text(source)
                environment["ZMEM_HOST_TRACE"] = str(host_trace)
                environment["ZMEM_HOST_EVENTS"] = str(host_events)
            SERVICE = installed_service
            for name in ("ZMEM_EXTENSION_HOST", "PYTHONPATH", "PYTHONHOME", "PYTHONUSERBASE"):
                environment.pop(name, None)
        else:
            environment["ZMEM_EXTENSION_HOST"] = str(HOST)
        trace = root / "git-trace"
        trace.mkdir()
        if args.trace_git:
            environment["GIT_TRACE2_EVENT"] = str(trace)
        git_count = lambda: sum(1 for path in trace.iterdir() if path.is_file())
        wal = home / "db" / "entries.db-wal"
        wal_peak = [0]

        def sample_wal() -> None:
            wal_peak[0] = max(wal_peak[0], wal.stat().st_size if wal.exists() else 0)

        (home / "config.toml").write_text(
            f"background_commit_limit = {args.commits}\n"
            "staging_max_bytes = 268435456\n"
        )
        deep_check = None
        try:
            before_cold_git = git_count()
            cold_ms, cold = run_service(
                environment, "query", str(repo), "--commit-limit", str(args.requested_commits), "--timeout-ms", "10000"
            )
            if cold.returncode == 0:
                raise RuntimeError("cold query unexpectedly hit a published trail")
            error = json.loads(cold.stderr)
            if error.get("code") != "not_ready":
                raise RuntimeError(f"cold query failed: {error}")
            job_id = error["job_id"]
            cold_git = git_count() - before_cold_git
            started = time.perf_counter()
            queue_peak = 0
            job_queue_wait_ms = None
            job_work_ms = None
            publication_metric = None
            while time.perf_counter() - started < 120:
                _, status = run_service(environment, "job-status", job_id, "--timeout-ms", "10000")
                if status.returncode:
                    raise RuntimeError(f"job status failed: {status.stderr}")
                status_payload = json.loads(status.stdout)
                state = status_payload["state"]
                queue_peak = max(queue_peak, status_payload["queue_depth"])
                job_queue_wait_ms = status_payload["queue_wait_ms"]
                job_work_ms = status_payload["work_ms"]
                publication_metric = status_payload.get("last_publication")
                sample_wal()
                if state == "ready":
                    break
                if state == "failed":
                    raise RuntimeError(f"index job failed: {status.stdout}")
                time.sleep(0.05)
            else:
                raise TimeoutError("requested trail did not publish")
            publish_ms = round((time.perf_counter() - started) * 1000, 1)
            if publication_metric is None or head not in publication_metric["trail_id"]:
                raise RuntimeError(f"requested publication metric is missing: {publication_metric}")
            publication_wal_bytes = wal.stat().st_size if wal.exists() else 0
            published_at = time.perf_counter()
            host_before_warm = host_count()
            restart_checkpoint = None
            rewrite_checkpoint = None
            selected_head = head
            selected_count = args.commits
            if args.exercise_restart or args.exercise_rewrite:
                database = home / "db" / "entries.db"
                wait_until = time.perf_counter() + 20
                while time.perf_counter() < wait_until:
                    with closing(sqlite3.connect(database)) as connection:
                        row = connection.execute(
                            "SELECT completed_count,state FROM prefetch_jobs WHERE head_oid=?", (head,)
                        ).fetchone()
                    if row and 64 <= row[0] < args.commits and row[1] == "running":
                        restart_checkpoint = row[0]
                        break
                    if row and row[1] == "ready":
                        raise RuntimeError("prefetch completed before restart checkpoint; use more commits")
                    time.sleep(0.01)
                if restart_checkpoint is None:
                    raise TimeoutError("background checkpoint did not appear before restart")
            if args.exercise_restart:
                _, stopped = run_service(environment, "stop", "--timeout-ms", "10000")
                if stopped.returncode:
                    raise RuntimeError(f"could not stop before restart: {stopped.stderr}")
                _, ensured = run_service(environment, "ensure", "--timeout-ms", "10000")
                if ensured.returncode:
                    raise RuntimeError(f"could not restart service: {ensured.stderr}")
            if args.exercise_rewrite:
                rewrite_checkpoint = restart_checkpoint
                restart_checkpoint = None
                git(repo, "reset", "--hard", "HEAD~1")
                selected_head = git(repo, "rev-parse", "HEAD")
                selected_count -= 1
                _, changed = run_service(
                    environment, "query", str(repo), "--commit-limit", str(args.requested_commits), "--timeout-ms", "10000"
                )
                if changed.returncode == 0 or json.loads(changed.stderr).get("code") != "not_ready":
                    raise RuntimeError(f"rewritten HEAD did not start a distinct job: {changed.stdout} {changed.stderr}")
                changed_job = json.loads(changed.stderr)["job_id"]
                wait_until = time.perf_counter() + 30
                while time.perf_counter() < wait_until:
                    _, status = run_service(environment, "job-status", changed_job, "--timeout-ms", "10000")
                    if status.returncode:
                        raise RuntimeError(f"rewritten job status failed: {status.stderr}")
                    if json.loads(status.stdout)["state"] == "ready":
                        break
                    time.sleep(0.05)
                else:
                    raise TimeoutError("rewritten HEAD did not publish")
                with closing(sqlite3.connect(database)) as connection:
                    mixed = connection.execute(
                        "SELECT COUNT(*) FROM trail_membership m JOIN trails t ON t.id=m.trail_id "
                        "WHERE t.head_oid=? AND m.commit_oid=?", (selected_head, head)
                    ).fetchone()[0]
                if mixed:
                    raise RuntimeError("rewritten trail included the obsolete pinned HEAD")
            deep_check_started = None
            if args.deep_check:
                deep_check_started = time.perf_counter()
                deep_check = subprocess.Popen(
                    [str(SERVICE), "check", str(repo), "--deep", "--commit-limit", "500", "--timeout-ms", "120000"],
                    env=environment, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE, text=True,
                )
                assert deep_check.stdin is not None
                deep_check.stdin.write("zmem(DECISION): benchmark proposal\n")
                deep_check.stdin.close()
                deep_check.stdin = None
            times = []
            active_samples = 0
            deep_check_overlap_samples = 0
            with ThreadPoolExecutor(max_workers=args.lookup_clients) as clients:
                for offset in range(0, args.warm_samples, args.lookup_clients):
                    batch_count = min(args.lookup_clients, args.warm_samples - offset)
                    sample_wal()
                    deep_check_overlap_samples += batch_count * int(
                        deep_check is not None and deep_check.poll() is None
                    )
                    with closing(sqlite3.connect(home / "db" / "entries.db")) as connection:
                        row = connection.execute(
                            "SELECT state FROM prefetch_jobs WHERE head_oid=?", (selected_head,)
                        ).fetchone()
                    active_samples += batch_count * int(row is None or row[0] != "ready")
                    batch = list(clients.map(
                        lambda _: run_service(
                            environment, "query", str(repo), "--commit-limit",
                            str(args.requested_commits), "--timeout-ms", "10000"
                        ),
                        range(batch_count),
                    ))
                    for elapsed, query in batch:
                        if query.returncode:
                            raise RuntimeError(f"warm query failed: {query.stderr}")
                        payload = json.loads(query.stdout)
                        if payload["summary"]["trail"]["resolved_oid"] != selected_head:
                            raise RuntimeError("warm query returned a different HEAD")
                        times.append(elapsed)
            host_during_warm = host_count() - host_before_warm
            _, observed_status = run_service(environment, "job-status", job_id, "--timeout-ms", "10000")
            if observed_status.returncode:
                raise RuntimeError(f"post-warm job status failed: {observed_status.stderr}")
            lookup_admitted_peak = json.loads(observed_status.stdout)["lookup_admitted_peak"]
            deep_check_ms = None
            if deep_check is not None:
                stdout, stderr = deep_check.communicate(timeout=130)
                deep_check_ms = round((time.perf_counter() - deep_check_started) * 1000, 1)
                if deep_check.returncode:
                    raise RuntimeError(f"deep check failed: {stdout} {stderr}")
            started = time.perf_counter()
            database = home / "db" / "entries.db"
            prefetch = None
            while time.perf_counter() - started < 120:
                sample_wal()
                with closing(sqlite3.connect(database)) as connection:
                    prefetch = connection.execute(
                        "SELECT completed_count,state FROM prefetch_jobs WHERE head_oid=?", (selected_head,)
                    ).fetchone()
                    facts = connection.execute("SELECT COUNT(*) FROM raw_commit_facts").fetchone()[0]
                    parent_edges = connection.execute("SELECT COUNT(*) FROM raw_parent_edges").fetchone()[0]
                    ancestry_pairs = connection.execute("SELECT COUNT(*) FROM commit_ancestry").fetchone()[0]
                if prefetch == (selected_count, "ready"):
                    break
                if prefetch and prefetch[1] in {"paused", "obsolete", "failed"}:
                    break
                time.sleep(0.1)
            with closing(sqlite3.connect(database)) as connection:
                raw_fact_bytes = connection.execute(
                    "SELECT COALESCE(SUM(bytes),0) FROM raw_commit_facts"
                ).fetchone()[0]
                raw_parent_edge_bytes = connection.execute(
                    "SELECT COALESCE(SUM(bytes),0) FROM raw_parent_edges"
                ).fetchone()[0]
                unreferenced_staging_bytes = connection.execute(
                    "SELECT COALESCE((SELECT SUM(r.bytes) FROM raw_commit_facts r "
                    "WHERE NOT EXISTS(SELECT 1 FROM trail_membership m WHERE m.repository_id=r.repository_id AND m.commit_oid=r.commit_oid)),0) "
                    "+ COALESCE((SELECT SUM(e.bytes) FROM raw_parent_edges e "
                    "WHERE NOT EXISTS(SELECT 1 FROM trail_membership m WHERE m.repository_id=e.repository_id AND m.commit_oid=e.commit_oid)),0)"
                ).fetchone()[0]
            obsolete_state = None
            if args.exercise_rewrite:
                with closing(sqlite3.connect(database)) as connection:
                    old = connection.execute(
                        "SELECT state FROM prefetch_jobs WHERE head_oid=?", (head,)
                    ).fetchone()
                obsolete_state = old[0] if old else None
                if obsolete_state != "obsolete":
                    raise RuntimeError(f"old pinned prefetch was not obsoleted: {obsolete_state}")
            idle_git_before = git_count()
            idle_times = []
            for _ in range(min(10, args.warm_samples)):
                elapsed, query = run_service(
                    environment, "query", str(repo), "--commit-limit", str(args.requested_commits), "--timeout-ms", "10000"
                )
                if query.returncode:
                    raise RuntimeError(f"idle warm query failed: {query.stderr}")
                idle_times.append(elapsed)
            host_peak = None
            host_trace_unmatched_starts = None
            if args.trace_host:
                events = [
                    line.split()
                    for path in host_events.iterdir()
                    for line in path.read_text().splitlines()
                    if line.strip()
                ]
                malformed_host_events = sum(
                    len(row) != 3 or not row[0].isdigit() for row in events
                )
                events = [row for row in events if len(row) == 3 and row[0].isdigit()]
                active = set()
                host_peak = 0
                for _, event, pid in sorted(events, key=lambda row: int(row[0])):
                    if event == "start":
                        active.add(pid)
                    else:
                        active.discard(pid)
                    host_peak = max(host_peak, len(active))
                host_trace_unmatched_starts = len(active)
            report = {
                "commits": args.commits,
                "requested_commits": args.requested_commits,
                "lookup_clients": args.lookup_clients,
                "installed_host": args.installed_host,
                "merge_every": args.merge_every,
                "selected_commits_after_rewrite": selected_count if args.exercise_rewrite else None,
                "cold_admission_ms": round(cold_ms, 1),
                "cold_git_processes": cold_git if args.trace_git else None,
                "requested_publish_ms": publish_ms,
                "requested_job_queue_wait_ms": job_queue_wait_ms,
                "requested_job_work_ms": job_work_ms,
                "publication_transaction_us": publication_metric["transaction_us"],
                "publication_wal_bytes_before": publication_metric["wal_bytes_before"],
                "publication_wal_bytes_after": publication_metric["wal_bytes_after"],
                "queue_depth_peak_observed": queue_peak,
                "lookup_admitted_peak": lookup_admitted_peak,
                "wal_bytes_at_publication": publication_wal_bytes,
                "wal_bytes_peak_observed": wal_peak[0],
                "deep_check_ms": deep_check_ms,
                "deep_check_overlap_samples": deep_check_overlap_samples,
                "identity_host_calls_during_warm": host_during_warm if args.trace_host else None,
                "host_process_peak": host_peak,
                "host_trace_unmatched_starts": host_trace_unmatched_starts,
                "host_trace_malformed_events": malformed_host_events if args.trace_host else None,
                "warm_ms": {
                    "p50": percentile(times, 0.50),
                    "p95": percentile(times, 0.95),
                    "p99": percentile(times, 0.99),
                    "mean": round(statistics.mean(times), 1),
                    "samples_during_prefetch": active_samples,
                },
                "idle_warm_ms": {
                    "p50": percentile(idle_times, 0.50),
                    "p95": percentile(idle_times, 0.95),
                    "p99": percentile(idle_times, 0.99),
                },
                "idle_git_processes_per_query": (
                    round((git_count() - idle_git_before) / len(idle_times), 1) if args.trace_git else None
                ),
                "raw_facts": facts,
                "raw_parent_edges": parent_edges,
                "raw_fact_bytes": raw_fact_bytes,
                "raw_parent_edge_bytes": raw_parent_edge_bytes,
                "unreferenced_staging_bytes": unreferenced_staging_bytes,
                "transitive_ancestry_pairs": ancestry_pairs,
                "prefetch": prefetch,
                "prefetch_wait_ms": round((time.perf_counter() - started) * 1000, 1),
                "prefetch_since_publish_ms": round((time.perf_counter() - published_at) * 1000, 1),
                "restart_checkpoint": restart_checkpoint,
                "rewrite_checkpoint": rewrite_checkpoint,
                "obsolete_prefetch_state": obsolete_state,
                "total_git_processes": git_count() if args.trace_git else None,
                "database_bytes": database.stat().st_size,
            }
            print(json.dumps(report, indent=2))
        finally:
            if deep_check is not None and deep_check.poll() is None:
                deep_check.terminate()
                deep_check.communicate(timeout=5)
            run_service(environment, "stop", "--timeout-ms", "10000")


if __name__ == "__main__":
    main()
