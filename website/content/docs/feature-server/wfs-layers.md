---
weight: 9
---

# Adding a WFS layer

Every collection of the feature server is published as a WFS feature type ("layer") at `/wfs`.
Adding a WFS layer therefore means adding a collection to the configuration file (`bbox.toml`).
There is no separate WFS layer registration.

The steps are:

1. Declare a datasource (GeoPackage, PostGIS or ClickHouse).
2. Add a `[[collection]]` that points at a table or SQL query of that datasource.
3. Optionally assign the collection to a namespace and tune the `[wfs]` settings.
4. Restart the server and check the layer in GetCapabilities.

The collection `name` becomes the feature type name. Without further configuration it is
published in the default namespace with prefix `bbox`, so the collection `roads` is requested as
`typeNames=bbox:roads` (WFS 2.0) or `typeName=bbox:roads` (WFS 1.x).

## Step 1: datasource

A datasource is declared once and can be used by many collections. Relative paths are resolved
relative to the directory of the configuration file.

GeoPackage:

```toml
[[datasource]]
name = "basedata"
[datasource.gpkg]
path = "data/basedata.gpkg"
```

PostGIS:

```toml
[[datasource]]
name = "gisdb"
[datasource.postgis]
url = "postgresql://user:password@dbhost:5432/gis"
```

ClickHouse (native protocol `tcp://`, port 9000, or HTTP `http://`, port 8123):

```toml
[[datasource]]
name = "ch"
[datasource.clickhouse]
url = "tcp://user:password@chhost:9000/mydb"
```

## Step 2: collection

### Table

The simplest layer is a table. The feature id and geometry columns are detected from the table
metadata (GeoPackage `gpkg_geometry_columns`, PostGIS `geometry_columns`).

```toml
[[collection]]
name = "roads"
title = "Roads"
description = "Road network"
[collection.gpkg]
datasource = "basedata"
table_name = "roads"
```

The same layer from PostGIS, with an explicit schema:

```toml
[[collection]]
name = "roads"
title = "Roads"
[collection.postgis]
datasource = "gisdb"
table_schema = "transport"
table_name = "roads"
```

`title` is used as the feature type title and `description` as its abstract in the
capabilities document.

### SQL query

A layer can be defined by a query, for example to select a subset of the columns or rows, or to
build a geometry. For queries, `geometry_field` and `fid_field` must be set.

```toml
[[collection]]
name = "major_roads"
title = "Major roads"
[collection.postgis]
datasource = "gisdb"
sql = "SELECT id, name, class, geom FROM transport.roads WHERE class IN ('motorway', 'primary')"
geometry_field = "geom"
fid_field = "id"
```

WFS filters on columns of a query layer are still translated to SQL (the query is used as a
subquery), so indexes on the underlying table remain usable.

### ClickHouse table

ClickHouse geometries can be native geo types, WKB/WKT strings or longitude/latitude columns:

```toml
[[collection]]
name = "observations"
title = "Observations"
[collection.clickhouse]
datasource = "ch"
table_name = "observations"
fid_field = "fid"
lon_field = "lon"
lat_field = "lat"
srid = 4326
```

For a String geometry column use `geometry_field` and `geometry_format = "wkb"` or `"wkt"`.

### Automatic discovery

Instead of configuring each layer, all tables of a GeoPackage directory or a PostGIS database can
be published. Every table with a geometry column becomes a layer named after the table:

```toml
[[collections.directory]]
dir = "data"

[[collections.postgis]]
url = "postgresql://user:password@dbhost:5432/gis"
```

### Layer without geometry

Tables without a geometry column (GeoPackage attribute tables, PostGIS tables, ClickHouse
tables without geometry configuration) are served as feature types without a geometry property.
They can be queried with attribute filters and used in joins.

## Step 3: namespace (optional)

To publish layers under their own prefix and namespace URI, change the default namespace or add
additional namespaces:

```toml
[wfs]
title = "Transport WFS"
namespace_prefix = "app"                  # default: bbox
namespace_uri = "https://example.com/app"

[[wfs.namespace]]
prefix = "tn"
uri = "https://example.com/transport"
collections = ["roads", "major_roads"]
```

