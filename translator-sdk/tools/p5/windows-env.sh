#!/usr/bin/env bash
# windows-env.sh -- run tools/p5/windows-env-current.ps1 on a remote Windows host
# over OpenSSH and store the resulting `dsh-win-env/v1` JSON locally, validated.
#
# The PowerShell script is streamed over stdin (`powershell -Command -`), so
# nothing is written to the remote disk and there is no command-line length
# limit to fight. The script itself is read-only (docs/32 §2).
#
# Usage:
#   tools/p5/windows-env.sh home
#   tools/p5/windows-env.sh home -o fixtures/p5/windows-env-current.json
#
# Exit codes: 0 = JSON captured and parsed; 2 = ssh ok but output unusable;
#             3 = precondition missing (ssh/ps1/python3).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ps1="$here/windows-env-current.ps1"
port=22
out=""

usage() { sed -n '2,13p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

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

ssh -p "$port" \
    -o BatchMode=yes \
    -o ConnectTimeout=10 \
    -o StrictHostKeyChecking=accept-new \
    "$target" \
    "powershell -NoProfile -ExecutionPolicy Bypass -Command -" < "$ps1" > "$raw" 2>&1 || true

if [ -z "$out" ]; then
  out="windows-env-$(date -u +%Y%m%dT%H%M%SZ).json"
fi

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
    print("FAIL: remote output decodable as neither utf-8 nor cp936", file=sys.stderr)
    sys.exit(2)

inside, block = False, []
for line in text.splitlines():
    s = line.strip()
    if s == "DSH-ENV-BEGIN":
        inside, block = True, []
        continue
    if s == "DSH-ENV-END":
        break
    if inside:
        block.append(line)

if not block:
    print("FAIL: no DSH-ENV-BEGIN/END block. Raw output follows:", file=sys.stderr)
    print(text[:2000], file=sys.stderr)
    sys.exit(2)

try:
    doc = json.loads("\n".join(block))
    assert doc["schema"] == "dsh-win-env/v1"
except Exception as exc:
    print("FAIL: captured block is not valid dsh-win-env/v1 JSON: %s" % exc, file=sys.stderr)
    print("\n".join(block)[:2000], file=sys.stderr)
    sys.exit(2)

with open(out_path, "w", encoding="utf-8") as fh:
    json.dump(doc, fh, ensure_ascii=False, indent=1)
    fh.write("\n")
print("decoded    : %s" % used, file=sys.stderr)
PY

python3 - "$out" <<'PY'
import json, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
ws, tc, py = d["workspace"], d["toolchain"], d["python"]
ms, git, ort = d["msvc"], d["git"], d["ort"]
print(f"out        : {sys.argv[1]}")
print(f"workspace  : {ws['path']}  exists={ws['exists']} empty={ws['empty']} git={ws['git_present']} free={ws['free_gib']} GiB")
print(f"rust       : {tc['rustc_version']} (host {tc['rustc_host']}, LLVM {tc['llvm_version']})")
print(f"cargo      : {tc['cargo_version']}   rustup home {tc['rustup_home']}")
print(f"toolchains : {tc['rustup_toolchains']}")
print(f"python     : {py['path']} -> {py['version']} (exists={py['exists']})")
print(f"msvc       : vc {ms['vc_tools_version']}  sdk {ms['sdk_lib_versions']}  cl_on_path={ms['cl_on_path']}")
print(f"git        : {git['version']}")
print(f"ort        : {ort['dir']} version {ort['version']} ({len(ort['lib_files'])} files)")
for n in d.get("network") or []:
    print(f"net        : direct={n['direct_code']:<4} proxy={n['via_env_proxy_code']:<4} {n['url'][:70]}")
print(f"pyke sample: {d['pyke_sample']['direct_range_0_256KiB']}")
PY
