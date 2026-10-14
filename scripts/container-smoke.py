#!/usr/bin/env python3
"""Linux/WSL: exercise the built image in disposable volumes and loopback ports."""
import argparse
import json
import io
import pathlib
import select
import subprocess
import tempfile
import tarfile
import time
import urllib.request
import uuid

PASSWORD = "Disposable-Container-Test-Password"


def command(*args):
    result = subprocess.run(["docker", *args], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(f"Docker {args[0]} failed: {result.stderr.strip()}")
    return result.stdout.strip()


def terminal_command(args):
    process = subprocess.Popen(["docker", *args], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, bufsize=0)
    output = b""
    sent = 0
    deadline = time.monotonic() + 30
    try:
        while time.monotonic() < deadline:
            ready, _, _ = select.select([process.stdout], [], [], 0.2)
            if ready:
                chunk = process.stdout.read(8192)
                if not chunk:
                    break
                output += chunk
                for i, prompt in enumerate([b"New password:", b"Confirm password:"]):
                    if sent == i and prompt in output:
                        time.sleep(0.02)
                        process.stdin.write(PASSWORD.encode() + b"\n")
                        process.stdin.flush()
                        sent += 1
        else:
            process.terminate()
            raise RuntimeError("Interactive CLI timed out")
    finally:
        process.stdin.close()
        process.stdout.close()
        process.wait(timeout=5)
    if process.returncode != 0:
        raise RuntimeError("Interactive command failed: " + output.decode(errors="replace").replace(PASSWORD, "[redacted]")[:2000])
    assert PASSWORD.encode() not in output, "Hidden password was echoed"
    assert b"Created account" in output
    return "passed"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", default="rustdesk-account-server:0.1.0")
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    name = "rustdesk-account-smoke-" + uuid.uuid4().hex[:12]
    data, keys = name + "-data", name + "-keys"
    containers = []
    volumes = []
    try:
        for volume in [data, keys]:
            command("volume", "create", volume)
            volumes.append(volume)
        mounts = ["--mount", f"type=volume,src={data},dst=/data", "--mount", f"type=volume,src={keys},dst=/secrets"]
        helper = name + "-prepare"
        command("create", "--name", helper, "--user", "0:0", *mounts, "--entrypoint", "sh", args.image,
                "-c", "chown 10001:10001 /data /secrets /data/account.toml && chmod 700 /data /secrets")
        containers.append(helper)
        with tempfile.TemporaryDirectory(prefix=name) as temporary:
            config = pathlib.Path(temporary) / "account.toml"
            config.write_text('listen="0.0.0.0:21114"\ndatabase="/data/accounts.sqlite3"\nmaster_key="/secrets/master.key"\n')
            archive = io.BytesIO()
            with tarfile.open(fileobj=archive, mode="w") as tar:
                tar.add(config, arcname="account.toml")
            # Streaming avoids Docker Desktop/WSL host-path translation differences.
            subprocess.run(["docker", "cp", "-", f"{helper}:/data"], input=archive.getvalue(), check=True, capture_output=True)
        command("start", "--attach", helper)
        command("run", "--rm", *mounts, args.image, "--config", "/data/account.toml", "init")
        # Allocate the CLI terminal inside the container; works even with Docker's
        # Windows CLI called from WSL, which cannot inherit a Linux controlling TTY.
        result = {"interactive_hidden_password": terminal_command(["run", "--rm", "-i", *mounts, "--entrypoint", "script", args.image, "--quiet", "--return", "--command", "rustdesk-account-server --config /data/account.toml user create smoke", "/dev/null"])}
        readonly_mounts = ["--mount", f"type=volume,src={data},dst=/data", "--mount", f"type=volume,src={keys},dst=/secrets,readonly"]
        command("run", "-d", "--name", name, "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges:true", "--memory", "128m", "--pids-limit", "32", "-p", "127.0.0.1::21114", *readonly_mounts, args.image, "--config", "/data/account.toml", "serve")
        containers.append(name)
        info = json.loads(command("inspect", name))[0]
        assert info["Config"]["User"] == "10001:10001"
        assert info["HostConfig"]["ReadonlyRootfs"]
        assert info["HostConfig"]["CapDrop"] == ["ALL"]
        assert "no-new-privileges:true" in info["HostConfig"]["SecurityOpt"]
        assert next(m for m in info["Mounts"] if m["Destination"] == "/secrets")["RW"] is False
        port = info["NetworkSettings"]["Ports"]["21114/tcp"][0]["HostPort"]
        base = "http://127.0.0.1:" + port
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

        def request(path, body=None, token=None, method="POST"):
            payload = None if body is None else json.dumps(body).encode()
            headers = {} if token is None else {"Authorization": f"Bearer {token}"}
            with opener.open(urllib.request.Request(base + path, payload, headers, method=method), timeout=5) as response:
                raw = response.read()
                return json.loads(raw) if raw and raw != b"ok" else None

        for _ in range(100):
            try:
                request("/healthz", method="GET")
                break
            except OSError:
                time.sleep(0.1)
        else:
            raise RuntimeError("Container health check failed")
        token = request("/api/login", {"username": "smoke", "password": PASSWORD, "id": "800000", "uuid": "disposable-machine", "type": "account"})["access_token"]
        book = request("/api/ab/personal", token=token)["guid"]
        request(f"/api/ab/peer/add/{book}", {"id": "800001", "hash": "disposable-container-credential"}, token)
        assert request(f"/api/ab/peers?ab={book}", token=token)["data"][0]["hash"] == "disposable-container-credential"
        command("exec", name, "rustdesk-account-server", "--config", "/data/account.toml", "backup", "/data/consistent-backup.sqlite3")
        command("restart", name)
        # Docker may allocate a different host port when an ephemeral binding restarts.
        restarted = json.loads(command("inspect", name))[0]
        base = "http://127.0.0.1:" + restarted["NetworkSettings"]["Ports"]["21114/tcp"][0]["HostPort"]
        for _ in range(100):
            try:
                assert request("/api/currentUser", token=token)["name"] == "smoke"
                break
            except OSError:
                time.sleep(0.1)
        else:
            raise RuntimeError("Restart session persistence failed")
        log = command("logs", name)
        for secret in [PASSWORD, token, "disposable-container-credential"]:
            assert secret not in log
        result.update({"image_id": info["Image"], "user": info["Config"]["User"], "read_only_root": True, "read_only_key_mount": True, "no_new_privileges": True, "capabilities_dropped": "ALL", "loopback_http_login_and_address_book": "passed", "restart_session_persistence": "passed", "online_backup_cli": "passed", "server_log_secret_check": "passed"})
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2))
    finally:
        for container in reversed(containers):
            subprocess.run(["docker", "rm", "-f", container], capture_output=True)
        for volume in reversed(volumes):
            subprocess.run(["docker", "volume", "rm", volume], capture_output=True)


if __name__ == "__main__":
    main()
