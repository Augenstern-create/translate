#!/usr/bin/env bash
# windows-recon.sh -- run tools/p5/windows-recon.ps1 on a remote Windows host
# over OpenSSH and store the JSON locally, validated.
#
# The PowerShell script is streamed over stdin (`powershell -Command -`), so
# nothing is written to the remote disk and there is no command-line length
# limit to fight.
#
# Usage:
#   tools/p5/windows-recon.sh nan@192.168.1.20
#   tools/p5/windows-recon.sh nan@host -p 2222 -o /tmp/win-recon.json
#
# Exit codes: 0 = JSON captured and parsed; 2 = ssh ok but output unusable;
#             3 = precondition missing (ssh/ps1/python3).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ps1="$here/windows-recon.ps1"
port=22
out=""

usage() { sed -n '2,12p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

target="${1:-}"
shift || true
while [ $# -gt 0 ]; do
  case "$1" in
    -p|--port) port="$2"; shift 2 ;;
    -o|--out)  out="$2";  shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 3 ;;
  esac
done

if [ -z "$target" ]; then usage >&2; exit 3; fi
command -v ssh     >/dev/null || { echo "ssh not found"     >&2; exit 3; }
command -v python3 >/dev/null || { echo "python3 not found" >&2; exit 3; }
[ -f "$ps1" ] || { echo "missing $ps1" >&2; exit 3; }

raw="$(mktemp)"
trap 'rm -f "$raw"' EXIT

# ConnectTimeout keeps a wrong IP from hanging the caller; BatchMode=yes fails
# fast instead of prompting when no key/agent is available.
ssh -p "$port" \
    -o BatchMode=yes \
    -o ConnectTimeout=10 \
    -o StrictHostKeyChecking=accept-new \
    "$target" \
    "powershell -NoProfile -ExecutionPolicy Bypass -Command -" < "$ps1" > "$raw" 2>&1 || true

if [ -z "$out" ]; then
  out="win-recon-$(date -u +%Y%m%dT%H%M%SZ).json"
fi

# Marker extraction + decode + validation in one python step: a Chinese-locale
# Windows console can hand us CP936 bytes even though the script asks for UTF-8.
python3 - "$raw" "$out" <<'PY' || exit 2
import json, sys

raw_path, out_path = sys.argv[1], sys.argv[2]
data = open(raw_path, "rb").read()

text, used = None, None
for enc in ("utf-8", "cp936", "cp1252"):
    try:
        text, used = data.decode(enc), enc
        break
    except UnicodeDecodeError:
        continue
if text is None:
    print("FAIL: remote output is decodable as neither utf-8 nor cp936", file=sys.stderr)
    sys.exit(2)

inside, block = False, []
for line in text.splitlines():
    s = line.strip()
    if s == "DSH-RECON-BEGIN":
        inside, block = True, []
        continue
    if s == "DSH-RECON-END":
        break
    if inside:
        block.append(line)

if not block:
    print("FAIL: no DSH-RECON-BEGIN/END block in remote output. Raw output follows:", file=sys.stderr)
    print(text[:2000], file=sys.stderr)
    sys.exit(2)

try:
    doc = json.loads("\n".join(block))
    assert doc["schema"] == "dsh-win-recon/v1"
except Exception as exc:
    print("FAIL: captured block is not valid dsh-win-recon/v1 JSON: %s" % exc, file=sys.stderr)
    print("\n".join(block)[:2000], file=sys.stderr)
    sys.exit(2)

with open(out_path, "w", encoding="utf-8") as fh:
    json.dump(doc, fh, ensure_ascii=False, indent=2)
    fh.write("\n")
print("decoded    : %s" % used, file=sys.stderr)
PY

python3 - "$out" <<'PY'
import json, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
os_ = d["os"]; host = d["host"]; rust = d["rust"]
print(f"out        : {sys.argv[1]}")
print(f"os         : {os_.get('caption')} build {os_.get('build')} ({os_.get('architecture')})")
print(f"elevated   : {host.get('elevated')}   long_paths : {host.get('long_paths_enabled')}")
print(f"cpu        : {d['cpu'].get('name')}  cores={d['cpu'].get('cores')}/{d['cpu'].get('logical')}")
print(f"ram        : {d['memory'].get('total_gib')} GiB total, {d['memory'].get('free_gib')} GiB free")
for v in d.get("volumes") or []:
    print(f"volume     : {v['drive']} {v['fs']} {v['size_gib']} GiB, {v['free_gib']} GiB free")
print(f"rust       : {rust.get('rustc_version') or 'absent'}  targets={rust.get('targets') or []}")
print(f"vs installs: {[i['displayName'] for i in (d.get('vs') or {}).get('installs') or []]}")
print(f"sshd       : {(d.get('ssh') or {}).get('sshd_service') or 'absent'}")
PY
