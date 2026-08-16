"""Assemble a strict native-service release manifest."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any

REQUIRED_TARGETS = (
    "aarch64-apple-darwin",
    "aarch64-pc-windows-msvc",
    "aarch64-unknown-linux-musl",
    "i686-pc-windows-msvc",
    "x86_64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-musl",
)
SEMVER_PATTERN = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:[-+][0-9A-Za-z.-]+)?")
SHA256_BLOCK_SIZE = 1024 * 1024


def asset_name(target: str) -> str:
    suffix = ".exe" if target.endswith("windows-msvc") else ""
    return f"zmem-svc-{target}{suffix}"


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(SHA256_BLOCK_SIZE), b""):
            digest.update(block)
    return digest.hexdigest()


def _identity(value: object, *, target: str, release_version: str) -> dict[str, Any]:
    if not isinstance(value, Mapping):
        raise TypeError(f"identity for {target} must be an object")
    expected = {"release_version", "protocol_version", "schema_version"}
    missing = expected - set(value)
    unknown = set(value) - expected
    if missing:
        raise ValueError(f"identity for {target} is missing fields: {', '.join(sorted(missing))}")
    if unknown:
        raise ValueError(f"identity for {target} has unknown fields: {', '.join(sorted(unknown))}")
    if value["release_version"] != release_version:
        raise ValueError(f"identity for {target} has a different release version")
    for field in ("protocol_version", "schema_version"):
        if type(value[field]) is not int or value[field] <= 0:
            raise ValueError(f"identity {field} for {target} must be a positive integer")
    return dict(value)


def assemble_manifest(
    release_version: str,
    artifacts: Mapping[str, Path],
    identities: Mapping[str, object],
) -> dict[str, Any]:
    if not SEMVER_PATTERN.fullmatch(release_version):
        raise ValueError("release version must be semantic version text")
    required = set(REQUIRED_TARGETS)
    if set(artifacts) != required or set(identities) != required:
        raise ValueError("release target coverage must exactly match the supported target set")

    protocol_version: int | None = None
    schema_version: int | None = None
    assets: list[dict[str, Any]] = []
    for target in sorted(REQUIRED_TARGETS):
        path = artifacts[target]
        if not path.is_file():
            raise ValueError(f"release artifact for {target} does not exist")
        expected_name = asset_name(target)
        if path.name != expected_name:
            raise ValueError(f"release asset name for {target} must be {expected_name}")
        identity = _identity(identities[target], target=target, release_version=release_version)
        current_protocol = identity["protocol_version"]
        current_schema = identity["schema_version"]
        if protocol_version is None:
            protocol_version = current_protocol
            schema_version = current_schema
        elif current_protocol != protocol_version or current_schema != schema_version:
            raise ValueError("release artifact protocol or schema identities disagree")
        size = path.stat().st_size
        if size <= 0:
            raise ValueError(f"release artifact for {target} is empty")
        assets.append(
            {
                "target": target,
                "name": expected_name,
                "size": size,
                "sha256": _sha256(path),
            }
        )
    assert protocol_version is not None and schema_version is not None
    return {
        "manifest_version": 1,
        "release_version": release_version,
        "protocol_version": protocol_version,
        "schema_version": schema_version,
        "assets": assets,
    }


def render_manifest(manifest: Mapping[str, Any]) -> str:
    return json.dumps(manifest, indent=2, sort_keys=True) + "\n"


def _assignments(values: Sequence[str], *, label: str) -> dict[str, Path]:
    parsed: dict[str, Path] = {}
    for value in values:
        target, separator, path = value.partition("=")
        if not separator or not target or not path:
            raise ValueError(f"{label} must use TARGET=PATH")
        if target in parsed:
            raise ValueError(f"duplicate {label} target: {target}")
        parsed[target] = Path(path)
    return parsed


def _read_identities(paths: Mapping[str, Path]) -> dict[str, object]:
    identities: dict[str, object] = {}
    for target, path in paths.items():
        try:
            identities[target] = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise ValueError(f"invalid identity for {target}: {exc}") from exc
    return identities


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--artifact", action="append", default=[])
    parser.add_argument("--identity", action="append", default=[])
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    options = _parser().parse_args(argv)
    try:
        artifacts = _assignments(options.artifact, label="artifact")
        identity_paths = _assignments(options.identity, label="identity")
        manifest = assemble_manifest(options.version, artifacts, _read_identities(identity_paths))
        options.output.parent.mkdir(parents=True, exist_ok=True)
        temporary = options.output.with_suffix(options.output.suffix + ".tmp")
        temporary.write_text(render_manifest(manifest), encoding="utf-8")
        temporary.replace(options.output)
    except (OSError, TypeError, ValueError) as exc:
        print(f"release manifest assembly failed: {exc}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
