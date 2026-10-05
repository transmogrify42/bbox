#!/usr/bin/env bash
# Download OGC and W3C XML schemas used for validating WFS responses in tests.
#
# Usage: tests/fetch-ogc-schemas.sh [target-dir]
# Default target: ../target/ogc-schemas (relative to this crate)
# Tests look for the schemas in $BBOX_OGC_SCHEMAS or the default target dir.

set -euo pipefail

cd "$(dirname "$0")/.."
DEST="${1:-../target/ogc-schemas}"
mkdir -p "$DEST"
DEST="$(cd "$DEST" && pwd)"

if [ ! -f "$DEST/wfs/2.0/wfs.xsd" ]; then
    curl -sSfL -o "$DEST/schemas.zip" https://schemas.opengis.net/SCHEMAS_OPENGIS_NET.zip
    unzip -q -o "$DEST/schemas.zip" -d "$DEST"
    rm "$DEST/schemas.zip"
    unzip -q -o "$DEST/xlink/xlink-1_0_0.zip" -d "$DEST"
fi

mkdir -p "$DEST/w3c"
for f in 1999/xlink.xsd 2001/xml.xsd 2001/XMLSchema.xsd 2001/XMLSchema.dtd 2001/datatypes.dtd; do
    if [ ! -f "$DEST/w3c/$f" ]; then
        mkdir -p "$DEST/w3c/$(dirname $f)"
        curl -sSfL -o "$DEST/w3c/$f" "https://www.w3.org/$f"
    fi
done

# Use absolute file locations everywhere, so that libxml2 sees each schema under one URI
if [ ! -f "$DEST/.rewritten" ]; then
    find "$DEST" -name '*.xsd' -print0 | xargs -0 perl -pi -e "s#https?://schemas\.opengis\.net/#file://$DEST/#g; s#http://www\.w3\.org/(1999/xlink\.xsd|2001/xml\.xsd|2001/XMLSchema\.xsd)#file://$DEST/w3c/\$1#g"
    touch "$DEST/.rewritten"
fi

cat > "$DEST/catalog.xml" <<EOF
<?xml version="1.0"?>
<catalog xmlns="urn:oasis:names:tc:entity:xmlns:xml:catalog">
  <rewriteURI uriStartString="http://schemas.opengis.net/" rewritePrefix="file://$DEST/"/>
  <rewriteSystem systemIdStartString="http://schemas.opengis.net/" rewritePrefix="file://$DEST/"/>
  <rewriteURI uriStartString="https://schemas.opengis.net/" rewritePrefix="file://$DEST/"/>
  <rewriteSystem systemIdStartString="https://schemas.opengis.net/" rewritePrefix="file://$DEST/"/>
  <rewriteURI uriStartString="http://www.w3.org/" rewritePrefix="file://$DEST/w3c/"/>
  <rewriteSystem systemIdStartString="http://www.w3.org/" rewritePrefix="file://$DEST/w3c/"/>
</catalog>
EOF

echo "OGC schemas available in $DEST"
