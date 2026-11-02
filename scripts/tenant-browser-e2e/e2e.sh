#!/usr/bin/env bash
# Tenant browser end to end on a docker host, for one container runtime
# (agent-runtime-program.md 3.3, docs/harness/tenant-browser.md):
# real daemon (shared mode) -> scripted model asks for browser.open -> the
# daemon starts Chromium in the tenant's isolated container -> the page loads
# through the real egress proxy -> its text comes back in the reply.
#
#   KURA=/path/to/linux/kura BROWSER_IMAGE=kura-browser-worker:local ./e2e.sh runc|runsc
#
# Needs root (the sandbox chowns tenant workspaces), docker with the runtime
# registered, python3, and outbound HTTPS from the proxy. It creates and
# removes its own network, proxy container and data dir under $WORK.
set -uo pipefail
RT=${1:?runtime}
cd "$(dirname "$0")"
WORK=${WORK:-/tmp/kura-browser-e2e}
D=$WORK/$RT; rm -rf "$D"; mkdir -p "$D/data"
KURA=${KURA:?path to a linux kura binary}
BROWSER_IMAGE=${BROWSER_IMAGE:-kura-browser-worker:local}
NET=kura-tenant-e2e PROXY=kura-egress-proxy-e2e
API=127.0.0.1:19393 MODEL_PORT=18093
PIDS=()
fail=0
check() { if [[ "$2" == "$3" ]]; then echo "PASS [$RT] $1"; else echo "FAIL [$RT] $1: expected [$2] got [$3]"; fail=1; fi; }
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done; wait 2>/dev/null
  docker rm -f $PROXY >/dev/null 2>&1; docker network rm $NET >/dev/null 2>&1
  docker ps -aq --filter label=kura.sandbox=isolated | xargs -r docker rm -f >/dev/null
}
trap cleanup EXIT
cleanup

# A fixed proxy address: under runsc docker's embedded DNS does not answer,
# so the proxy is addressed by IP (the daemon refuses a name under runsc).
docker network create --internal --subnet 172.31.251.0/24 $NET >/dev/null
docker run -d --name $PROXY --mount type=bind,src=$KURA,dst=/kura,readonly alpine:3.20 \
  /kura egress-proxy --listen 0.0.0.0:3128 >/dev/null
docker network connect --ip 172.31.251.2 $NET $PROXY
PROXY_URL=http://172.31.251.2:3128
for _ in $(seq 1 50); do docker logs $PROXY 2>&1 | grep -q listening && break; sleep 0.2; done

python3 fake_model.py $MODEL_PORT "$D/model.log" & PIDS+=($!)

KURA_ENV=hosted KURA_HOSTING=shared KURA_DATA_DIR="$D/data" KURA_BIND_ADDR=$API \
KURA_LLM_OPENAI_COMPATIBLE_BASE_URL=http://127.0.0.1:$MODEL_PORT/v1 KURA_LLM_OPENAI_COMPATIBLE_API_KEY=operator-key \
KURA_LLM_OPENAI_COMPATIBLE_MODEL=fake KURA_LLM_DEFAULT_PROVIDER=openai_compatible KURA_LLM_DEFAULT_MODEL=fake \
KURA_SANDBOX_TENANT_NETWORK=$NET KURA_SANDBOX_EGRESS_PROXY=$PROXY_URL \
KURA_SANDBOX_DOCKER_RUNTIME=$RT KURA_BROWSER_WORKER_IMAGE=$BROWSER_IMAGE \
KURA_BROWSER_IDLE_SECONDS=8 \
  $KURA daemon run >"$D/daemon.log" 2>&1 & PIDS+=($!)
for _ in $(seq 1 100); do curl -fsS -m 1 -o /dev/null http://$API/healthz 2>/dev/null && break; sleep 0.2; done
grep "\[kura\] browser\|\[kura\] sandbox" "$D/daemon.log"

