//! Test fixtures: GeoPackages created from GML application schemas and loaded with
//! the OGC CITE datasets (which are distributed as WFS Transaction Insert documents).

use bbox_feature_server::wfs::crs::{self, Crs};
use bbox_feature_server::wfs::gml;
use bbox_feature_server::wfs::model::*;
use bbox_feature_server::wfs::wkb::{encode_gpkg, encode_gpkg_z};
use bbox_feature_server::wfs::xml::serialize_node;
use bbox_feature_server::wfs::xsd::read_feature_types_from_file;
use geo::BoundingRect;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Executor, SqlitePool};
use std::path::{Path, PathBuf};

pub fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

/// Application schema of a namespace
pub struct SchemaSpec {
    pub prefix: &'static str,
    pub uri: &'static str,
    pub xsd: PathBuf,
}

fn sql_type(p: &PropertyDef) -> &'static str {
    if p.is_xml() {
        return "TEXT";
    }
    match &p.value_type {
        ValueType::Geometry(g) => g.sf_name(),
        ValueType::Integer => "INTEGER",
        ValueType::Double => "REAL",
        ValueType::Boolean => "BOOLEAN",
        ValueType::Date => "DATE",
        ValueType::DateTime => "DATETIME",
        _ => "TEXT",
    }
}

fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// Companion column storing the gml:id of a geometry
pub fn geometry_id_column(column: &str) -> String {
    format!("{column}_gmlid")
}

async fn create_gpkg(path: &Path) -> SqlitePool {
    let _ = std::fs::remove_file(path);
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .unwrap();
    pool.execute(
        r#"
        PRAGMA application_id = 1196444487;
        PRAGMA user_version = 10300;
        CREATE TABLE gpkg_spatial_ref_sys (
          srs_name TEXT NOT NULL, srs_id INTEGER PRIMARY KEY, organization TEXT NOT NULL,
          organization_coordsys_id INTEGER NOT NULL, definition TEXT NOT NULL, description TEXT);
        INSERT INTO gpkg_spatial_ref_sys VALUES
          ('Undefined cartesian SRS', -1, 'NONE', -1, 'undefined', NULL),
          ('Undefined geographic SRS', 0, 'NONE', 0, 'undefined', NULL);
        CREATE TABLE gpkg_contents (
          table_name TEXT NOT NULL PRIMARY KEY, data_type TEXT NOT NULL, identifier TEXT UNIQUE,
          description TEXT DEFAULT '', last_change DATETIME NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
          min_x DOUBLE, min_y DOUBLE, max_x DOUBLE, max_y DOUBLE, srs_id INTEGER);
        CREATE TABLE gpkg_geometry_columns (
          table_name TEXT NOT NULL, column_name TEXT NOT NULL, geometry_type_name TEXT NOT NULL,
          srs_id INTEGER NOT NULL, z TINYINT NOT NULL, m TINYINT NOT NULL,
          CONSTRAINT pk_geom_cols PRIMARY KEY (table_name, column_name));
        "#,
    )
    .await
    .unwrap();
    pool
}

async fn register_srs(pool: &SqlitePool, srid: u16) {
    sqlx::query(
        "INSERT OR IGNORE INTO gpkg_spatial_ref_sys VALUES (?, ?, 'EPSG', ?, 'undefined', NULL)",
    )
    .bind(format!("EPSG:{srid}"))
    .bind(srid as i64)
    .bind(srid as i64)
    .execute(pool)
    .await
    .unwrap();
}

