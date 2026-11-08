import json
import re
import tomllib
import urllib.request
from pathlib import Path

root = Path.cwd()
lock = tomllib.loads((root / 'Cargo.lock').read_text())
uuid = [p for p in lock['package'] if p['name'] == 'uuid']
if len(uuid) != 1:
    raise SystemExit(f'expected exactly one locked uuid package, found {len(uuid)}')
locked = uuid[0]
manifest = tomllib.loads((root / 'gluco-hub/Cargo.toml').read_text())
req = manifest.get('dev-dependencies', {}).get('uuid', {}).get('version')
if not isinstance(req, str) or req.startswith('=') or req != '1':
    raise SystemExit(f'dev-dependency uuid requirement is not the expected compatible range: {req!r}')
request = urllib.request.Request('https://index.crates.io/uu/id/uuid', headers={'User-Agent': 'gluco-hub-rs-issue-142-qa'})
with urllib.request.urlopen(request, timeout=20) as response:
    lines = response.read().decode('utf-8').splitlines()
records = [json.loads(line) for line in lines if line]
record = next((entry for entry in records if entry.get('vers') == locked['version']), None)
if record is None:
    raise SystemExit(f'uuid {locked["version"]} is absent from current crates.io index')
if record.get('yanked') is not False:
    raise SystemExit(f'uuid {locked["version"]} yanked={record.get("yanked")!r}')
if record.get('cksum') != locked['checksum']:
    raise SystemExit('registry checksum differs from Cargo.lock')
rust = record.get('rust_version')
if rust and tuple(map(int, rust.split('.'))) > (1, 95):
    raise SystemExit(f'uuid {locked["version"]} requires Rust {rust}, above project MSRV 1.95')
stable = []
for entry in records:
    version = entry.get('vers', '')
    match = re.fullmatch(r'1\.(\d+)\.(\d+)', version)
    if match and entry.get('yanked') is False:
        stable.append((int(match.group(1)), int(match.group(2)), version))
latest = max(stable)
if locked['version'] != latest[2]:
    raise SystemExit(f'locked uuid {locked["version"]} is not current non-yanked stable uuid 1.x ({latest[2]})')
print(f'PASS: uuid {locked["version"]}; manifest requirement {req!r}; current latest non-yanked stable 1.x; yanked=false; rust_version={rust}; checksum matches Cargo.lock')
