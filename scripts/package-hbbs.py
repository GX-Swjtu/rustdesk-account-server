#!/usr/bin/env python3
"""Package only the independent hbbs patch and its verified Linux binary."""
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import zipfile

root = Path(__file__).resolve().parent.parent
repo = root.parent / 'rustdesk-server'
dist = root / 'dist'
existing = ['src/lib.rs', 'src/rendezvous_server.rs']
added = ['src/account_handshake.rs', 'src/account_handshake_tests.rs', 'docs/account-handshake.md']
patch = subprocess.check_output(['git', '-C', str(repo), 'diff', 'HEAD', '--', *existing]).decode()
for name in added:
    lines = (repo / name).read_text(encoding='utf-8').splitlines()
    patch += f'diff --git a/{name} b/{name}\nnew file mode 100644\n--- /dev/null\n+++ b/{name}\n@@ -0,0 +1,{len(lines)} @@\n'
    patch += ''.join('+' + line + '\n' for line in lines)
patch_file = dist / 'hbbs-account-handshake.patch'
patch_file.write_text(patch, encoding='utf-8', newline='\n')
with tempfile.TemporaryDirectory(prefix='hbbs-patch-check-') as directory:
    checkout = Path(directory)
    for name in existing:
        target = checkout / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(subprocess.check_output(['git', '-C', str(repo), 'show', 'HEAD:'+name]))
    subprocess.run(['git', 'apply', '--check', str(patch_file)], cwd=checkout, check=True)
binary = root.parent / 'artifacts/hbbs-account-20261014'
manifest = {
    'server_base_commit': subprocess.check_output(['git','-C',str(repo),'rev-parse','HEAD'],text=True).strip(),
    'hbb_common_commit': subprocess.check_output(['git','-C',str(repo / 'libs/hbb_common'),'rev-parse','HEAD'],text=True).strip(),
    'binary_sha256': hashlib.file_digest(binary.open('rb'), 'sha256').hexdigest(),
    'patch_sha256': hashlib.sha256(patch_file.read_bytes()).hexdigest(),
    'changed_existing_files': existing,
    'added_files': added,
    'enable_environment': 'HBBS_ACCOUNT_HANDSHAKE=Y',
    'patch_apply_check': 'passed',
    'tests': '14 library tests; public plain/encrypted NAT and native P2P coordination; user confirmed Windows relay/P2P connection',
}
archive = dist / 'hbbs-account-handshake-20261014.zip'
with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as output:
    output.write(binary, 'hbbs-account-linux-x86_64')
    output.write(patch_file, patch_file.name)
    for name in added:
        output.write(repo / name, name)
    output.writestr('manifest.json', json.dumps(manifest, indent=2)+'\n')
    output.write(root / 'scripts/probe-hbbs.py', 'probe-hbbs.py')
print(json.dumps({'archive':str(archive), **manifest}, indent=2))
