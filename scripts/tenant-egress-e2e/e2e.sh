#!/usr/bin/env bash
# Tenant egress attribution end to end on a docker host, for one container
# runtime (agent-runtime-program.md 3.6, sandbox-execution-plane.md §8):
# proxy and daemon share a tenant key -> a tenant's browser turn goes out
# with that tenant's credential (Chromium answers the proxy's 407 through
# Playwright) -> the proxy logs the connection under the tenant on open and
# on close. Raw requests without a credential, or with another tenant's
# name, are refused.
#
#   KURA=/path/to/linux/kura BROWSER_IMAGE=kura-browser-worker:local ./e2e.sh runc|runsc
#
# Needs what ../tenant-browser-e2e needs: root, docker with the runtime
# registered, python3, outbound HTTPS from the proxy, and the worker image
# (built from the repo root: docker build -f capabilities/browser/Dockerfile .).
set -uo pipefail
RT=${1:?runtime}
cd "$(dirname "$0")"
WORK=${WORK:-/tmp/kura-egress-e2e}
D=$WORK/$RT; rm -rf "$D"; mkdir -p "$D/data"
KURA=${KURA:?path to a linux kura binary}
BROWSER_IMAGE=${BROWSER_IMAGE:-kura-browser-worker:local}
NET=kura-tenant-e2e PROXY=kura-egress-proxy-e2e PROXY_IP=172.31.251.2
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

python3 -c 'import secrets;print(secrets.token_hex(32))' > "$D/egress.key"; chmod 600 "$D/egress.key"
docker network create --internal --subnet 172.31.251.0/24 $NET >/dev/null
docker run -d --name $PROXY --mount type=bind,src=$KURA,dst=/kura,readonly \
  --mount type=bind,src="$D/egress.key",dst=/egress.key,readonly alpine:3.20 \
  /kura egress-proxy --listen 0.0.0.0:3128 --tenant-key-file /egress.key --per-tenant-connections 16 >/dev/null
docker network connect --ip $PROXY_IP $NET $PROXY
for _ in $(seq 1 50); do docker logs $PROXY 2>&1 | grep -q listening && break; sleep 0.2; done
docker logs $PROXY 2>&1 | head -1

python3 ../tenant-browser-e2e/fake_model.py $MODEL_PORT "$D/model.log" & PIDS+=($!)
KURA_ENV=hosted KURA_HOSTING=shared KURA_DATA_DIR="$D/data" KURA_BIND_ADDR=$API \
KURA_LLM_OPENAI_COMPATIBLE_BASE_URL=http://127.0.0.1:$MODEL_PORT/v1 KURA_LLM_OPENAI_COMPATIBLE_API_KEY=operator-key \
KURA_LLM_OPENAI_COMPATIBLE_MODEL=fake KURA_LLM_DEFAULT_PROVIDER=openai_compatible KURA_LLM_DEFAULT_MODEL=fake \
KURA_SANDBOX_TENANT_NETWORK=$NET KURA_SANDBOX_EGRESS_PROXY=http://$PROXY_IP:3128 KURA_SANDBOX_EGRESS_KEY_FILE="$D/egress.key" \
KURA_SANDBOX_DOCKER_RUNTIME=$RT KURA_BROWSER_WORKER_IMAGE=$BROWSER_IMAGE KURA_BROWSER_IDLE_SECONDS=8 \
  $KURA daemon run >"$D/daemon.log" 2>&1 & PIDS+=($!)
for _ in $(seq 1 100); do curl -fsS -m 1 -o /dev/null http://$API/healthz 2>/dev/null && break; sleep 0.2; done
grep "\[kura\] sandbox: tenant egress" "$D/daemon.log"
check "the daemon hands out per-tenant credentials" 1 "$(grep -c 'its own proxy credential' "$D/daemon.log")"

j() { python3 -c "import json,sys;d=json.load(sys.stdin);print($1)"; }
start=$(curl -fsS -X POST http://$API/v1/auth/pairings/start -H 'Content-Type: application/json' -d '{"mode":"token","label":"op","ttlSeconds":120}')
OP=$(curl -fsS -X POST "http://$API/v1/auth/pairings/$(echo "$start" | j 'd["pairing"]["pairingId"]')/complete" \
  -H 'Content-Type: application/json' -d "{\"code\":\"$(echo "$start" | j 'd["pairingCode"]')\"}" | j 'd["accessToken"]')
prov=$(curl -fsS -X POST http://$API/v1/provisioning/users -H "Authorization: Bearer $OP" -H 'Content-Type: application/json' \
  -d '{"externalSubject":"github:888","displayName":"Attributed Tenant"}')
USER=$(echo "$prov" | j 'd["accessToken"]'); TENANT=$(echo "$prov" | j 'd["tenant"]["tenantId"]')
curl -fsS -o /dev/null -X POST http://$API/v1/tenant-secrets -H "Authorization: Bearer $USER" -H 'Content-Type: application/json' \
  -d '{"secretRef":"llm.openai_compatible.api_key","displayName":"key","value":"sk-tenant"}'

r=$(curl -sS -m 120 -X POST http://$API/v1/chat/query -H "Authorization: Bearer $USER" -H 'Content-Type: application/json' \
  -d '{"provider":"openai_compatible","model":"fake","query":"Please open https://example.com/ and tell me what it says"}')
reply=$(echo "$r" | j 'd.get("reply","")')
echo "[$RT] reply: ${reply:0:160}"
check "the page loaded through the authenticated proxy" 1 "$(grep -c 'Example Domain' <<<"$reply")"
cid=$(docker ps -q --filter label=kura.sandbox=isolated --filter "label=kura.tenant=$TENANT" | head -1)
if [[ -n "$cid" ]]; then
  check "the container's proxy URL names its tenant" 1 \
    "$(docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$cid" | grep -c "^HTTPS_PROXY=http://$TENANT:")"