async fn create_table(pool: &SqlitePool, ft: &FeatureTypeDef, srid: u16) {
    let table = &ft.name.local;
    let mut cols = vec![
        "fid INTEGER PRIMARY KEY AUTOINCREMENT".to_string(),
        "gml_id TEXT UNIQUE".to_string(),
    ];
    for p in &ft.properties {
        cols.push(format!("{} {}", quote(&p.column), sql_type(p)));
        if p.is_geometry() && !(p.name.is_gml() && p.name.local == "boundedBy") {
            cols.push(format!("{} TEXT", quote(&geometry_id_column(&p.column))));
            cols.push(format!("{} TEXT", quote(&format!("{}_gmlmeta", p.column))));
            cols.push(format!("{} TEXT", quote(&format!("{}_gmlxml", p.column))));
        }
    }
    let ddl = format!("CREATE TABLE {} ({})", quote(table), cols.join(", "));
    pool.execute(ddl.as_str()).await.unwrap();
    sqlx::query("INSERT INTO gpkg_contents (table_name, data_type, identifier, srs_id) VALUES (?, 'features', ?, ?)")
        .bind(table)
        .bind(table)
        .bind(srid as i64)
        .execute(pool)
        .await
        .unwrap();
    // default geometry first, so that single-geometry readers pick it
    let mut geoms: Vec<&PropertyDef> = ft.properties.iter().filter(|p| p.is_geometry()).collect();
    geoms.sort_by_key(|p| p.name.is_gml() && p.name.local == "boundedBy");
    for p in geoms {
        let ValueType::Geometry(gt) = p.value_type else {
            unreachable!()
        };
        sqlx::query("INSERT INTO gpkg_geometry_columns VALUES (?, ?, ?, ?, 0, 0)")
            .bind(table)
            .bind(&p.column)
            .bind(gt.sf_name())
            .bind(srid as i64)
            .execute(pool)
            .await
            .unwrap();
    }
}

/// Feature type with its loaded schema
struct TypeInfo {
    ft: FeatureTypeDef,
}

/// Build a GeoPackage with one table per feature type of the schemas and load data files
pub async fn build_gpkg(path: &Path, schemas: &[SchemaSpec], data: &[PathBuf], srid: u16) {
    let pool = create_gpkg(path).await;
    register_srs(&pool, srid).await;
    let mut types = Vec::new();
    for spec in schemas {
        let fts = read_feature_types_from_file(spec.xsd.to_str().unwrap(), spec.prefix).unwrap();
        for ft in fts {
            assert_eq!(ft.name.ns, spec.uri);
            create_table(&pool, &ft, srid).await;
            types.push(TypeInfo { ft });
        }
    }
    for file in data {
        let text = std::fs::read_to_string(file).unwrap();
        load_inserts(&pool, &types, &text, srid).await;
    }
    for t in &types {
        update_extent(&pool, &t.ft).await;
    }
    pool.close().await;
}

async fn update_extent(pool: &SqlitePool, ft: &FeatureTypeDef) {
    use sqlx::Row;
    let mut rect: Option<geo::Rect<f64>> = None;
    for (_, geom) in ft
        .geometry_properties()
        .filter(|(_, p)| !(p.name.is_gml() && p.name.local == "boundedBy"))
    {
        let sql = format!(
            "SELECT {} FROM {}",
            quote(&geom.column),
            quote(&ft.name.local)
        );
        let rows = sqlx::query(&sql).fetch_all(pool).await.unwrap();
        for row in rows {
            let blob: Option<Vec<u8>> = row.try_get(0).unwrap();
            if let Some(blob) = blob {
                if let Ok(Some(g)) = bbox_feature_server::wfs::wkb::decode_gpkg(&blob) {
                    if let Some(r) = g.bounding_rect() {
                        rect = Some(match rect {
                            None => r,
                            Some(acc) => merge_rect(&acc, &r),
                        });
                    }
                }
            }
        }
    }
    if let Some(r) = rect {
        sqlx::query(
            "UPDATE gpkg_contents SET min_x=?, min_y=?, max_x=?, max_y=? WHERE table_name=?",
        )
        .bind(r.min().x)
        .bind(r.min().y)
        .bind(r.max().x)
        .bind(r.max().y)
        .bind(&ft.name.local)
        .execute(pool)
        .await
        .unwrap();
    }
}

