#!/usr/bin/env python3
"""Linux-only, loopback-only disposable API resource/protocol exercise. No third-party Python packages."""
import argparse
import concurrent.futures
import json
import pathlib
import socket
import sqlite3
import subprocess
import tempfile
import threading
import time
import urllib.request

PASSWORD = "Disposable-Load-Test-Password-Only"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--fixture-binary", type=pathlib.Path, required=True)
    parser.add_argument("--seconds", type=int, default=180)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    if args.seconds < 60:
        parser.error("use at least 60 seconds to exercise heartbeat persistence")
    result = {"devices": 15, "duration_seconds": args.seconds, "scope": "API RSS only; excludes Caddy/hbbs/hbbr"}
    with tempfile.TemporaryDirectory(prefix="rustdesk-account-measure-") as temporary:
        root = pathlib.Path(temporary)
        fixture = root / "fixture"
        subprocess.run([str(args.fixture_binary.resolve()), str(fixture)], check=True, capture_output=True)
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        config = fixture / "account.toml"
        config.write_text(config.read_text().replace('127.0.0.1:21114', f'127.0.0.1:{port}'))
        base = f"http://127.0.0.1:{port}"
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        server_log = root / "server.log"
        with server_log.open("wb") as log:
            process = subprocess.Popen([str(args.binary.resolve()), "--config", str(config), "serve"], stdout=log, stderr=log)
            stop = threading.Event()
            samples = []

            def sample():
                while not stop.is_set():
                    try:
                        status = pathlib.Path(f"/proc/{process.pid}/status").read_text()
                        values = {line.split(":")[0]: int(line.split()[1]) for line in status.splitlines() if line.startswith(("VmRSS:", "VmHWM:"))}
                        samples.append((time.monotonic(), values["VmRSS"], values["VmHWM"]))
                    except (OSError, KeyError):
                        pass
                    stop.wait(0.005)

            def request(path, body=None, token=None, method="POST"):
                raw = None if body is None else json.dumps(body).encode()
                headers = {"Authorization": f"Bearer {token}"} if token else {}
                # No JSON content type: mimics Dart's login behavior.
                req = urllib.request.Request(base + path, raw, headers, method=method)
                with opener.open(req, timeout=10) as response:
                    payload = response.read()
                    assert response.status == 200
                    try:
                        return json.loads(payload) if payload else None
                    except json.JSONDecodeError:
                        return payload.decode()

            thread = threading.Thread(target=sample, daemon=True)
            thread.start()
            try:
                for _ in range(200):
                    if process.poll() is not None:
                        raise RuntimeError("API exited during startup")
                    try:
                        request("/healthz", method="GET")
                        break
                    except OSError:
                        time.sleep(0.05)
                else:
                    raise RuntimeError("API did not start")
                time.sleep(1)
                result["idle_rss_kib"] = samples[-1][1]
                login_start = time.monotonic()
                tokens = []
                for n in range(15):
                    response = request("/api/login", {"username": "loadtest", "password": PASSWORD, "type": "account", "autoLogin": True, "id": str(800000+n), "uuid": f"disposable-machine-{n}", "deviceInfo": {"os": "linux", "name": f"load-{n}"}})
                    tokens.append(response["access_token"])
                result["login_peak_rss_kib"] = max(s[1] for s in samples if s[0] >= login_start)
                result["login_15_duration_ms"] = round((time.monotonic()-login_start)*1000)
                assert request("/api/peers", token=tokens[0], method="GET")["total"] == 15
                book = request("/api/ab/personal", token=tokens[0])["guid"]
                for n in range(15):
                    request(f"/api/ab/peer/add/{book}", {"id": str(800000+n), "hash": f"disposable-credential-{n}"}, tokens[0])
                    request("/api/sysinfo", {"id": str(800000+n), "uuid": f"disposable-machine-{n}", "hostname": f"load-{n}", "os": "Linux"})
                started = time.monotonic()
                latencies = []
                rounds = 0

                def exercise(n):
                    t = time.monotonic()
                    request("/api/heartbeat", {"id": str(800000+n), "uuid": f"disposable-machine-{n}", "ver": 1005000, "conns": [1], "modified_at": 0})
                    return (time.monotonic()-t)*1000

                with concurrent.futures.ThreadPoolExecutor(max_workers=15) as pool:
                    while time.monotonic()-started < args.seconds:
                        round_start = time.monotonic()
                        latencies.extend(pool.map(exercise, range(15)))
                        request(f"/api/ab/peer/update/{book}", {"id": "800000", "note": f"round {rounds}"}, tokens[rounds % 15], method="PUT")
                        peers = request(f"/api/ab/peers?ab={book}&current=1&pageSize=100", token=tokens[(rounds+1) % 15])
                        assert peers["total"] == 15 and peers["data"][0]["note"] == f"round {rounds}"
                        rounds += 1
                        stop.wait(max(0, 3-(time.monotonic()-round_start)))
                result["rounds"] = rounds
                result["heartbeat_requests"] = len(latencies)
                result["heartbeat_latency_p95_ms"] = round(sorted(latencies)[int(len(latencies)*.95)-1], 3)
                result["steady_peak_rss_kib"] = max(s[1] for s in samples if s[0] >= started)
                result["end_rss_kib"] = samples[-1][1]
                result["process_high_water_kib"] = max(s[2] for s in samples)
                result["limits_pass"] = result["idle_rss_kib"] <= 65536 and result["login_peak_rss_kib"] <= 131072
                with sqlite3.connect(fixture / "accounts.sqlite3") as database:
                    result["database_integrity"] = database.execute("PRAGMA integrity_check").fetchone()[0]
                    result["devices_persisted"] = database.execute("SELECT count(*) FROM devices WHERE last_seen > 0").fetchone()[0]
                result["errors"] = 0
            finally:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
                stop.set()
                thread.join(timeout=2)
            log_text = server_log.read_text()
            assert PASSWORD not in log_text
            assert all(token not in log_text for token in tokens)
            assert "disposable-credential" not in log_text
            result["server_log_secret_check"] = "passed"
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
