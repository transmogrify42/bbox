#!/usr/bin/env bash
# Run an OGC executable test suite (TEAM Engine Docker image) against bbox-feature-server.
#
# Usage: tests/ets/run-ets.sh wfs10|wfs11|wfs20|dgiwg
#
# Prerequisite: cargo test --test ets_fixtures -- --ignored   (writes target/wfs-ets)
# Results: target/wfs-ets/<suite>-earl.xml (EARL report), <suite>-te-logs (TEAM Engine session logs)
# and a summary on stdout.

set -euo pipefail

SUITE="${1:?suite: wfs10|wfs11|wfs20|dgiwg}"
cd "$(dirname "$0")/../.."
ROOT="$(cd .. && pwd)"
OUT="$ROOT/target/wfs-ets"
TE_PORT="${TE_PORT:-18081}"
HOST="http://host.docker.internal"

urlencode() { python3 -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.argv[1], safe=""))' "$1"; }

case "$SUITE" in
    wfs10)
        IMAGE="ogccite/ets-wfs10:1.14-teamengine-6.0.0-RC2"; PORT=18010; CFG=ets-wfs10.toml
        RUN="wfs10/run?capabilities-url=$(urlencode "$HOST:$PORT/wfs?service=WFS&version=1.0.0&request=GetCapabilities")&multiplenamenspaces=multiplenamenspaces&complexvalue=complexvalue"
        ;;
    wfs11)
        IMAGE="ogccite/ets-wfs11:1.38-teamengine-6.0.0-RC2"; PORT=18011; CFG=ets-wfs11.toml
        RUN="wfs/run?capabilities-url=$(urlencode "$HOST:$PORT/wfs?service=WFS&version=1.1.0&request=GetCapabilities")&wfs-xlink=XLink&profile=sf-0"
        ;;
    wfs20)
        IMAGE="ogccite/ets-wfs20:1.43-teamengine-6.0.0-RC2"; PORT=18020; CFG=ets-wfs20.toml
        RUN="wfs20/run?wfs=$(urlencode "$HOST:$PORT/wfs?service=WFS&version=2.0.0&request=GetCapabilities")"
        ;;
    dgiwg)
        IMAGE="ogccite/ets-wfs20-dgiwg:0.9-teamengine-6.0.0"; PORT=18021; CFG=ets-dgiwg.toml
        RUN="wfs20-dgiwg/run?wfs=$(urlencode "$HOST:$PORT/wfs?service=WFS&version=2.0.2&request=GetCapabilities")"
        ;;
    *) echo "unknown suite $SUITE"; exit 1 ;;
esac

cargo build --release -q -p bbox-feature-server
"$ROOT/target/release/bbox-feature-server" --config "$OUT/$CFG" --loglevel warn serve > "$OUT/$SUITE-server.log" 2>&1 &
SERVER_PID=$!
CONTAINER="bbox-ets-$SUITE"
cleanup() {
    # keep TEAM Engine session logs (requests, responses, messages) for analysis
    rm -rf "$OUT/$SUITE-te-logs"
    docker cp "$CONTAINER:/root/te_base/users/ogctest" "$OUT/$SUITE-te-logs" >/dev/null 2>&1 || true
    kill "$SERVER_PID" 2>/dev/null || true
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
docker run -d --name "$CONTAINER" --platform linux/amd64 -p "127.0.0.1:$TE_PORT:8080" \
    --add-host=host.docker.internal:host-gateway "$IMAGE" >/dev/null
echo "Waiting for TEAM Engine ($IMAGE)..."
for _ in $(seq 1 120); do
    if curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$TE_PORT/teamengine/" | grep -q 200; then break; fi
    sleep 3
done
curl -sf "http://127.0.0.1:$PORT/wfs?service=WFS&request=GetCapabilities" >/dev/null || { echo "server not reachable"; cat "$OUT/$SUITE-server.log"; exit 1; }

echo "Running $SUITE..."
curl -s -u ogctest:ogctest -H 'Accept: application/rdf+xml' --max-time 3600 \
    "http://127.0.0.1:$TE_PORT/teamengine/rest/suites/$RUN" -o "$OUT/$SUITE-earl.xml"

python3 "$(dirname "$0")/summary.py" "$OUT/$SUITE-earl.xml"
