import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

spec = importlib.util.spec_from_file_location("release", Path(__file__).with_name("release.py"))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseTests(unittest.TestCase):
    def test_native_packages_use_the_matching_prebuilt_container_without_source_builds(self):
        tracked = subprocess.check_output(["git", "ls-files", "deploy"], cwd=release.ROOT, text=True)
        originals = ["LICENSE", "README.md", "config.example.toml", "docs/DEPLOYMENT.md", "docs/RELEASE.md"]
        originals += tracked.splitlines()
        source_compose = (release.ROOT / "deploy/compose.yaml").read_bytes()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in originals:
                destination = root / name
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(release.ROOT / name, destination)
            (root / "target/release").mkdir(parents=True)
            for binary in ["rustdesk-account-server", "rustdesk-account-server.exe"]:
                (root / "target/release" / binary).write_bytes(b"test binary")
            for target in ["x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"]:
                with self.subTest(target=target), patch.object(release, "ROOT", root), \
                        patch.object(release, "DIST", root / "dist/release"), \
                        patch.dict(os.environ, {"RELEASE_TARGET": target}), \
                        patch.object(release.subprocess, "check_output", return_value=tracked):
                    release.package()
                stem = root / "dist/release" / f"rustdesk-account-server-{release.VERSION}-{target}"
                if "windows" in target:
                    with zipfile.ZipFile(str(stem) + ".zip") as archive:
                        compose = archive.read("deploy/compose.yaml")
                else:
                    with tarfile.open(str(stem) + ".tar.gz") as archive:
                        compose = archive.extractfile("deploy/compose.yaml").read()
                # Resolve the shipped configuration as Compose will use it; no daemon or container starts.
                config = json.loads(subprocess.check_output([
                    "docker", "compose", "--project-directory", str(root / "deploy"),
                    "--env-file", str(root / "deploy/.env.example"), "-f", "-", "config", "--format", "json",
                ], input=compose))
                account = config["services"]["account"]
                self.assertEqual(account["image"], "rustdesk-account-server:" + release.VERSION)
                self.assertEqual(account["pull_policy"], "never")
                self.assertNotIn("build", account)
            self.assertEqual((root / "deploy/compose.yaml").read_bytes(), source_compose)


if __name__ == "__main__":
    unittest.main()