fn find_property<'a>(ft: &'a FeatureTypeDef, node: &roxmltree::Node) -> Option<&'a PropertyDef> {
    let ns = node.tag_name().namespace().unwrap_or("");
    let local = node.tag_name().name();
    ft.properties
        .iter()
        .find(|p| p.name.local == local && (p.name.ns == ns || (p.name.is_gml() && is_gml_ns(ns))))
}

/// Parse geometry (or envelope for boundedBy) and transform to x/y in the table CRS
#[allow(clippy::type_complexity)]
fn geometry_value(
    node: roxmltree::Node,
    srid: u16,
    envelope: bool,
    default_srs: Option<&str>,
) -> Option<(
    geo::Geometry<f64>,
    Option<Vec<f64>>,
    Option<String>,
    Option<String>,
)> {
    let geom_node = node.children().find(|c| c.is_element())?;
    if geom_node.tag_name().name() == "null" {
        return None;
    }
    let gml_id = geom_node
        .attribute((gml::GML_NS, "id"))
        .or_else(|| geom_node.attribute((gml::GML32_NS, "id")))
        .map(str::to_string);
    let (mut geometry, srs_name, z) = if envelope {
        let (rect, srs) = gml::parse_envelope(geom_node).unwrap();
        (geo::Geometry::Polygon(rect.to_polygon()), srs, None)
    } else {
        let g = gml::parse_geometry(geom_node).unwrap();
        // srsName may be given on an inner geometry only
        let srs = g.srs_name.or_else(|| {
            geom_node
                .descendants()
                .find_map(|d| d.attribute("srsName"))
                .map(str::to_string)
        });
        (g.geometry, srs, g.z)
    };
    if let Some(name) = srs_name.or(default_srs.map(str::to_string)) {
        let c = Crs::parse(&name).unwrap();
        if c.swap_xy() {
            gml::swap_xy(&mut geometry);
        }
        if c.epsg != srid {
            crs::transform(&mut geometry, c.epsg, srid).unwrap();
        }
    }
    let meta: String = geom_node
        .children()
        .filter(|c| c.is_element() && is_gml_ns(c.tag_name().namespace().unwrap_or("")))
        .filter(|c| matches!(c.tag_name().name(), "description" | "name"))
        .map(serialize_node)
        .collect();
    Some((geometry, z, gml_id, (!meta.is_empty()).then_some(meta)))
}

/// Load features of all wfs:Insert elements of a transaction document
async fn load_inserts(pool: &SqlitePool, types: &[TypeInfo], xml: &str, srid: u16) {
    let doc = roxmltree::Document::parse(xml).unwrap();
    for insert in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "Insert")
    {
        for feature in insert.children().filter(|c| c.is_element()) {
            let ns = feature.tag_name().namespace().unwrap_or("");
            let local = feature.tag_name().name();
            let Some(info) = types
                .iter()
                .find(|t| t.ft.name.ns == ns && t.ft.name.local == local)
            else {
                panic!("feature type {{{ns}}}{local} not in schema");
            };
            insert_feature(pool, &info.ft, feature, srid, insert.attribute("srsName")).await;
        }
    }
}

enum Bind {
    Text(String),
    Int(i64),
    Real(f64),
    Blob(Vec<u8>),
}

