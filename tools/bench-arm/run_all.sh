#!/usr/bin/env bash
# All measurement rounds one after the other (never two setups at the same time), order changes.
# Start: setsid nohup bash ~/prbench/tools/run_all.sh > ~/prbench/run_all.log 2>&1 < /dev/null &
# Optionally own step list: STEPS="1:rs 1:ngxb" bash run_all.sh
# Steps that already ran successfully (json without "error") are skipped.
cd ~/prbench || exit 1
LOCK=~/jobs/paperrust_bench.lock
trap 'rm -f "$LOCK"' EXIT
# Form round:setup[:short]; short = S1 only 1 min, without S2/S3 (for ngxc = ngxb without cleaning/straightening)
STEPS=${STEPS:-"1:rs 1:ngxb 2:ngxb 2:rs 3:rs 3:ngxb 1:ngxa 1:ngxc:short"}
foreign() { ls ~/jobs/mpc_*.lock 2>/dev/null | grep -v mpc_luecke_; }
for step in $STEPS; do
  IFS=: read -r rnd sys opt <<< "$step"
  flag=""; [ "$opt" = short ] && flag="--short"
  out=results/run_${rnd}_${sys}.json
  if [ -f "$out" ] && ! grep -q '"error"' "$out"; then continue; fi
  echo "$$ $(date -Is) PAPERRUST BENCHMARK queue" > "$LOCK"
  # foreign compute jobs: wait up to 2 h so that all rounds run on 4 cores (otherwise bench.py: 2 cores)
  for i in $(seq 1 24); do [ -z "$(foreign)" ] && break; echo "$(date +%T) warte: $(foreign)"; sleep 300; done
  echo "$(date +%T) Runde $rnd $sys"; free -m | sed -n 2p; uptime
  taskset -c 4,5 python3 tools/bench.py run "$rnd" "$sys" $flag > "results/log_${rnd}_${sys}.txt" 2>&1
  tail -2 "results/log_${rnd}_${sys}.txt"
  echo "$$ $(date -Is) PAPERRUST BENCHMARK queue" > "$LOCK"
  sleep 60
done
echo fertig > run_all.done
