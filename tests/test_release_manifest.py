from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from scripts.release_manifest import REQUIRED_TARGETS, assemble_manifest, render_manifest


class ReleaseManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def inputs(self):
        artifacts = {}
        identities = {}
        for index, target in enumerate(REQUIRED_TARGETS):
            suffix = ".exe" if target.endswith("windows-msvc") else ""
            path = self.root / f"zmem-svc-{target}{suffix}"
            path.write_bytes(f"artifact-{index}".encode())
            artifacts[target] = path
            identities[target] = {
                "release_version": "1.0.1",
                "protocol_version": 2,
                "schema_version": 2,
            }
        return artifacts, identities

    def test_complete_manifest_is_typed_and_canonical(self) -> None:
        artifacts, identities = self.inputs()
        manifest = assemble_manifest("1.0.1", artifacts, identities)
        self.assertEqual(manifest["manifest_version"], 1)
        self.assertEqual([asset["target"] for asset in manifest["assets"]], sorted(REQUIRED_TARGETS))
        rendered = render_manifest(manifest)
        self.assertEqual(json.loads(rendered), manifest)
        self.assertTrue(rendered.endswith("\n"))

    def test_missing_target_is_rejected(self) -> None:
        artifacts, identities = self.inputs()
        artifacts.pop(next(iter(REQUIRED_TARGETS)))
        with self.assertRaisesRegex(ValueError, "target coverage"):
            assemble_manifest("1.0.1", artifacts, identities)

    def test_identity_is_strict_and_coherent(self) -> None:
        artifacts, identities = self.inputs()
        target = next(iter(REQUIRED_TARGETS))
        identities[target] = identities[target] | {"unknown": True}
        with self.assertRaisesRegex(ValueError, "unknown"):
            assemble_manifest("1.0.1", artifacts, identities)

        artifacts, identities = self.inputs()
        identities[target]["release_version"] = "0.2.0"
        with self.assertRaisesRegex(ValueError, "release version"):
            assemble_manifest("1.0.1", artifacts, identities)

    def test_asset_name_must_match_target(self) -> None:
        artifacts, identities = self.inputs()
        target = next(iter(REQUIRED_TARGETS))
        wrong = self.root / "../wrong-name"
        wrong.resolve().write_bytes(b"wrong")
        artifacts[target] = wrong.resolve()
        with self.assertRaisesRegex(ValueError, "asset name"):
            assemble_manifest("1.0.1", artifacts, identities)


if __name__ == "__main__":
    unittest.main()