async fn insert_feature(
    pool: &SqlitePool,
    ft: &FeatureTypeDef,
    feature: roxmltree::Node<'_, '_>,
    srid: u16,
    default_srs: Option<&str>,
) {
    let id = feature
        .attribute((gml::GML_NS, "id"))
        .or_else(|| feature.attribute((gml::GML32_NS, "id")))
        .or_else(|| feature.attribute("fid"))
        .map(str::to_string);
    let mut columns: Vec<String> = vec!["gml_id".to_string()];
    let mut values: Vec<Option<Bind>> = vec![id.map(Bind::Text)];
    let mut fragments: Vec<(String, String)> = Vec::new();
    for prop_node in feature.children().filter(|c| c.is_element()) {
        let Some(prop) = find_property(ft, &prop_node) else {
            panic!(
                "property {} not in {}",
                prop_node.tag_name().name(),
                ft.name.local
            );
        };
        if prop.is_xml() {
            let frag = serialize_node(prop_node);
            match fragments.iter_mut().find(|(c, _)| *c == prop.column) {
                Some((_, f)) => f.push_str(&frag),
                None => fragments.push((prop.column.clone(), frag)),
            }
            continue;
        }
        let envelope = prop.name.is_gml() && prop.name.local == "boundedBy";
        // geometries with xlinks are kept as verbatim GML
        if prop.is_geometry()
            && !envelope
            && prop_node.descendants().any(|d| {
                d.attribute(("http://www.w3.org/1999/xlink", "href"))
                    .is_some()
            })
        {
            columns.push(format!("{}_gmlxml", prop.column));
            values.push(Some(Bind::Text(serialize_node(prop_node))));
            continue;
        }
        let bind = match &prop.value_type {
            ValueType::Geometry(_) => {
                match geometry_value(prop_node, srid, envelope, default_srs) {
                    Some((g, z, gml_id, meta)) => {
                        if let Some(gid) = gml_id {
                            columns.push(geometry_id_column(&prop.column));
                            values.push(Some(Bind::Text(gid)));
                        }
                        if let Some(m) = meta.filter(|_| !envelope) {
                            columns.push(format!("{}_gmlmeta", prop.column));
                            values.push(Some(Bind::Text(m)));
                        }
                        Some(Bind::Blob(encode_gpkg_z(&g, z.as_deref(), srid as i32)))
                    }
                    None => None,
                }
            }
            ValueType::Integer => {
                let t = prop_node.text().unwrap_or("").trim().to_string();
                Some(t.parse::<i64>().map(Bind::Int).unwrap_or(Bind::Text(t)))
            }
            ValueType::Double => {
                let t = prop_node.text().unwrap_or("").trim().to_string();
                Some(t.parse::<f64>().map(Bind::Real).unwrap_or(Bind::Text(t)))
            }
            ValueType::Boolean => {
                let t = prop_node.text().unwrap_or("").trim();
                Some(Bind::Int(matches!(t, "true" | "1") as i64))
            }
            _ => Some(Bind::Text(prop_node.text().unwrap_or("").to_string())),
        };
        columns.push(prop.column.clone());
        values.push(bind);
    }
    for (col, frag) in fragments {
        columns.push(col);
        values.push(Some(Bind::Text(frag)));
    }
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        quote(&ft.name.local),
        columns
            .iter()
            .map(|c| quote(c))
            .collect::<Vec<_>>()
            .join(", "),
        vec!["?"; columns.len()].join(", ")
    );
    let mut q = sqlx::query(&sql);
    for v in values {
        q = match v {
            None => q.bind(None::<String>),
            Some(Bind::Text(t)) => q.bind(t),
            Some(Bind::Int(i)) => q.bind(i),
            Some(Bind::Real(r)) => q.bind(r),
            Some(Bind::Blob(b)) => q.bind(b),
        };
    }
    q.execute(pool).await.unwrap();
}

pub fn cite10_schemas() -> Vec<SchemaSpec> {
    let dir = data_dir().join("cite/wfs10");
    vec![
        SchemaSpec {
            prefix: "cdf",
            uri: "http://www.opengis.net/cite/data",
            xsd: dir.join("dataFeatures.xsd"),
        },
        SchemaSpec {
            prefix: "cgf",
            uri: "http://www.opengis.net/cite/geometry",
            xsd: dir.join("geometryFeatures.xsd"),
        },
        SchemaSpec {
            prefix: "ccf",
            uri: "http://www.opengis.net/cite/complex",
            xsd: dir.join("complexFeatures.xsd"),
        },
    ]
}

pub fn cite11_schemas() -> Vec<SchemaSpec> {
    let dir = data_dir().join("cite/wfs11");
    vec![SchemaSpec {
        prefix: "sf",
        uri: "http://cite.opengeospatial.org/gmlsf",
        xsd: dir.join("cite-gmlsf2.xsd"),
    }]
}