j() { python3 -c "import json,sys;d=json.load(sys.stdin);print($1)"; }
start=$(curl -fsS -X POST http://$API/v1/auth/pairings/start -H 'Content-Type: application/json' -d '{"mode":"token","label":"op","ttlSeconds":120}')
OP=$(curl -fsS -X POST "http://$API/v1/auth/pairings/$(echo "$start" | j 'd["pairing"]["pairingId"]')/complete" \
  -H 'Content-Type: application/json' -d "{\"code\":\"$(echo "$start" | j 'd["pairingCode"]')\"}" | j 'd["accessToken"]')
prov=$(curl -fsS -X POST http://$API/v1/provisioning/users -H "Authorization: Bearer $OP" -H 'Content-Type: application/json' \
  -d '{"externalSubject":"github:777","displayName":"Browsing Tenant"}')
# The scripted model only looks for a URL in the question; 10.3.4.4 stands
# for any private address (the cell pilot's gateway host).
USER=$(echo "$prov" | j 'd["accessToken"]'); TENANT=$(echo "$prov" | j 'd["tenant"]["tenantId"]')
curl -fsS -o /dev/null -X POST http://$API/v1/tenant-secrets -H "Authorization: Bearer $USER" -H 'Content-Type: application/json' \
  -d '{"secretRef":"llm.openai_compatible.api_key","displayName":"key","value":"sk-tenant"}'
chat() {
  curl -sS -m 120 -X POST http://$API/v1/chat/query -H "Authorization: Bearer $USER" -H 'Content-Type: application/json' \
    -d "{\"provider\":\"openai_compatible\",\"model\":\"fake\",\"query\":\"$1\"}"
}

t0=$(date +%s%3N)
r=$(chat "Please open https://example.com/ and tell me what it says")
t1=$(date +%s%3N)
reply=$(echo "$r" | j 'd.get("reply","")')
echo "[$RT] reply (turn $((t1-t0)) ms): ${reply:0:200}"
check "the model was offered the browser tools" 1 "$(grep -c '"browser' "$D/model.log" | awk '{print ($1>0)?1:0}')"
check "the page text came back through the tool" 1 "$(grep -c 'Example Domain' <<<"$reply")"
running=$(docker ps --filter label=kura.sandbox=isolated --filter "label=kura.tenant=$TENANT" --format '{{.ID}}' | wc -l)
check "one browser container runs, labelled with the tenant" 1 "$running"
cid=$(docker ps -q --filter label=kura.sandbox=isolated --filter "label=kura.tenant=$TENANT" | head -1)
if [[ -n "$cid" ]]; then
  check "it runs under $RT" "$RT" "$(docker inspect -f '{{.HostConfig.Runtime}}' "$cid")"
  check "it sits on the internal tenant network only" "$NET" "$(docker inspect -f '{{range $k,$v := .NetworkSettings.Networks}}{{$k}}{{end}}' "$cid")"
  check "as the tenant's uid, read-only, no capabilities" "true [ALL]" \
    "$(docker inspect -f '{{.HostConfig.ReadonlyRootfs}} {{.HostConfig.CapDrop}}' "$cid")"
  docker stats --no-stream --format "[$RT] browser container: {{.MemUsage}} pids={{.PIDs}}" "$cid"
fi
r=$(chat "now open http://10.3.4.4/")
echo "[$RT] private address attempt: $(echo "$r" | j 'd.get("reply","")' | cut -c1-160)"
check "a private address goes to the proxy and is refused there" 1 \
  "$(docker logs $PROXY 2>&1 | grep -c 'refused 10.3.4.4' | awk '{print ($1>0)?1:0}')"

r=$(chat "now open http://169.254.0.23/latest/meta-data/")
reply=$(echo "$r" | j 'd.get("reply","")')
echo "[$RT] metadata attempt: ${reply:0:200}"
check "the metadata address is not reachable" 0 "$(grep -ci 'instance\|ami-\|hostname' <<<"$reply")"

docker stop -t 1 $PROXY >/dev/null
r=$(chat "now open https://example.org/")
reply=$(echo "$r" | j 'd.get("reply","")')
echo "[$RT] with the proxy stopped: ${reply:0:160}"
check "with the proxy stopped no page loads (no other way out)" 0 "$(grep -c 'Example Domain' <<<"$reply")"
docker start $PROXY >/dev/null

echo "[$RT] waiting for the idle reaper (8 s idle)..."
for _ in $(seq 1 40); do
  n=$(docker ps -q --filter label=kura.sandbox=isolated --filter "label=kura.tenant=$TENANT" | wc -l)
  [[ $n == 0 ]] && break; sleep 1
done
check "the idle browser's container is gone" 0 "$n"
grep "\[kura\] browser: tenant" "$D/daemon.log" | sed "s/^/[$RT] /"
check "nothing used the operator's key" 0 "$(grep -c operator-key "$D/model.log")"
if [[ $RT == runsc ]]; then
  KURA_ENV=hosted KURA_DATA_DIR="$D/data2" KURA_BIND_ADDR=127.0.0.1:19394 \
  KURA_SANDBOX_TENANT_NETWORK=$NET KURA_SANDBOX_EGRESS_PROXY=http://$PROXY:3128 KURA_SANDBOX_DOCKER_RUNTIME=runsc \
    timeout 20 $KURA daemon run >"$D/daemon2.log" 2>&1; code=$?
  check "runsc with a proxy named by host refuses to boot" 1 "$( [[ $code != 0 && $code != 124 ]] && grep -q 'by IP' "$D/daemon2.log" && echo 1 || echo 0)"
fi
echo "[$RT] $([[ $fail == 0 ]] && echo ALL PASSED || echo SOME FAILED)"
exit $fail
