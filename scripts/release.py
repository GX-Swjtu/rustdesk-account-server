"""CI release packaging. Includes only source-controlled deployment examples."""
import hashlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import tarfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
REPO = "GX-Swjtu/rustdesk-account-server"
VERSION = re.search(r'^version = "([^"]+)"', (ROOT / "Cargo.toml").read_text(), re.M)[1]
DIST = ROOT / "dist/release"


def gh(*args):
    return subprocess.check_output(["gh", *args], text=True).strip()


def validate():
    tag = os.environ["RELEASE_TAG"]
    if tag != "v" + VERSION or not re.fullmatch(r"v\d+\.\d+\.\d+", tag):
        raise ValueError("Release tag must match Cargo.toml: v" + VERSION)
    pages = json.loads(gh("api", f"repos/{REPO}/releases", "--paginate", "--slurp"))
    existing = next((r for page in pages for r in page if r["tag_name"] == tag), None)
    if existing and not existing["draft"]:
        raise ValueError("Refusing to overwrite a published release")
    return tag, existing


def package():
    target = os.environ["RELEASE_TARGET"]
    binary = "rustdesk-account-server" + (".exe" if "windows" in target else "")
    files = [(ROOT / "target/release" / binary, binary)]
    for name in ["LICENSE", "README.md", "config.example.toml", "docs/DEPLOYMENT.md", "docs/RELEASE.md"]:
        files.append((ROOT / name, name))
    for name in subprocess.check_output(["git", "ls-files", "deploy"], cwd=ROOT, text=True).splitlines():
        files.append((ROOT / name, name))
    compose = (ROOT / "deploy/compose.yaml").read_text(encoding="utf-8")
    compose, build_count = re.subn(r"(?m)^    build:\n      context: \.\.\n", "", compose)
    compose, image_count = re.subn(r"(?m)^    image: rustdesk-account-server:[^\n]+$",
        f"    image: rustdesk-account-server:{VERSION}\n    pull_policy: never", compose)
    if build_count != 1 or image_count != 1:
        raise ValueError("Deployment template changed; cannot prepare the release image configuration")
    compose = compose.encode("utf-8")
    DIST.mkdir(parents=True, exist_ok=True)
    stem = DIST / f"rustdesk-account-server-{VERSION}-{target}"
    if "windows" in target:
        with zipfile.ZipFile(str(stem) + ".zip", "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for path, name in files:
                if name == "deploy/compose.yaml":
                    archive.writestr(name, compose)
                else:
                    archive.write(path, name)
    else:
        with tarfile.open(str(stem) + ".tar.gz", "w:gz") as archive:
            for path, name in files:
                if name == "deploy/compose.yaml":
                    info = archive.gettarinfo(path, arcname=name)
                    info.size = len(compose)
                    archive.addfile(info, io.BytesIO(compose))
                else:
                    archive.add(path, arcname=name)


def draft():
    tag, existing = validate()
    files = sorted(p for p in DIST.iterdir() if p.name.endswith((".zip", ".tar.gz")))
    if len(files) != 6:
        raise ValueError("Expected five native packages and one container archive")
    sums = []
    for file in files:
        digest = hashlib.sha256()
        with file.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1048576), b""):
                digest.update(chunk)
        sums.append(f"{digest.hexdigest()}  {file.name}\n")
    checksum = DIST / "SHA256SUMS"
    checksum.write_text("".join(sums), encoding="utf-8")
    body = DIST / "release-notes.md"
    warning = (ROOT / ".github/RELEASE_WARNING.md").read_text(encoding="utf-8")
    body.write_text(warning + f"\n# RustDesk Account Server {VERSION}\n\n独立兼容账号服务，不是官方 Server Pro。Independent compatible account API, not official Server Pro.\n\n"
        + "本流程只生成发布草稿，不自动升级线上服务或数据库。This workflow creates a draft only; it does not update running services or databases.\n\n"
        + "Linux 原生包需要 Ubuntu 24.04 或相当版本；较旧主机请使用容器包。Linux native packages require Ubuntu 24.04 or equivalent; use the container archive for older hosts.\n\n"
        + "登录后远控仍依赖支持账号加密握手的 hbbs。Remote control after account login still requires compatible hbbs account-handshake support.\n\n"
        + "## Downloads / 下载\n" + "\n".join(f"- [{p.name}](https://github.com/{REPO}/releases/download/{tag}/{p.name})" for p in files)
        + "\n\nVerify SHA-256 using `SHA256SUMS`. Review `docs/DEPLOYMENT.md` and back up the database and master key before upgrading.\n", encoding="utf-8")
    if not existing:
        gh("release", "create", tag, "--repo", REPO, "--verify-tag", "--draft", "--title", "Account server " + VERSION, "--notes-file", str(body))
    else:
        gh("release", "edit", tag, "--repo", REPO, "--draft", "--notes-file", str(body))
    gh("release", "upload", tag, *map(str, files + [checksum]), "--repo", REPO, "--clobber")


if __name__ == "__main__":
    import sys
    {"validate": validate, "package": package, "draft": draft}[sys.argv[1]]()
