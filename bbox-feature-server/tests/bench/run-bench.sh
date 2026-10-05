#!/usr/bin/env bash
# Benchmark bbox-feature-server against GeoServer on the same PostGIS data.
#
# Prerequisites (throwaway containers, see tests/wfs_postgis.rs and tests/geoserver/start-oracle.sh):
#   - PostGIS `bbox-wfs-postgis` on 127.0.0.1:55432 with schema synth_big (1M observations,
#     created by: BBOX_WFS_TEST_PG=... cargo test --release --test wfs_postgis -- --ignored)
#   - GeoServer on 127.0.0.1:18083 (GS_URL), natively (not an emulated amd64 image), e.g. the
#     GeoServer 3.0.1 binary distribution in an arm64 eclipse-temurin:21-jre container
#
# Usage: tests/bench/run-bench.sh [requests per scenario]

set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT="$(cd .. && pwd)"
OUT="$ROOT/target/wfs-bench"
mkdir -p "$OUT"
GS="${GS_URL:-http://127.0.0.1:18083/geoserver}"
AUTH=(-u admin:geoserver)
PG_URL="postgresql://wfstest:wfstest@127.0.0.1:55432/wfstest"
PORT=18090

# bbox server (release build)
cat > "$OUT/bench.toml" <<TOML
[webserver]
server_addr = "127.0.0.1:$PORT"
public_server_url = "http://127.0.0.1:$PORT"

[[datasource]]
name = "pg"
[datasource.postgis]
url = "$PG_URL"

[[collection]]
name = "observations"
[collection.postgis]
datasource = "pg"
table_schema = "synth_big"
table_name = "observations"

[[collection]]
name = "footprints"
[collection.postgis]
datasource = "pg"
table_schema = "synth_big"
table_name = "footprints"

[wfs]
namespace_prefix = "app"
namespace_uri = "http://example.com/app"
TOML
# bbox on ClickHouse (native protocol), same synthetic data
CH_PORT=18091
cat > "$OUT/bench-ch.toml" <<TOML
[webserver]
server_addr = "127.0.0.1:$CH_PORT"
public_server_url = "http://127.0.0.1:$CH_PORT"

[[datasource]]
name = "ch"
[datasource.clickhouse]
url = "tcp://wfstest:wfstest@127.0.0.1:59000/synth_big"

[[collection]]
name = "observations"
[collection.clickhouse]
datasource = "ch"
table_name = "observations"
fid_field = "fid"

[wfs]
namespace_prefix = "app"
namespace_uri = "http://example.com/app"
TOML
cargo build --release -q -p bbox-feature-server
"$ROOT/target/release/bbox-feature-server" --config "$OUT/bench.toml" --loglevel warn serve > "$OUT/bbox.log" 2>&1 &
BBOX_PID=$!
"$ROOT/target/release/bbox-feature-server" --config "$OUT/bench-ch.toml" --loglevel warn serve > "$OUT/bbox-ch.log" 2>&1 &
BBOX_CH_PID=$!
trap 'kill $BBOX_PID $BBOX_CH_PID 2>/dev/null || true' EXIT

# GeoServer: PostGIS store on the same tables
rest() { curl -sS "${AUTH[@]}" -H 'Content-Type: application/json' "$@" >/dev/null; }
rest -XPOST "$GS/rest/namespaces" -d '{"namespace":{"prefix":"app","uri":"http://example.com/app","isolated":false}}' || true
rest -XPOST "$GS/rest/workspaces/app/datastores" -d '{"dataStore":{"name":"pg","type":"PostGIS","enabled":true,"connectionParameters":{"entry":[
  {"@key":"dbtype","$":"postgis"},{"@key":"host","$":"host.docker.internal"},{"@key":"port","$":"55432"},
  {"@key":"database","$":"wfstest"},{"@key":"schema","$":"synth_big"},{"@key":"user","$":"wfstest"},
  {"@key":"passwd","$":"wfstest"},{"@key":"namespace","$":"http://example.com/app"},
  {"@key":"Expose primary keys","$":"true"},{"@key":"max connections","$":"20"},{"@key":"Estimated extends","$":"true"}]}}}' || true
for t in observations footprints; do
  rest -XPOST "$GS/rest/workspaces/app/datastores/pg/featuretypes" \
    -d "{\"featureType\":{\"name\":\"$t\",\"nativeName\":\"$t\",\"srs\":\"EPSG:4326\",\"projectionPolicy\":\"FORCE_DECLARED\",\"numDecimals\":15}}" || true
done
for _ in $(seq 1 60); do
  curl -fs "http://127.0.0.1:$PORT/wfs?service=WFS&request=GetCapabilities" >/dev/null 2>&1 && break
  sleep 1
done

BBOX_WFS_BENCH_BBOX="http://127.0.0.1:$PORT" BBOX_WFS_BENCH_BBOX_CH="http://127.0.0.1:$CH_PORT" BBOX_WFS_BENCH_GEOSERVER="$GS" BBOX_WFS_BENCH_REQUESTS="${1:-400}" \
  cargo test --release -q --test wfs_bench -- --ignored --nocapture 2>&1 | tee "$OUT/results.txt"
