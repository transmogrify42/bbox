#!/usr/bin/env bash
# Start a GeoServer reference instance (Docker) serving the GeoServer WFS test catalog
# (cite, cdf, cgf, sf) from the property files fetched by tests/fetch-geoserver-data.sh.
#
# Usage: tests/geoserver/start-oracle.sh        (stop with: docker rm -f bbox-gs-oracle)
# Then:  BBOX_WFS_TEST_GEOSERVER=http://127.0.0.1:18082/geoserver cargo test --test gs_oracle -- --ignored

set -euo pipefail
cd "$(dirname "$0")/../.."
IMAGE="docker.osgeo.org/geoserver:3.0.1"
NAME=bbox-gs-oracle
PORT="${GS_PORT:-18082}"
GS="http://127.0.0.1:$PORT/geoserver"
AUTH=(-u admin:geoserver)
DATA="$(cd ../target && pwd)/geoserver-data"
[ -f "$DATA/main/BasicPolygons.properties" ] || tests/fetch-geoserver-data.sh

# property directories per namespace
PROPS="$DATA/props"
rm -rf "$PROPS"
NAMESPACES="cite cdf cgf sf"
types_of() {
  case "$1" in
    cite) echo "BasicPolygons Bridges Buildings DividedRoutes Forests Lakes MapNeatline NamedPlaces Ponds RoadSegments Streams Geometryless" ;;
    cdf) echo "Deletes Fifteen Inserts Locks Nulls Other Seven Updates" ;;
    cgf) echo "Lines MLines MPoints MPolygons Points Polygons" ;;
    sf) echo "PrimitiveGeoFeature AggregateGeoFeature GenericEntity" ;;
  esac
}
uri_of() {
  case "$1" in
    cite) echo "http://www.opengis.net/cite" ;;
    cdf) echo "http://www.opengis.net/cite/data" ;;
    cgf) echo "http://www.opengis.net/cite/geometry" ;;
    sf) echo "http://cite.opengeospatial.org/gmlsf" ;;
  esac
}
srs_of() {
  case "$1" in cdf|cgf) echo 32615 ;; *) echo 4326 ;; esac
}
for ns in $NAMESPACES; do
  mkdir -p "$PROPS/$ns"
  for t in $(types_of $ns); do cp "$DATA/main/$t.properties" "$PROPS/$ns/"; done
done

docker rm -f "$NAME" >/dev/null 2>&1 || true
docker run -d --name "$NAME" -p "127.0.0.1:$PORT:8080" \
  -e SKIP_DEMO_DATA=true -e GEOSERVER_ADMIN_USER=admin -e GEOSERVER_ADMIN_PASSWORD=geoserver \
  -e EXTRA_JAVA_OPTS="-Xms512m -Xmx2g -Duser.timezone=UTC" \
  -v "$PROPS:/opt/geoserver_data/data/props:ro" "$IMAGE" >/dev/null
echo "Waiting for GeoServer..."
for _ in $(seq 1 120); do
  curl -fs "${AUTH[@]}" "$GS/rest/about/version.json" >/dev/null 2>&1 && break
  sleep 3
done

rest() { curl -sS -f "${AUTH[@]}" -H 'Content-Type: application/json' "$@" >/dev/null; }
rest -XPUT "$GS/rest/settings" -d '{"global":{"settings":{"numDecimals":15,"verbose":false,"verboseExceptions":false,"charset":"UTF-8"}}}'
curl -sS -f "${AUTH[@]}" -XPUT -H 'Content-Type: application/xml' "$GS/rest/services/wfs/settings" \
  -d '<wfs><enabled>true</enabled><serviceLevel>BASIC</serviceLevel><maxFeatures>1000000</maxFeatures><featureBounding>false</featureBounding></wfs>' >/dev/null
for ns in $NAMESPACES; do
  rest -XPOST "$GS/rest/namespaces" -d "{\"namespace\":{\"prefix\":\"$ns\",\"uri\":\"$(uri_of $ns)\",\"isolated\":false}}"
  rest -XPOST "$GS/rest/workspaces/$ns/datastores" -d "{\"dataStore\":{\"name\":\"$ns\",\"type\":\"Properties\",\"enabled\":true,\"connectionParameters\":{\"entry\":[{\"@key\":\"directory\",\"\$\":\"file:data/props/$ns\"},{\"@key\":\"namespace\",\"\$\":\"$(uri_of $ns)\"}]}}}"
  for t in $(types_of $ns); do
    rest -XPOST "$GS/rest/workspaces/$ns/datastores/$ns/featuretypes" \
      -d "{\"featureType\":{\"name\":\"$t\",\"nativeName\":\"$t\",\"srs\":\"EPSG:$(srs_of $ns)\",\"projectionPolicy\":\"FORCE_DECLARED\",\"numDecimals\":15}}"
  done
done
echo "GeoServer oracle ready at $GS"
