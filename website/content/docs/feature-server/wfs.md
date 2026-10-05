---
weight: 8
---

# Web Feature Service (WFS)

The feature server provides a read-only WFS at `/wfs` for the same collections as OGC API
Features, from GeoPackage, PostGIS and ClickHouse datasources.

## Versions and operations

| Version | Operations |
|---|---|
| WFS 1.0.0 | GetCapabilities, DescribeFeatureType, GetFeature |
| WFS 1.1.0 | GetCapabilities, DescribeFeatureType, GetFeature, GetGmlObject |
| WFS 2.0.0 / 2.0.2 | GetCapabilities, DescribeFeatureType, GetFeature, GetPropertyValue, ListStoredQueries, DescribeStoredQueries, CreateStoredQuery, DropStoredQuery |

Requests are accepted as KVP (GET or form POST), XML (POST) and SOAP 1.1/1.2 (WFS 2.0).
Transaction, LockFeature and GetFeatureWithLock are not supported (read-only service).

WFS 2.0 conformance classes: Simple, Basic WFS, Inheritance, Remote resolve, Response paging,
Standard joins, Spatial joins, Temporal joins, Manage stored queries, KVP/XML/SOAP encodings.
Filter Encoding 2.0: Query, Ad hoc query, Functions, Resource identification, Version navigation
(read-only), Minimum/standard spatial and temporal filters, Sorting, Extended operators.

## Output formats

* GML 2.1.2, 3.1.1, 3.2 (`text/xml; subtype=gml/2.1.2`, `gml2`, `gml3`, `gml32`, ...)
* GeoJSON (`application/json`, `json`), JSONP (`text/javascript`, `format_options=callback:fn`)
* CSV (`csv`, `format_options=csvSeparator:semicolon;filename:x`)
* Shapefile (`SHAPE-ZIP`, `format_options=CHARSET:ISO-8859-1;FILENAME:x.zip;PRJFILEFORMAT:ESRI`)
* KML (`application/vnd.google-earth.kml+xml`)

3D coordinates are written with `srsDimension="3"`. Curved PostGIS geometries are linearized.

## GeoServer compatibility

Vendor parameters known from GeoServer are supported: `CQL_FILTER` (ECQL), `featureid`,
`startIndex` in WFS 1.x, `format_options`, `propertyName=*`, `namespace=<prefix>` in
GetCapabilities, `EXCEPTIONS=application/json | text/javascript`, GeoJSON `id_policy` and
paging links, `storedQueryId`. Web Mercator aliases (EPSG:900913, 102100, 3785) are accepted.

## Filters and SQL

Filters (Filter Encoding and CQL) are translated into SQL wherever possible, using bound
parameters on plain columns so that database indexes are used; only untranslatable parts are
evaluated in the server.

## Examples

    curl "http://127.0.0.1:8080/wfs?service=WFS&version=2.0.0&request=GetCapabilities"

    curl "http://127.0.0.1:8080/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=bbox:populated_places&count=10"

    curl "http://127.0.0.1:8080/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=bbox:populated_places&cql_filter=pop_max>1000000&outputFormat=application/json"

## Testing

The implementation is tested against the OGC executable test suites (ets-wfs10, ets-wfs11,
ets-wfs20, DGIWG WFS 2.0 profile), against GeoServer's WFS test cases and differentially against
a GeoServer instance (see `bbox-feature-server/tests`).
