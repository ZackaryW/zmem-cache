## 1. Shape Public Behavior

- [x] 1.1 Add an independently runnable `service-distribution` Behave root that exercises complete manifest assembly and malformed-input rejection through the public utility command
- [x] 1.2 Add focused unit tests for strict identity input, safe asset records, complete target coverage, and canonical manifest output
- [x] 1.3 Run focused tests and record the expected RED before implementation

## 2. Manifest Utility

- [x] 2.1 Implement the standard-library release manifest assembly command with strict typed JSON input validation
- [x] 2.2 Enforce version, protocol, schema, target coverage, filename safety, byte length, SHA-256, and canonical JSON output
- [x] 2.3 Run focused unit tests and the `service-distribution` BDD root GREEN independently

## 3. Release Workflow

- [x] 3.1 Add the Saucepan-derived macOS, Windows, and Linux build matrix with target-based artifact names and captured identity metadata
- [x] 3.2 Add Linux ARM64 cross-compilation and require the complete supported target set before publication
- [x] 3.3 Assemble `release-manifest.json`, verify the tag and workspace version, and publish all assets in one GitHub Release job with scoped permissions
- [x] 3.4 Document artifact names, manifest shape, version coordination, and local workflow validation

## 4. Verification

- [x] 4.1 Check Cargo and uv locks plus supported Rust and Python toolchains
- [x] 4.2 Run Rustfmt, Clippy with warnings denied, the complete Rust workspace tests, and a locked release build
- [x] 4.3 Run Ruff and every capability-owned BDD root independently
- [x] 4.4 Validate workflow structure and strict JSON generation without publishing a release
- [x] 4.5 Strictly validate the OpenSpec change
