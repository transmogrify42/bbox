# BBOX OGC API Features Service

Asynchronous OGC API Features server implementation.

Features:
- [x] OGC API - Features - Part 1: Core 1.0
- [ ] OGC API - Features - Part 2: Coordinate Reference Systems by Reference 1.0
- [x] OpenAPI endpoint
- [x] Builtin storage backends: PostGIS, GeoPackage, ClickHouse
- [x] Output formats: GeoJSON
- [x] WFS 1.0.0, 1.1.0, 2.0.0 (read-only) with GML 2/3.1/3.2, GeoJSON, CSV, Shapefile and KML output


## Configuration

See [Documentation](https://www.bbox.earth/docs/feature-server/configuration/) for examples.

## Usage

Run feature server with bbox.toml configuration:

    cargo run serve

or with a custom configuration:

    cargo run -- --config=bbox-pg.toml serve

Inspect collections:

    x-www-browser http://127.0.0.1:8080/collections

Feature requests:

    curl -s http://127.0.0.1:8080/collections/populated_places/items | jq .

    curl -s http://127.0.0.1:8080/collections/populated_places_names/items/2 | jq .