fi
sleep 1
docker logs $PROXY 2>&1 | grep "tenant=" | head -5 | sed "s/^/[$RT] proxy: /"
check "the proxy logged the open connection under the tenant" 1 \
  "$(docker logs $PROXY 2>&1 | grep -E "tenant=$TENANT connect example\.com:443 -> .* opened" | head -1 | wc -l)"

# A download by URL goes through the same authenticated proxy (Playwright's
# request API, not the page) and lands in the tenant's workspace.
r=$(curl -sS -m 120 -X POST http://$API/v1/chat/query -H "Authorization: Bearer $USER" -H 'Content-Type: application/json' \
  -d '{"provider":"openai_compatible","model":"fake","query":"Please download https://example.com/ for me"}')
reply=$(echo "$r" | j 'd.get("reply","")')
echo "[$RT] download reply: ${reply:0:200}"
check "the download tool saved the file" 1 "$(grep -c 'Saved /workspace/downloads/' <<<"$reply")"
saved=$(find "$D/data" -path '*/workspace/downloads/*' -type f | head -1)
check "the file is in the tenant's workspace on the host" 1 "$( [[ -n "$saved" ]] && grep -c 'Example Domain' "$saved" | awk '{print ($1>0)?1:0}' || echo 0)"
[[ -n "$saved" ]] && echo "[$RT] saved: ${saved#$D/} owner=$(stat -c %u "$saved") size=$(stat -c %s "$saved")"
check "the download went out under the tenant" 1 \
  "$(docker logs $PROXY 2>&1 | grep -cE "tenant=$TENANT connect example\.com:443 -> .* opened" | awk '{print ($1>1)?1:0}')"

# Raw requests from a container on the tenant network.
raw() { docker run --rm --network $NET alpine:3.20 sh -c "printf '$1' | nc -w 5 $PROXY_IP 3128 | head -1"; }
cred=$(docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$cid" 2>/dev/null | sed -n "s#^HTTPS_PROXY=http://$TENANT:\([0-9a-f]*\)@.*#\1#p")
basic() { printf '%s' "$1:$2" | base64 -w0; }
check "no credential is refused" 407 "$(raw 'CONNECT example.com:443 HTTP/1.1\r\n\r\n' | awk '{print $2}')"
check "another tenant's name with this credential is refused" 407 \
  "$(raw "CONNECT example.com:443 HTTP/1.1\r\nProxy-Authorization: Basic $(basic ten_other "$cred")\r\n\r\n" | awk '{print $2}')"
check "the tenant's own credential is accepted" 200 \
  "$(raw "CONNECT example.com:443 HTTP/1.1\r\nProxy-Authorization: Basic $(basic "$TENANT" "$cred")\r\n\r\n" | awk '{print $2}')"
check "the policy still applies to an authenticated tenant" 403 \
  "$(raw "CONNECT 10.3.4.4:443 HTTP/1.1\r\nProxy-Authorization: Basic $(basic "$TENANT" "$cred")\r\n\r\n" | awk '{print $2}')"
check "refusals are logged with the tenant" 1 \
  "$(docker logs $PROXY 2>&1 | grep -c "tenant=$TENANT refused 10.3.4.4" | awk '{print ($1>0)?1:0}')"
echo "[$RT] waiting for the idle reaper to close the browser..."
for _ in $(seq 1 40); do
  docker logs $PROXY 2>&1 | grep -qE "tenant=$TENANT connect example\.com:443 .* closed up=[0-9]+ down=[1-9]" && break; sleep 1
done
docker logs $PROXY 2>&1 | grep "tenant=$TENANT connect example.com" | tail -1 | sed "s/^/[$RT] proxy: /"
check "closing the connection logs its bytes under the tenant" 1 \
  "$(docker logs $PROXY 2>&1 | grep -E "tenant=$TENANT connect example\.com:443 .* closed up=[0-9]+ down=[1-9]" | head -1 | wc -l)"
check "nothing used the operator's key" 0 "$(grep -c operator-key "$D/model.log")"
echo "[$RT] $([[ $fail == 0 ]] && echo ALL PASSED || echo SOME FAILED)"
exit $fail
