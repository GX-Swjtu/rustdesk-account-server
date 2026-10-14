#!/usr/bin/env python3
"""Package an explicit allowlist; never include databases, keys or local configuration."""
import datetime
import hashlib
import json
import pathlib
import subprocess
import zipfile

root = pathlib.Path(__file__).resolve().parent.parent
dist = root / "dist"
version = "0.1.0"
files = [root / name for name in ["Cargo.toml", "Cargo.lock", "Dockerfile", ".dockerignore", ".gitignore", "README.md", "LICENSE", "config.example.toml"]]
for directory, pattern in [("src", "*.rs"), ("tests", "*.rs"), ("examples", "*.rs"), ("scripts", "*.py"), ("docs", "*.md")]:
    files.extend((root / directory).rglob(pattern))
files.extend(root / "deploy" / name for name in ["compose.yaml", "account.toml", "Caddyfile", ".env.example"])
files.extend(path for path in (root / "deploy" / "systemd").rglob("*") if path.is_file())
files.extend(root / "test-results" / name for name in ["client-source-contract.json", "resources-linux.json", "dependency-audit.json", "container-smoke.json", "public-deployment.json", "hbbs-native-public.json", "deployment-final.json"])
archive = dist / f"rustdesk-account-server-{version}-source.zip"
with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as target:
    for path in sorted(set(files)):
        target.write(path, "rustdesk-account-server/" + path.relative_to(root).as_posix())


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


assets = [dist / "rustdesk-account-server-linux-x86_64", dist / f"rustdesk-account-server-{version}-image.tar", archive, dist / "hbbs-account-handshake.patch", dist / "hbbs-account-handshake-20261014.zip"]
image_id = subprocess.run(["docker", "image", "inspect", f"rustdesk-account-server:{version}", "--format", "{{.Id}}"], check=True, capture_output=True, text=True).stdout.strip()
manifest = {"version": version, "packaged_at": datetime.datetime.now(datetime.timezone.utc).isoformat(), "image_id": image_id, "cargo_lock_sha256": digest(root / "Cargo.lock"), "assets": [{"name": path.name, "bytes": path.stat().st_size, "sha256": digest(path)} for path in assets]}
manifest_path = dist / "manifest.json"
manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
(dist / "SHA256SUMS").write_text("".join(f"{digest(path)}  {path.name}\n" for path in [*assets, manifest_path]))
print(json.dumps(manifest, indent=2))
