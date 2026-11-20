#!/usr/bin/env bash
# document_exporter: Rust vs upstream on the same DB copy (pl_w7_exp from paperless_snapshot),
# own target folders per option set; comparison with compare.py (names, bytes; JSON CRLF-normalized,
# because Python on Windows writes \r\n in text mode).
set -uo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
BIN="${PLCLI_BIN:-$REPO_DIR/target/debug/examples/manage.exe}"
copy_env exp >/dev/null 2>&1; use_env exp
W="$WORKROOT/exp"; fail=0
run_case() {  # name, args...
  local name="$1"; shift
  rm -rf "$W/out_up_$name" "$W/out_rs_$name"; mkdir -p "$W/out_up_$name" "$W/out_rs_$name"
  manage_py document_exporter --no-progress-bar "$W/out_up_$name" "$@" >"$W/up_$name.log" 2>&1 || { echo "upstream $name failed"; tail -3 "$W/up_$name.log"; }
  "$BIN" document_exporter "$W/out_rs_$name" "$@" >"$W/rs_$name.log" 2>&1 || { echo "rust $name failed"; tail -3 "$W/rs_$name.log"; }
  local z=""; [[ " $* " == *" --zip "* ]] && z="--zip"
  if [[ -n "$z" ]]; then
    local fa fb; fa=$(ls "$W/out_up_$name"/*.zip); fb=$(ls "$W/out_rs_$name"/*.zip)
    [[ "$(basename "$fa")" == "$(basename "$fb")" ]] || echo "DIFF zip name $(basename "$fa") $(basename "$fb")"
    "$PY" -I "$HERE_PLCLI/compare.py" tree "$fa" "$fb" --zip | tail -4
  else
    "$PY" -I "$HERE_PLCLI/compare.py" tree "$W/out_up_$name" "$W/out_rs_$name" | tail -4
  fi
  [[ ${PIPESTATUS[0]} -eq 0 ]] && echo "OK   $name" || { echo "FAIL $name"; fail=1; }
}
run_case default
run_case split -sm
run_case dataonly --data-only -nt -na
run_case zip --zip -zn exp-test
PAPERLESS_FILENAME_FORMAT="{{ created_year }}/{{ correspondent }}/{{ title }}" run_case fmt -f -p -sm
# incremental: second run into the same folder with -c -d; foreign file is deleted, manifest stays the same
echo junk >"$W/out_rs_default/junk.txt"; mkdir -p "$W/out_rs_default/old"; echo x >"$W/out_rs_default/old/x.pdf"
"$BIN" document_exporter "$W/out_rs_default" -c -d >/dev/null 2>&1
[[ ! -e "$W/out_rs_default/junk.txt" && ! -e "$W/out_rs_default/old" ]] && echo "OK   --delete entfernt Fremddateien" || { echo "FAIL --delete"; fail=1; }
"$PY" -I "$HERE_PLCLI/compare.py" tree "$W/out_up_default" "$W/out_rs_default" | tail -1
# Error cases like upstream
"$BIN" document_exporter "$W/does-not-exist" 2>&1 | tail -1
"$BIN" document_exporter "$W/out_rs_default" --zip -c 2>&1 | tail -1
"$BIN" document_exporter "$W/out_rs_default" --zip-compression stored 2>&1 | tail -1
exit $fail
