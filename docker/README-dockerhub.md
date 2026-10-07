# BBOX with WFS

Minimal Alpine image of [BBOX](https://www.bbox.earth/) (`bbox-server`) with a read-only
Web Feature Service (WFS 1.0.0, 1.1.0, 2.0.0/2.0.2) and ClickHouse support in the feature server.

* Source: https://github.com/transmogrify42/bbox (branch `wfs`)
* Platforms: `linux/amd64`, `linux/arm64`
* Tags: `latest`, `<bbox version>-wfs<wfs version>` (e.g. `0.6.2-wfs0.1.0`)
* Runs as non-root user `bbox` (uid 1000), listens on port 8080

## Quick start

```shell
docker run --rm -p 8080:8080 \
  -v $PWD/bbox.toml:/var/www/bbox.toml:ro \
  -v $PWD/data:/var/www/data:ro \
  gogeospatial/bbox
```

* Web UI: http://127.0.0.1:8080/
* WFS: http://127.0.0.1:8080/wfs
* OGC API Features: http://127.0.0.1:8080/collections

The configuration is read from `/var/www/bbox.toml` (or the file in `BBOX_CONFIG`). Relative
paths in the configuration are resolved relative to the configuration file. Single values can be
overridden with environment variables, e.g. `-e BBOX_WFS__TITLE="My WFS"`.

## Adding a WFS layer

Every feature server collection is published as a WFS feature type. To add a layer, declare a
datasource and a collection in `bbox.toml`. Datasources can be GeoPackage, PostGIS or
ClickHouse.

### GeoPackage

```toml
[[datasource]]
name = "basedata"
[datasource.gpkg]
path = "data/basedata.gpkg"

# Layer from a table
[[collection]]
name = "roads"
title = "Roads"
description = "Road network"
[collection.gpkg]
datasource = "basedata"
table_name = "roads"

# Layer from a SQL query (geometry_field and fid_field required)
[[collection]]
name = "major_roads"
title = "Major roads"
[collection.gpkg]
datasource = "basedata"
sql = "SELECT fid, name, class, geom FROM roads WHERE class = 'motorway'"
geometry_field = "geom"
fid_field = "fid"
```

### PostGIS

```toml
[[datasource]]
name = "gisdb"
[datasource.postgis]
url = "postgresql://user:password@dbhost:5432/gis"

# Layer from a table (feature id and geometry column detected from geometry_columns)
[[collection]]
name = "buildings"
title = "Buildings"
[collection.postgis]
datasource = "gisdb"
table_schema = "public"
table_name = "buildings"

# Layer from a SQL query
[[collection]]
name = "tall_buildings"
title = "Tall buildings"
[collection.postgis]
datasource = "gisdb"
sql = "SELECT id, name, height, geom FROM public.buildings WHERE height > 50"
geometry_field = "geom"
fid_field = "id"
```

Filters on plain columns (e.g. `cql_filter=height>100`) are sent to PostGIS with bound
parameters, so indexes on those columns are used. For a database on the Docker host use
`dbhost` = `host.docker.internal` (Docker Desktop) or run with `--network host` (Linux).

### ClickHouse

Native protocol (`tcp://`, port 9000) or HTTP (`http://`, port 8123); the database is the URL
path.

```toml
[[datasource]]
name = "ch"
[datasource.clickhouse]
url = "tcp://user:password@chhost:9000/analytics"
# url = "http://user:password@chhost:8123/analytics"

# Layer from a table with a native geo type column (Point, LineString, Polygon, ...)
[[collection]]
name = "sensors"
title = "Sensors"
[collection.clickhouse]
datasource = "ch"
table_name = "sensors"
fid_field = "id"
geometry_field = "location"

# Layer from longitude/latitude columns
[[collection]]
name = "observations"
title = "Observations"
[collection.clickhouse]
datasource = "ch"
table_name = "observations"
fid_field = "id"
lon_field = "lon"
lat_field = "lat"
srid = 4326
```

Geometries stored as WKB or WKT strings are configured with `geometry_field` and
`geometry_format = "wkb"` or `"wkt"`. A custom query can be set with `sql`.

### Auto discovery

All tables of a directory of GeoPackages or of a PostGIS database can be published at once:

```toml
[[collections.directory]]
dir = "data"

[[collections.postgis]]
url = "postgresql://user:password@dbhost:5432/gis"
```

Layers are published in the namespace `bbox` by default (`typeNames=bbox:roads`). Own
namespaces:

```toml
[wfs]
title = "Transport WFS"
namespace_prefix = "app"
namespace_uri = "https://example.com/app"

[[wfs.namespace]]
prefix = "tn"
uri = "https://example.com/transport"
collections = ["roads", "major_roads"]
```

Check the layer:

```shell
curl -s "http://127.0.0.1:8080/wfs?service=WFS&version=2.0.0&request=GetCapabilities" | grep -o "<wfs:Name>[^<]*</wfs:Name>"
curl "http://127.0.0.1:8080/wfs?service=WFS&version=2.0.0&request=DescribeFeatureType&typeNames=tn:roads"
curl "http://127.0.0.1:8080/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=tn:roads&count=5&outputFormat=application/json"
```

In QGIS: *Layer > Add Layer > Add WFS Layer > New*, URL `http://<host>:8080/wfs`.

If a collection cannot be loaded (missing table or column, invalid SQL), the server exits with
`Error during initialization` and logs the failing query. Mounted files must be readable for
uid 1000. Databases on the Docker host are reachable as `host.docker.internal` (Docker Desktop).

## Full documentation

The complete guide, including PostGIS and ClickHouse layers, application schemas, caching and
troubleshooting, is included in the image:

```shell
docker run --rm --entrypoint cat gogeospatial/bbox /usr/share/doc/bbox/feature-server/wfs-layers.md
docker run --rm --entrypoint ls gogeospatial/bbox -R /usr/share/doc/bbox
```

and in the repository under
[website/content/docs/feature-server](https://github.com/transmogrify42/bbox/tree/wfs/website/content/docs/feature-server).

## WFS features

* Operations: GetCapabilities, DescribeFeatureType, GetFeature, GetPropertyValue, GetGmlObject
  (1.1), stored queries (2.0); KVP, XML and SOAP encodings. Read-only (no Transaction/Lock).
* Filter Encoding 1.x/2.0 and CQL/ECQL, translated to SQL with bound parameters.
* Output: GML 2/3.1/3.2, GeoJSON/JSONP, CSV, Shapefile, KML.
* GeoServer compatible vendor parameters (`CQL_FILTER`, `featureid`, `format_options`, ...).
* Passes OGC ets-wfs10, ets-wfs20 and DGIWG WFS 2.0 test suites.

## License

MIT OR Apache-2.0