/// CITE WFS 1.0 dataset (cdf, cgf, ccf) in EPSG:32615
pub async fn cite10_gpkg(dir: &Path) -> PathBuf {
    let path = dir.join("cite10.gpkg");
    let data = data_dir().join("cite/wfs10");
    build_gpkg(
        &path,
        &cite10_schemas(),
        &[
            data.join("wfsBasicTestData.xml"),
            data.join("wfsComplexTestData.xml"),
            data.join("wfsTransactionTestData.xml"),
            data.join("wfsLockTestData.xml"),
        ],
        32615,
    )
    .await;
    path
}

/// CITE WFS 1.1 dataset (sf, GMLSF levels 0-2) in EPSG:4326
pub async fn cite11_gpkg(dir: &Path) -> PathBuf {
    let path = dir.join("cite11.gpkg");
    let data = data_dir().join("cite/wfs11");
    build_gpkg(
        &path,
        &cite11_schemas(),
        &[
            data.join("dataset-sf0-insert.xml"),
            data.join("dataset-sf1-insert.xml"),
            data.join("dataset-sf2-insert.xml"),
        ],
        4326,
    )
    .await;
    path
}

// ---------------------------------------------------------------- synthetic dataset

/// Deterministic synthetic observation record
#[derive(Clone, Debug)]
pub struct Observation {
    pub fid: i64,
    pub image_id: String,
    pub acquired: String,
    pub cloud: f64,
    pub label: Option<String>,
    pub lon: f64,
    pub lat: f64,
}

