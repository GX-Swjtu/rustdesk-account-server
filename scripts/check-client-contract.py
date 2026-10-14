#!/usr/bin/env python3
"""Source contract evidence, not a replacement for real client UAT."""
import argparse
import hashlib
import json
import pathlib
import urllib.request

FILES = {
    "flutter/lib/models/user_model.dart": ["/api/login", "/api/logout", "/api/currentUser"],
    "flutter/lib/models/group_model.dart": ["/api/users", "/api/peers", "/api/device-group/accessible"],
    "flutter/lib/models/ab_model.dart": ["/api/ab/personal", "/api/ab/settings", "/api/ab/shared/profiles", "/api/ab/peers", "/api/ab/tags/", "/api/ab/peer/add/", "/api/ab/peer/update/", "/api/ab/peer/", "/api/ab/tag/add/", "/api/ab/tag/rename/", "/api/ab/tag/update/", "/api/ab/tag/"],
    "flutter/lib/common/hbbs/hbbs.dart": ["access_token", "display_name", "deviceInfo"],
    "flutter/lib/models/peer_model.dart": ["forceAlwaysRelay", "rdpUsername", "loginName", "same_server"],
    "src/hbbs_http/sync.rs": ["SYSINFO_UPDATED", "ID_NOT_FOUND", '"sysinfo"', '"heartbeat"'],
}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--local-client", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    results = []
    # Pin the verified official 1.5.0 commit, not a moving branch.
    commit = "fada664df7a294d1d1a9ca3e7cd3637069122f17"
    for path, markers in FILES.items():
        for baseline in ["official-1.5.0", "local-1.5.1"]:
            if baseline.startswith("official"):
                source = f"https://raw.githubusercontent.com/rustdesk/rustdesk/{commit}/{path}"
                with urllib.request.urlopen(source, timeout=30) as response:
                    data = response.read()
            else:
                source = str(args.local_client / path)
                data = pathlib.Path(source).read_bytes()
            text = data.decode("utf-8")
            missing = [marker for marker in markers if marker not in text]
            if missing:
                raise RuntimeError(f"{baseline} {path}: missing {missing}")
            results.append({"baseline": baseline, "source": source, "sha256": hashlib.sha256(data).hexdigest(), "markers_checked": markers})
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps({"kind": "source-contract-only", "files": results}, indent=2) + "\n")
    print(f"Verified {len(results)} source files; real client interaction still requires UAT.")


if __name__ == "__main__":
    main()
