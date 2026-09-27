from __future__ import annotations

import json
import os
import shutil
import signal
import sqlite3
import subprocess
import tempfile
import time
import venv
from pathlib import Path

ROOT = Path(__file__).parents[2]
ZMEM_ROOT = ROOT.parent / "zmem"


def before_scenario(context, _scenario) -> None:
    context.temp_root = Path(tempfile.mkdtemp(prefix="zmem-cache-behave-"))
    context.repo = context.temp_root / "repo"
    context.home = context.temp_root / "home"
    context.home.mkdir()
    context.env = os.environ.copy()
    context.env["ZMEM_HOME"] = str(context.home)
    context.default_user = context.temp_root / "default-user"
    context.default_user.mkdir()
    context.env["USERPROFILE"] = str(context.default_user)
    context.env["HOME"] = str(context.default_user)
    scripts = "Scripts" if os.name == "nt" else "bin"
    suffix = ".exe" if os.name == "nt" else ""
    context.env["ZMEM_EXTENSION_HOST"] = str((ZMEM_ROOT / ".venv" / scripts / f"zmem-extension-host{suffix}").resolve())
    context.svc = Path(os.environ.get("ZMEM_TEST_SERVICE", ROOT / "target" / "debug" / f"zmem-svc{suffix}"))
    context.commit_count = 0
    context.service_pids = set()
    context.service_paths = {str(context.svc.resolve()).casefold()}


def assemble_test_runtime(context) -> None:
    runtime = context.temp_root / "runtime"
    binary_dir = runtime / "binary"
    binary_dir.mkdir(parents=True)
    installed = binary_dir / context.svc.name
    shutil.copy2(context.svc, installed)
    host = runtime / "host"
    venv.EnvBuilder(with_pip=False).create(host)
    context.svc = installed
    context.env.pop("ZMEM_EXTENSION_HOST", None)
    existing = context.env.get("PYTHONPATH")
    context.env["PYTHONPATH"] = str(ZMEM_ROOT / "src") + (os.pathsep + existing if existing else "")


def after_scenario(context, _scenario) -> None:
    if context.svc.is_file():
        subprocess.run(
            [context.svc, "stop"],
            env=context.env,
            capture_output=True,
            timeout=5,
            check=False,
        )
    deadline = time.monotonic() + 2
    while time.monotonic() < deadline and any(_process_alive(context, pid) for pid in context.service_pids):
        time.sleep(0.05)
    for pid in context.service_pids:
        if _process_alive(context, pid):
            os.kill(pid, signal.SIGTERM)
    shutil.rmtree(context.temp_root, ignore_errors=True)


def _process_alive(context, pid: int) -> bool:
    if os.name == "nt":
        import ctypes

        kernel = ctypes.windll.kernel32
        handle = kernel.OpenProcess(0x1000, False, pid)
        if not handle:
            return False
        code = ctypes.c_ulong()
        image = ctypes.create_unicode_buffer(32768)
        image_length = ctypes.c_ulong(len(image))
        try:
            return (
                bool(kernel.GetExitCodeProcess(handle, ctypes.byref(code)))
                and code.value == 259
                and bool(kernel.QueryFullProcessImageNameW(handle, 0, image, ctypes.byref(image_length)))
                and image.value.casefold() in context.service_paths
            )
        finally:
            kernel.CloseHandle(handle)
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    executable = Path(f"/proc/{pid}/exe")
    return not executable.exists() or str(executable.resolve()).casefold() in context.service_paths


def init_repo(context) -> None:
    context.repo.mkdir(exist_ok=True)
    subprocess.run(["git", "init", "-q", context.repo], check=True)
    subprocess.run(["git", "-C", context.repo, "config", "user.name", "Test"], check=True)
    subprocess.run(["git", "-C", context.repo, "config", "user.email", "test@example.com"], check=True)


def commit(context, body: str, *, subject: str = "feat(core): memory", timestamp: str | None = None) -> str:
    context.commit_count += 1
    (context.repo / "memory.txt").write_text(f"revision {context.commit_count}\n")
    subprocess.run(["git", "-C", context.repo, "add", "memory.txt"], check=True)
    env = context.env.copy()
    if timestamp:
        env["GIT_AUTHOR_DATE"] = timestamp
        env["GIT_COMMITTER_DATE"] = timestamp
    command = ["git", "-C", context.repo, "commit", "-q", "-m", subject]
    if body:
        command += ["-m", body]
    subprocess.run(command, env=env, check=True)
    return subprocess.run(
        ["git", "-C", context.repo, "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def run_svc(context, *args: str, input_text: str | None = None) -> subprocess.CompletedProcess[str]:
    context.service_paths.add(str(context.svc.resolve()).casefold())
    context.completed = subprocess.run(
        [context.svc, *args],
        env=context.env,
        input=input_text,
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    context.payload = None
    state_file = context.home / "service.json"
    if state_file.exists():
        try:
            context.service_pids.add(int(json.loads(state_file.read_text())["pid"]))
        except (OSError, ValueError, KeyError, json.JSONDecodeError):
            pass
    if context.completed.stdout.strip():
        try:
            context.payload = json.loads(context.completed.stdout)
        except json.JSONDecodeError:
            pass
    return context.completed


def query_until_ready(context, *args: str) -> subprocess.CompletedProcess[str]:
    deadline = time.monotonic() + 30
    while True:
        result = run_svc(context, *args)
        if result.returncode == 0:
            return result
        try:
            failure = json.loads(result.stderr)
        except json.JSONDecodeError:
            return result
        if failure.get("code") != "not_ready":
            return result
        job_id = failure["job_id"]
        if time.monotonic() >= deadline:
            raise AssertionError(f"indexing job {job_id} did not finish: {failure}")
        time.sleep(0.05)


def database(context) -> sqlite3.Connection:
    return sqlite3.connect(context.home / "db" / "entries.db")


def write_config(context, **values) -> None:
    lines = []
    for key, value in values.items():
        if isinstance(value, (str, list)):
            encoded = json.dumps(value)
        else:
            encoded = str(value).lower() if isinstance(value, bool) else str(value)
        lines.append(f"{key} = {encoded}")
    (context.home / "config.toml").write_text("\n".join(lines) + "\n")


def write_global_extension(context, branch: str, name: str, source: str) -> Path:
    path = context.home / "ext" / branch / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(source)
    return path