With this configuration the layers are requested as `typeNames=tn:roads`, all other layers as
`typeNames=app:<name>`.

A namespace can reference a GML application schema (`schema = "transport.xsd"`). Layers whose
name matches a feature type of the schema are then described by that schema instead of the
column types.

Other useful `[wfs]` settings for layers (see [configuration](../configuration/#wfs)):

| Setting | Purpose |
|---|---|
| `count_default` | Maximum number of features per response if the client sets no limit |
| `other_crs` | Additional EPSG codes offered for output (default `[4326, 3857]`) |
| `feature_bounding` | Write `gml:boundedBy` per feature and collection |
| `[wfs.cache]` | Cache responses and counts of selected layers |

## Step 4: check the layer

Restart the server after changing the configuration. The log lists the collections found.
Then check that the layer is published (the output lists the feature type names, e.g.
`<wfs:Name>bbox:roads</wfs:Name>`):

    curl -s "http://127.0.0.1:8080/wfs?service=WFS&version=2.0.0&request=GetCapabilities" | grep -o "<wfs:Name>[^<]*</wfs:Name>"

Describe the layer schema (attribute names and types):

    curl "http://127.0.0.1:8080/wfs?service=WFS&version=2.0.0&request=DescribeFeatureType&typeNames=bbox:roads"

Fetch a few features:

    curl "http://127.0.0.1:8080/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=bbox:roads&count=5"

    curl "http://127.0.0.1:8080/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=bbox:roads&count=5&outputFormat=application/json"

The layer is also available as OGC API Features collection at `/collections/roads/items`.

## Using the layer in a client

In QGIS, add a connection with *Layer > Add Layer > Add WFS Layer > New* and the URL
`http://<host>:8080/wfs`. Select version *2.0* (or *Maximum*), *Connect* and add the layer.

Other clients (OpenLayers, Leaflet plugins, ArcGIS, GDAL) use the same URL. With GDAL:

    ogrinfo WFS:"http://127.0.0.1:8080/wfs" bbox:roads -so

## Docker

In the Docker image the working directory is `/var/www` and the configuration file is read from
`/var/www/bbox.toml` (or the file given by `BBOX_CONFIG`). Mount the configuration and the data
files into the container:

```shell
docker run --rm -p 8080:8080 \
  -v $PWD/bbox.toml:/var/www/bbox.toml:ro \
  -v $PWD/data:/var/www/data:ro \
  gogeospatial/bbox
```

The server runs as user `bbox` (uid 1000), so mounted files must be readable for that user. A
GeoPackage in a read-only mount must not have pending `-wal` changes. Databases on the Docker host
are reachable as `host.docker.internal` (Docker Desktop) or with `--network host` (Linux).

Single configuration values can be overridden with environment variables (prefix `BBOX_`,
`__` as section separator):

```shell
docker run --rm -p 8080:8080 -e BBOX_WFS__TITLE="Transport WFS" ...
```

This guide is included in the image at `/usr/share/doc/bbox/feature-server/wfs-layers.md`
(all BBOX documentation is under `/usr/share/doc/bbox/`):

    docker run --rm --entrypoint cat gogeospatial/bbox /usr/share/doc/bbox/feature-server/wfs-layers.md

## Troubleshooting

| Symptom | Cause |
|---|---|
| Server exits with `Error during initialization` | A collection could not be loaded (missing table or column, invalid SQL). The log shows the failing query. |
| Layer missing in GetCapabilities | Table not found by auto discovery, or `[wfs] enabled = false`. Check the server log. |
| `InvalidParameterValue` for `typeNames` | Wrong prefix. Use the prefix shown in GetCapabilities (`bbox:` by default). |
| No geometry in the output | `geometry_field` missing for a SQL query layer, or the geometry column is not registered (PostGIS `geometry_columns`). |
| Duplicate or missing feature ids | `fid_field` is not a unique column. |
| Slow filters | Filtered column has no index. Filters on plain columns are passed to the database with bound parameters. |
