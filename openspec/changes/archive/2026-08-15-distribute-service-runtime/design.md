## Context

The Rust workspace exposes release, protocol, and schema identity through `zmem-svc version-json`, but has no distributable artifact contract. Saucepan's tag-driven matrix demonstrates the repository owner's established macOS, Windows, Linux, artifact-staging, and GitHub Release pattern. The coordinated Python client requires stronger typed metadata and Linux ARM64 coverage.

## Goals / Non-Goals

**Goals:**

- Produce a deterministic artifact set consumable without filename guessing.
- Fail release assembly when tag, workspace, binary, or manifest identity diverges.
- Keep manifest construction executable and testable outside GitHub Actions.

**Non-Goals:**

- Publishing the Python `zmem` distribution from this repository.
- Supporting targets not declared in the capability contract.
- Performing a real GitHub publication during local or pull-request verification.

## Decisions

### Reuse Saucepan's build-and-stage release topology

Separate macOS, Windows, and Linux jobs build target-specific binaries and upload workflow artifacts. A final release job downloads the complete set, verifies identity, generates metadata, and publishes all files together. The matrix extends Saucepan with Linux ARM64 and omits its optional macOS universal artifact because clients select an explicit architecture.

### Name assets by Rust target

Assets are named `zmem-svc-<target>` with `.exe` only for Windows. Target-based naming is stable across runner labels and matches the client platform parser. The manifest, rather than filename parsing, remains the downstream selection authority.

### Generate the manifest with a repository-owned standard-library utility

A small Python utility accepts the expected release version and staged `target=path` inputs, runs each artifact's `version-json` when executable on the current host or consumes already captured typed identity metadata from the build job, validates all fields, streams SHA-256, and emits canonical JSON. Pure parsing and validation remain unit tested; one capability BDD invokes the utility through its command boundary.

Because the final Linux job cannot execute Windows or macOS binaries, every build job captures `version-json` on its native runner before upload. Cross-compiled artifacts use the workspace identity injected at build time, and final assembly requires every captured identity document to agree.

### Treat the release as an all-or-nothing target set

The publication job requires every declared artifact and identity document. No partial GitHub Release is created when one platform build or validation fails. `contents: write` is scoped to the final release job; build jobs use read-only contents access.

### Use maintained native build surfaces first

Native GitHub runners and stable Rust toolchains build macOS and Windows targets following Saucepan. Linux x86-64 uses the native musl toolchain. Linux ARM64 uses a maintained cross-compilation action/container only for the target that cannot be linked by the native runner; the selected action is pinned to a release or commit rather than floating at an unbounded branch.

## Risks / Trade-offs

- [Cross-compiled Linux ARM64 cannot be executed on the x86-64 runner] → validate its compiled workspace identity metadata and exercise the artifact on an ARM64 runner when GitHub-hosted availability permits.
- [GitHub Action or Rust toolchain changes can break a platform independently] → require the whole matrix before publication and keep target builds isolated.
- [Repository-provided identity metadata could drift from the binary] → generate native identities by executing binaries where possible and retain client-side `version-json` validation after download.

## Migration Plan

1. Land the workflow and manifest utility without creating a release.
2. Run workflow dispatch against the intended version tag after repository remotes and release permissions are configured.
3. Publish the same-version Python client only after the cache release is complete.
4. Delete or replace a failed GitHub Release; no client selects a different version automatically.