/// i-th synthetic observation (well distributed over the globe, hourly timestamps from 2020)
pub fn observation(i: i64) -> Observation {
    let lon = -180.0 + ((i * 7919) % 3600) as f64 / 10.0 + 0.05;
    let lat = -60.0 + ((i * 104729) % 1200) as f64 / 10.0 + 0.05;
    let h = (i as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .rotate_left(17);
    let acquired = chrono::DateTime::from_timestamp(1_577_836_800 + i * 3600, 0)
        .unwrap()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    Observation {
        fid: i + 1,
        image_id: format!("img-{h:016x}"),
        acquired,
        cloud: (i % 100) as f64,
        label: if i % 10 == 0 {
            None
        } else {
            Some(format!("class-{}", i % 5))
        },
        lon,
        lat,
    }
}

/// GeoPackage with tables `observations` (points) and `footprints` (polygons), n rows each
pub async fn synthetic_gpkg(path: &Path, n: i64) -> PathBuf {
    let pool = create_gpkg(path).await;
    register_srs(&pool, 4326).await;
    pool.execute(
        r#"
        CREATE TABLE observations (fid INTEGER PRIMARY KEY AUTOINCREMENT, geom POINT, image_id TEXT NOT NULL,
          acquired DATETIME, cloud REAL, label TEXT);
        CREATE INDEX observations_image_id ON observations (image_id);
        CREATE TABLE footprints (fid INTEGER PRIMARY KEY AUTOINCREMENT, geom POLYGON, image_id TEXT NOT NULL,
          area_km2 REAL);
        INSERT INTO gpkg_contents (table_name, data_type, identifier, srs_id, min_x, min_y, max_x, max_y)
          VALUES ('observations', 'features', 'observations', 4326, -180, -60, 180, 60),
                 ('footprints', 'features', 'footprints', 4326, -180.1, -60.1, 180.1, 60.1);
        INSERT INTO gpkg_geometry_columns VALUES ('observations', 'geom', 'POINT', 4326, 0, 0),
                                                 ('footprints', 'geom', 'POLYGON', 4326, 0, 0);
        CREATE VIRTUAL TABLE rtree_observations_geom USING rtree(id, minx, maxx, miny, maxy);
        CREATE VIRTUAL TABLE rtree_footprints_geom USING rtree(id, minx, maxx, miny, maxy);
        "#,
    )
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    for i in 0..n {
        let o = observation(i);
        let point = geo::Geometry::Point(geo::point!(x: o.lon, y: o.lat));
        sqlx::query("INSERT INTO observations (fid, geom, image_id, acquired, cloud, label) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(o.fid)
            .bind(encode_gpkg(&point, 4326))
            .bind(&o.image_id)
            .bind(&o.acquired)
            .bind(o.cloud)
            .bind(&o.label)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO rtree_observations_geom VALUES (?, ?, ?, ?, ?)")
            .bind(o.fid)
            .bind(o.lon)
            .bind(o.lon)
            .bind(o.lat)
            .bind(o.lat)
            .execute(&mut *tx)
            .await
            .unwrap();
        let d = 0.05;
        let poly = geo::Geometry::Polygon(
            geo::Rect::new((o.lon - d, o.lat - d), (o.lon + d, o.lat + d)).to_polygon(),
        );
        sqlx::query("INSERT INTO footprints (fid, geom, image_id, area_km2) VALUES (?, ?, ?, ?)")
            .bind(o.fid)
            .bind(encode_gpkg(&poly, 4326))
            .bind(&o.image_id)
            .bind(123.4)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO rtree_footprints_geom VALUES (?, ?, ?, ?, ?)")
            .bind(o.fid)
            .bind(o.lon - d)
            .bind(o.lon + d)
            .bind(o.lat - d)
            .bind(o.lat + d)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    pool.close().await;
    path.to_path_buf()
}

// ---------------------------------------------------------------- PostGIS (throwaway test database)

/// Load the synthetic dataset into `{schema}.observations` / `{schema}.footprints` (idempotent)
pub async fn postgis_synthetic(url: &str, schema: &str, n: i64) {
    use sqlx::postgres::PgPoolOptions;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(url)
        .await
        .unwrap();
    let existing: Option<i64> = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM information_schema.tables WHERE table_schema = '{schema}' AND table_name = 'footprints'"
    ))
    .fetch_one(&pool)
    .await
    .ok();
    if existing == Some(1) {
        let rows: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {schema}.footprints"))
            .fetch_one(&pool)
            .await
            .unwrap();
        if rows == n {
            return;
        }
    }
    pool.execute(
        format!(
            r#"
        DROP SCHEMA IF EXISTS {schema} CASCADE;
        CREATE SCHEMA {schema};
        CREATE TABLE {schema}.observations (fid bigint PRIMARY KEY, geom geometry(Point, 4326), image_id text NOT NULL,
          acquired timestamptz, cloud double precision, label text);
        CREATE TABLE {schema}.footprints (fid bigint PRIMARY KEY, geom geometry(Polygon, 4326), image_id text NOT NULL,
          area_km2 double precision);
        "#
        )
        .as_str(),
    )
    .await
    .unwrap();
    let mut obs = String::new();
    let mut fps = String::new();
    for i in 0..n {
        let o = observation(i);
        let label = o.label.clone().unwrap_or_else(|| "\\N".to_string());
        obs.push_str(&format!(
            "{}\tSRID=4326;POINT({} {})\t{}\t{}\t{}\t{}\n",
            o.fid, o.lon, o.lat, o.image_id, o.acquired, o.cloud, label
        ));
        let d = 0.05;
        let (a, b, c, e) = (o.lon - d, o.lat - d, o.lon + d, o.lat + d);
        fps.push_str(&format!(
            "{}\tSRID=4326;POLYGON(({a} {b},{c} {b},{c} {e},{a} {e},{a} {b}))\t{}\t123.4\n",
            o.fid, o.image_id
        ));
    }
    for (table, data) in [("observations", obs), ("footprints", fps)] {
        let mut conn = pool.acquire().await.unwrap();
        let mut copy = conn
            .copy_in_raw(&format!("COPY {schema}.{table} FROM STDIN"))
            .await
            .unwrap();
        copy.send(data.into_bytes()).await.unwrap();
        copy.finish().await.unwrap();
    }
    pool.execute(
        format!(
            r#"
        CREATE INDEX ON {schema}.observations (image_id);
        CREATE INDEX ON {schema}.observations USING gist (geom);
        CREATE INDEX ON {schema}.footprints (image_id);
        CREATE INDEX ON {schema}.footprints USING gist (geom);
        ANALYZE {schema}.observations;
        ANALYZE {schema}.footprints;
        "#
        )
        .as_str(),
    )
    .await
    .unwrap();
    pool.close().await;
}

// ---------------------------------------------------------------- ClickHouse (throwaway test server)

async fn ch_exec(http: &str, user: &str, pw: &str, sql: String) {
    let resp = reqwest::Client::new()
        .post(http)
        .basic_auth(user, Some(pw))
        .body(sql)
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let text = resp.text().await.unwrap();
    assert!(status.is_success(), "{text}");
}

/// Load the synthetic dataset into ClickHouse database `db` (tables observations, footprints,
/// observations_wkb, observations_lonlat). Idempotent.
pub async fn clickhouse_synthetic(http: &str, user: &str, pw: &str, db: &str, n: i64) {
    let count = reqwest::Client::new()
        .post(http)
        .basic_auth(user, Some(pw))
        .body(format!("SELECT count() FROM {db}.observations_lonlat"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap_or_default();
    if count.trim() == n.to_string() {
        return;
    }
    ch_exec(http, user, pw, format!("DROP DATABASE IF EXISTS {db}")).await;
    ch_exec(http, user, pw, format!("CREATE DATABASE {db}")).await;
    ch_exec(http, user, pw, format!("CREATE TABLE {db}.observations (fid UInt64, geom Point, image_id String, acquired DateTime64(3, 'UTC'), cloud Float64, label Nullable(String)) ENGINE = MergeTree ORDER BY image_id")).await;
    ch_exec(http, user, pw, format!("CREATE TABLE {db}.footprints (fid UInt64, geom Polygon, image_id String, area_km2 Float64) ENGINE = MergeTree ORDER BY fid")).await;
    let mut obs = String::new();
    let mut fps = String::new();
    for i in 0..n {
        let o = observation(i);
        let label = o.label.clone().unwrap_or_else(|| "\\N".to_string());
        let acquired = o.acquired.trim_end_matches('Z').replace('T', " ");
        obs.push_str(&format!(
            "{}\t({},{})\t{}\t{}\t{}\t{}\n",
            o.fid, o.lon, o.lat, o.image_id, acquired, o.cloud, label
        ));
        let d = 0.05;
        let (a, b, c, e) = (o.lon - d, o.lat - d, o.lon + d, o.lat + d);
        fps.push_str(&format!(
            "{}\t[[({a},{b}),({c},{b}),({c},{e}),({a},{e}),({a},{b})]]\t{}\t123.4\n",
            o.fid, o.image_id
        ));
    }
    for (table, data) in [("observations", obs), ("footprints", fps)] {
        let resp = reqwest::Client::new()
            .post(format!(
                "{http}?query={}",
                urlencode(&format!("INSERT INTO {db}.{table} FORMAT TabSeparated"))
            ))
            .basic_auth(user, Some(pw))
            .body(data)
            .send()
            .await
            .unwrap();
        let status = resp.status();
        assert!(status.is_success(), "{}", resp.text().await.unwrap());
    }
    ch_exec(http, user, pw, format!("CREATE TABLE {db}.observations_wkb (fid UInt64, geom String, image_id String, cloud Float64) ENGINE = MergeTree ORDER BY image_id AS SELECT fid, wkb(geom), image_id, cloud FROM {db}.observations")).await;
    ch_exec(http, user, pw, format!("CREATE TABLE {db}.observations_lonlat (fid UInt64, lon Float64, lat Float64, image_id String, cloud Float64) ENGINE = MergeTree ORDER BY (lon, lat) AS SELECT fid, tupleElement(geom, 1), tupleElement(geom, 2), image_id, cloud FROM {db}.observations")).await;
}

fn urlencode(s: &str) -> String {
    serde_urlencoded::to_string([("x", s)]).unwrap()[2..].to_string()
}
