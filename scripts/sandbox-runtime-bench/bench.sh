#!/usr/bin/env bash
# runc vs runsc on a cell host, with kura's isolated-run flags
# (crates/domains/sandbox/src/isolation.rs isolated_docker_args).
# Results and the decision: docs/harness/sandbox-execution-plane.md §9.
#
# Run as root on a docker host with runsc registered and a kura cell named
# kura-cell-smoke (its image is the daemon measured last). It drops the page
# cache before the memory step; run it on a host that can take that.
set -euo pipefail
cd "$(dirname "$0")"
ALPINE=alpine:3.20
PY=python:3.12-alpine
CELL=$(docker inspect -f '{{.Config.Image}}' kura-cell-smoke)
docker pull -q $PY >/dev/null
WS=$(mktemp -d /tmp/kura-bench-ws.XXXX); chown 10001:10001 "$WS"
BASE=(--user 10001:10001 --cap-drop ALL --security-opt no-new-privileges --read-only
  --tmpfs /tmp:rw,noexec,nosuid,nodev,size=512m --network none --ipc private --init
  --cpus 1 --memory 1024m --memory-swap 1024m --pids-limit 256
  --mount type=bind,src=$WS,dst=/workspace --workdir /workspace -e HOME=/workspace)
FLAGS=(--rm -i "${BASE[@]}")
now_ms() { date +%s%3N; }

echo "== start: docker run --rm <isolated flags> alpine true (15 each, alternating)"
for rt in runc runsc; do : >"start.$rt"; done
for i in $(seq 1 15); do
  for rt in runc runsc; do
    t=$(now_ms); docker run --runtime=$rt "${FLAGS[@]}" $ALPINE true; echo $(( $(now_ms) - t )) >>"start.$rt"
  done
done
for rt in runc runsc; do echo "$rt $(sort -n start.$rt | awk '{a[NR]=$1} END {printf "p50=%dms p90=%dms min=%dms max=%dms", a[int(NR*0.5+0.5)], a[int(NR*0.9+0.5)], a[1], a[NR]}')"; done

echo "== workloads inside the container (ms; 3 runs each, alternating)"
for i in 1 2 3; do
  for rt in runc runsc; do
    echo "$rt $(docker run --runtime=$rt "${FLAGS[@]}" -v "$PWD/bench.py:/bench.py:ro" $PY python /bench.py)"
  done
done

echo "== idle memory: 5 containers per runtime running sleep, host MemAvailable delta"
for rt in runc runsc; do
  sync; echo 1 >/proc/sys/vm/drop_caches; sleep 2
  before=$(awk '/MemAvailable/ {print $2}' /proc/meminfo)
  ids=()
  for i in 1 2 3 4 5; do ids+=($(docker run -d --runtime=$rt "${BASE[@]}" $ALPINE sleep 300)); done
  sleep 8
  after=$(awk '/MemAvailable/ {print $2}' /proc/meminfo)
  per=$(docker stats --no-stream --format '{{.MemUsage}}' "${ids[@]}" | awk '{print $1}' | tr '\n' ' ')
  echo "$rt host delta per container: $(( (before - after) / 5 / 1024 )) MiB; docker stats: $per"
  docker rm -f "${ids[@]}" >/dev/null
done

echo "== kura daemon (cell image) boot to /healthz, and idle memory after boot"
for rt in runc runsc; do
  t=$(now_ms)
  id=$(docker run -d --runtime=$rt --name gvbench-cell-$rt --cap-drop ALL --security-opt no-new-privileges \
    --memory 512m --memory-swap 512m --pids-limit 512 --network none \
    -e KURA_ENV=hosted -e KURA_BIND_ADDR=127.0.0.1:19191 -e KURA_DATA_DIR=/data --tmpfs /data:rw,size=256m,uid=10001 $CELL)
  ok=""
  for _ in $(seq 1 300); do
    if docker exec $id wget -q -t 1 -T 1 -O /dev/null http://127.0.0.1:19191/healthz 2>/dev/null; then ok=1; break; fi
    sleep 0.1
  done
  boot=$(( $(now_ms) - t ))
  sleep 5
  mem=$(docker stats --no-stream --format '{{.MemUsage}}' $id | awk '{print $1}')
  t=$(now_ms); docker exec $id wget -q -t 1 -O /dev/null http://127.0.0.1:19191/healthz; probe=$(( $(now_ms) - t ))
  echo "$rt healthy=${ok:-no} boot_to_healthz=${boot}ms idle_mem=$mem one_exec_probe=${probe}ms"
  docker rm -f $id >/dev/null
done
rm -rf "$WS"
