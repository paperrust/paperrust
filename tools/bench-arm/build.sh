#!/usr/bin/env bash
# Builds the PaperRust image natively (arm64) in an own BuildKit builder (prbench_builder),
# so that the build cache can be deleted together with the builder when cleaning up.
set -euo pipefail
source ~/prbench/tools/lib.sh
take_lock build
wait_mem 2500
JOBS=3
if [ -n "$(foreign_mpc)" ]; then JOBS=2; fi
cd ~/prbench/src
sudo -n docker buildx inspect prbench_builder >/dev/null 2>&1 || \
  sudo -n docker buildx create --name prbench_builder --driver docker-container >/dev/null
start=$(date +%s)
echo "BUILD_START $(date -Is) jobs=$JOBS"
# Watchdog: aborts if RAM gets tight (protect foreign services)
( while sleep 10; do a=$(mem_avail); echo "$(date +%T) avail=$a" >> ~/prbench/build_mem.log;
    if [ "$a" -lt 900 ]; then echo "RAM knapp ($a MB) – Abbruch" >> ~/prbench/build_mem.log; sudo -n docker buildx stop prbench_builder; break; fi; done ) &
WD=$!
set +e
sudo -n docker buildx build --builder prbench_builder --load --progress=plain \
  -f docker/Dockerfile.prbench -t prbench_paperrust:local \
  --build-arg CARGO_BUILD_JOBS=$JOBS --build-arg CARGO_RELEASE_LTO=thin --build-arg CARGO_RELEASE_CODEGEN_UNITS=16 .
rc=$?
kill $WD 2>/dev/null
end=$(date +%s)
echo "BUILD_END $(date -Is) rc=$rc seconds=$((end-start))"
exit $rc
