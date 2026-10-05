#!/usr/bin/env bash
# Download the GeoServer WFS test datasets (GeoServer `.properties` files, GPL-2.0 licensed,
# not part of this repository) used by the gs_* tests.
#
# Usage: tests/fetch-geoserver-data.sh [target-dir]
# Default target: ../target/geoserver-data (relative to this crate); override with BBOX_GEOSERVER_DATA.

set -euo pipefail

cd "$(dirname "$0")/.."
DEST="${1:-${BBOX_GEOSERVER_DATA:-../target/geoserver-data}}"
# pinned GeoServer commit (main, 2026-10-05)
COMMIT=405a0355d0d8440d6f45ad908d6e24138c581832
BASE="https://raw.githubusercontent.com/geoserver/geoserver/$COMMIT/src"

mkdir -p "$DEST/main"
for t in AggregateGeoFeature BasicPolygons Bridges Buildings Deletes DividedRoutes Fifteen Forests \
    GenericEntity Geometryless Inserts Lakes Lines Locks MLines MPoints MPolygons MapNeatline \
    MarsPoi NamedPlaces Nulls Other Points Polygons Ponds PrimitiveGeoFeature \
    PrimitiveGeoFeatureId RoadSegments Seven Streams TemporalData TimeElevation Updates \
    curvelines curvemultilines curvepolygons; do
    f="$DEST/main/$t.properties"
    [ -f "$f" ] || curl -sSfL -o "$f" "$BASE/main/src/test/java/org/geoserver/data/test/$t.properties"
done
echo "GeoServer test data in $(cd "$DEST" && pwd)"
