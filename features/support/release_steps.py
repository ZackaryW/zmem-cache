from __future__ import annotations

import hashlib
import json
import subprocess
import sys
from pathlib import Path

from behave import given, then, when

TARGETS = (
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
    "i686-pc-windows-msvc",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
)


def _name(target: str) -> str:
    return f"zmem-svc-{target}" + (".exe" if target.endswith("windows-msvc") else "")


def _stage_release(context, *, malformed: bool) -> None:
    context.release_dir = context.temp_root / "release"
    context.release_dir.mkdir()
    context.artifacts = []
    context.identities = []
    for index, target in enumerate(TARGETS):
        artifact = context.release_dir / _name(target)
        artifact.write_bytes(f"artifact-{index}".encode())
        identity = context.release_dir / f"{target}.identity.json"
        payload = {"release_version": "0.1.0", "protocol_version": 2, "schema_version": 2}
        if malformed and index == 0:
            payload["unknown"] = True
        identity.write_text(json.dumps(payload), encoding="utf-8")
        context.artifacts.extend(["--artifact", f"{target}={artifact}"])
        context.identities.extend(["--identity", f"{target}={identity}"])
    context.manifest = context.release_dir / "release-manifest.json"


@given("staged service artifacts and identities for every supported target")
def given_complete_release(context):
    _stage_release(context, malformed=False)


@given("staged service artifacts with malformed identity metadata")
def given_malformed_release(context):
    _stage_release(context, malformed=True)


@when("I assemble the release manifest through its command")
def when_assemble_manifest(context):
    root = Path(__file__).parents[2]
    context.completed = subprocess.run(
        [
            sys.executable,
            root / "scripts" / "release_manifest.py",
            "--version",
            "0.1.0",
            "--output",
            context.manifest,
            *context.artifacts,
            *context.identities,
        ],
        cwd=root,
        capture_output=True,
        text=True,
        check=False,
    )


@then("the manifest describes the complete target set with verified sizes and checksums")
def then_complete_manifest(context):
    assert context.completed.returncode == 0, (context.completed.stdout, context.completed.stderr)
    payload = json.loads(context.manifest.read_text())
    assert payload["release_version"] == "0.1.0"
    assert {asset["target"] for asset in payload["assets"]} == set(TARGETS)
    for asset in payload["assets"]:
        path = context.release_dir / asset["name"]
        assert asset["size"] == path.stat().st_size
        assert asset["sha256"] == hashlib.sha256(path.read_bytes()).hexdigest()


@then("release assembly fails without writing a manifest")
def then_manifest_rejected(context):
    assert context.completed.returncode != 0
    assert not context.manifest.exists()
