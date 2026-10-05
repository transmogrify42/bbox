# Feature server configuration

## Datasources

```toml
[[datasource]]
name = "mvtbenchdb"
[datasource.postgis]
url = "postgresql://mvtbench:mvtbench@127.0.0.1:5439/mvtbench"

[[datasource]]
name = "ne_extracts"
[datasource.gpkg]
path = "../data/ne_extracts.gpkg"
```

## Collections with auto discovery

```toml
[[collections.postgis]]
url = "postgresql://mvtbench:mvtbench@127.0.0.1:5439/mvtbench"

[[collections.directory]]
dir = "../data" # Relative to configuration file
```

## Collections

```toml
[[collection]]
name = "populated_places"
title = "populated places"
description = "Natural Earth populated places"
[collection.gpkg]
datasource = "ne_extracts"
table_name = "ne_10m_populated_places"
```

With custom SQL query:
```toml
[[collection]]
name = "populated_places_names"
title = "populated places names"
description = "Natural Earth populated places"
[collection.gpkg]
datasource = "ne_extracts"
sql = "SELECT fid, name, geom FROM ne_10m_populated_places"
geometry_field = "geom"
fid_field = "fid"
```

Collections with a PostGIS datasource:
```toml
[[collection]]
name = "states_provinces_lines"
title = "States/provinces borders"
description = "Natural Earth states/provinces borders"
[collection.postgis]
datasource = "mvtbenchdb"
table_name = "ne_10m_admin_1_states_provinces_lines"

[[collection]]
name = "country_labels"
title = "Country names"
description = "Natural Earth country names"
[collection.postgis]
datasource = "mvtbenchdb"
sql = "SELECT fid, abbrev, name, wkb_geometry FROM ne_10m_admin_0_country_points"
geometry_field = "wkb_geometry"
fid_field = "fid"
```

With queriable fields:
```toml
[[collection]]
name = "gpstracks"
title = "GPS tracks"
description = "Daily GPS tracks"
[collection.postgis]
datasource = "trackingdb"
sql = "SELECT id, date, ST_Point(lon, lat, 4326) AS geom FROM gpslog"
geometry_field = "geom"
fid_field = "id"
queryable_fields = ["date"]
```

Queriable fields are passed by name: `/collections/gpstracks/items?date=2024-11-08`

Temporal filters can be applied by configuring `temporal_field` and optionally `temporal_end_field`.

## ClickHouse

ClickHouse tables are served with the native protocol (`tcp://`, default port 9000) or HTTP
(`http://`, default port 8123). The database is the URL path; the optional `compression`
parameter selects `zstd` (default), `lz4` or `none` for the native protocol.

```toml
[[datasource]]
name = "ch"
[datasource.clickhouse]
url = "tcp://user:password@127.0.0.1:9000/mydb"

[[collection]]
name = "observations"
[collection.clickhouse]
datasource = "ch"
table_name = "observations"
fid_field = "fid"
```

Geometries can be stored as ClickHouse geo types (`Point`, `Ring`, `Polygon`, `MultiPolygon`,
`LineString`, `MultiLineString`), as WKB or WKT strings (`geometry_field`, `geometry_format`
`wkb` / `wkt`), or as longitude/latitude columns (`lon_field`, `lat_field`). `srid` sets the
CRS (default: 4326). A custom query can be configured with `sql`.

## WFS

The feature server provides a read-only Web Feature Service (WFS 1.0.0, 1.1.0, 2.0.0 / 2.0.2)
at `/wfs` for all collections. It is enabled by default.

```toml
[wfs]
enabled = true
title = "My WFS"
abstract = "Features served by BBOX"
keywords = ["roads", "buildings"]
fees = "NONE"
access_constraints = "NONE"
# Namespace of feature types (collections) without explicit namespace
namespace_prefix = "app"
namespace_uri = "https://example.com/app"
# Default number of features per response (CountDefault). Unlimited if not set.
count_default = 10000
# Additional CRS offered for output (EPSG codes)
other_crs = [4326, 3857]
# Resolve xlinks to remote resources (resolve=remote / all)
remote_resolve = true
# Default resolve timeout in seconds
resolve_timeout = 300
# Strict OGC CITE compliance (SERVICE parameter required); like GeoServer's `citeCompliant`
cite_compliant = false
# gml:boundedBy of collections and features (extra pass over the result); like GeoServer's `featureBounding`
feature_bounding = false

[wfs.provider]
name = "My Organization"
site = "https://example.com"
```

### Namespaces and application schemas

Collections can be grouped into additional namespaces. A GML application schema (XSD) defines
the feature types of a namespace; collections are matched to schema feature types by name.
Without schema, feature types are derived from the collection columns. Columns named `name`,
`description` or `boundedBy` (geometry) are mapped to the standard GML properties (GML 3).

```toml
[[wfs.namespace]]
prefix = "sf"
uri = "http://cite.opengeospatial.org/gmlsf"
schema = "cite-gmlsf2.xsd"

[[wfs.namespace]]
prefix = "ne"
uri = "http://www.naturalearthdata.com"
collections = ["populated_places", "lakes"]
```

### Caching

Capabilities documents are rendered once; capabilities, schemas and stored query lists carry an
`ETag` and support `If-None-Match` (304 Not Modified). Feature responses and counts can be
cached in memory. Cached responses do not reflect data changes until they expire.

```toml
[wfs.cache]
# Time to live of cached responses in seconds (0: response cache disabled)
response_ttl = 60
# Larger responses are streamed without caching
max_response_bytes = 1048576
# Total size of cached responses
capacity_bytes = 67108864
# Cache responses of these collections only (empty: all)
collections = ["observations"]
# Time to live of cached feature counts (numberMatched) in seconds
count_ttl = 300
# Cache-Control max-age header
max_age = 60
```
