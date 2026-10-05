//! GeoServer WFS test datasets (GeoServer `.properties` files, downloaded by
//! tests/fetch-geoserver-data.sh) loaded into GeoPackages, one per namespace.

use super::*;
use bbox_feature_server::wfs::cql::parse_wkt;
use bbox_feature_server::wfs::wkb::encode_gpkg;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Executor, SqlitePool};
use std::path::Path;

/// GeoServer test data directory
pub fn data_dir() -> PathBuf {
    let dir = std::env::var("BBOX_GEOSERVER_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/geoserver-data")
        });
    assert!(
        dir.join("main/BasicPolygons.properties").exists(),
        "GeoServer test data missing in {}: run tests/fetch-geoserver-data.sh",
        dir.display()
    );
    dir
}

/// Namespace with its feature types (property file base names) and default SRS
pub struct GsNamespace {
    pub prefix: &'static str,
    pub uri: &'static str,
    pub srid: u16,
    pub types: &'static [&'static str],
}

/// Default GeoServer WFS test catalog (CiteTestData.TYPENAMES)
pub const NAMESPACES: &[GsNamespace] = &[
    GsNamespace {
        prefix: "cite",
        uri: "http://www.opengis.net/cite",
        srid: 4326,
        types: &[
            "BasicPolygons",
            "Bridges",
            "Buildings",
            "DividedRoutes",
            "Forests",
            "Lakes",
            "MapNeatline",
            "NamedPlaces",
            "Ponds",
            "RoadSegments",
            "Streams",
            "Geometryless",
        ],
    },
    GsNamespace {
        prefix: "cdf",
        uri: "http://www.opengis.net/cite/data",
        srid: 32615,
        types: &[
            "Deletes", "Fifteen", "Inserts", "Locks", "Nulls", "Other", "Seven", "Updates",
        ],
    },
    GsNamespace {
        prefix: "cgf",
        uri: "http://www.opengis.net/cite/geometry",
        srid: 32615,
        types: &[
            "Lines",
            "MLines",
            "MPoints",
            "MPolygons",
            "Points",
            "Polygons",
        ],
    },
    GsNamespace {
        prefix: "sf",
        uri: "http://cite.opengeospatial.org/gmlsf",
        srid: 4326,
        types: &[
            "PrimitiveGeoFeature",
            "AggregateGeoFeature",
            "GenericEntity",
        ],
    },
];

/// Attribute of a property file schema line
#[derive(Debug)]
struct Attr {
    name: String,
    sql_type: &'static str,
    geometry: Option<(String, Option<u16>)>,
}

fn parse_schema(line: &str) -> Vec<Attr> {
    line.split(',')
        .map(|spec| {
            let mut parts = spec.trim().trim_start_matches('*').split(':');
            let name = parts.next().unwrap_or("").to_string();
            let ty = parts.next().unwrap_or("String").to_string();
            let srid = parts
                .find_map(|p| p.strip_prefix("srid="))
                .and_then(|s| s.parse().ok());
            let short = ty.rsplit('.').next().unwrap_or(&ty).to_string();
            let geom = [
                "Point",
                "LineString",
                "Polygon",
                "MultiPoint",
                "MultiLineString",
                "MultiPolygon",
                "Geometry",
                "GeometryCollection",
                "LinearRing",
                "MultiSurface",
                "MultiCurve",
            ];
            if geom.contains(&short.as_str()) {
                let sf = match short.as_str() {
                    "LinearRing" => "LINESTRING".to_string(),
                    "MultiSurface" => "MULTIPOLYGON".to_string(),
                    "MultiCurve" => "MULTILINESTRING".to_string(),
                    s => s.to_uppercase(),
                };
                return Attr {
                    name,
                    sql_type: "BLOB",
                    geometry: Some((sf, srid)),
                };
            }
            let sql_type = match short.as_str() {
                "Integer" | "int" | "Long" | "long" | "Short" | "short" | "BigInteger" => "INTEGER",
                "Double" | "double" | "Float" | "float" | "BigDecimal" => "REAL",
                "Boolean" | "boolean" => "BOOLEAN",
                "Timestamp" => "DATETIME",
                "Date" if ty == "java.sql.Date" => "DATE",
                "Date" => "DATETIME",
                "Time" => "TEXT",
                _ => "TEXT",
            };
            Attr {
                name,
                sql_type,
                geometry: None,
            }
        })
        .collect()
}

/// java.sql.Timestamp text (`yyyy-mm-dd hh:mm:ss[.f][offset]`, UTC without offset) as ISO UTC
fn timestamp(v: &str) -> String {
    let t = v.replacen(' ', "T", 1);
    let with_offset = |s: &str| {
        // offsets like -07 or +0200 or +02:00
        for fmt in [
            "%Y-%m-%dT%H:%M:%S%.f%#z",
            "%Y-%m-%dT%H:%M:%S%.f%z",
            "%Y-%m-%dT%H:%M:%S%.f%:z",
        ] {
            if let Ok(d) = chrono::DateTime::parse_from_str(s, fmt) {
                return Some(
                    d.with_timezone(&chrono::Utc)
                        .format("%Y-%m-%dT%H:%M:%SZ")
                        .to_string(),
                );
            }
        }
        None
    };
    if let Some(d) = with_offset(&t) {
        return d;
    }
    match chrono::NaiveDateTime::parse_from_str(&t, "%Y-%m-%dT%H:%M:%S%.f") {
        Ok(d) => d.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        Err(_) if t.len() == 10 => format!("{t}T00:00:00Z"),
        Err(_) => t,
    }
}

fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// Lines of a property file with backslash continuations joined
fn logical_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for l in text.lines() {
        if let Some(stripped) = l.strip_suffix('\\') {
            cur.push_str(stripped);
            continue;
        }
        cur.push_str(l);
        lines.push(std::mem::take(&mut cur));
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Load a property file as table `name`
async fn load_type(pool: &SqlitePool, file: &Path, table: &str, default_srid: u16) {
    let text = std::fs::read_to_string(file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    let lines = logical_lines(&text);
    let schema = lines
        .iter()
        .find_map(|l| l.strip_prefix("_="))
        .unwrap_or_else(|| panic!("no schema line in {}", file.display()));
    let attrs = parse_schema(schema);
    let mut cols = vec![
        "ogc_fid INTEGER PRIMARY KEY AUTOINCREMENT".to_string(),
        "gml_id TEXT".to_string(),
    ];
    cols.extend(
        attrs
            .iter()
            .map(|a| format!("{} {}", quote(&a.name), a.sql_type)),
    );
    pool.execute(format!("CREATE TABLE {} ({})", quote(table), cols.join(", ")).as_str())
        .await
        .unwrap();
    let table_srid = attrs
        .iter()
        .find_map(|a| a.geometry.as_ref().and_then(|g| g.1))
        .unwrap_or(default_srid);
    for srid in [table_srid].into_iter().chain(
        attrs
            .iter()
            .filter_map(|a| a.geometry.as_ref().and_then(|g| g.1)),
    ) {
        sqlx::query("INSERT OR IGNORE INTO gpkg_spatial_ref_sys VALUES (?, ?, 'EPSG', ?, 'undefined', NULL)")
            .bind(format!("EPSG:{srid}"))
            .bind(srid as i64)
            .bind(srid as i64)
            .execute(pool)
            .await
            .unwrap();
    }
    let has_geom = attrs.iter().any(|a| a.geometry.is_some());
    sqlx::query(
        "INSERT INTO gpkg_contents (table_name, data_type, identifier, srs_id) VALUES (?, ?, ?, ?)",
    )
    .bind(table)
    .bind(if has_geom { "features" } else { "attributes" })
    .bind(table)
    .bind(table_srid as i64)
    .execute(pool)
    .await
    .unwrap();
    for a in &attrs {
        if let Some((gt, srid)) = &a.geometry {
            sqlx::query("INSERT INTO gpkg_geometry_columns VALUES (?, ?, ?, ?, 0, 0)")
                .bind(table)
                .bind(&a.name)
                .bind(gt)
                .bind(srid.unwrap_or(table_srid) as i64)
                .execute(pool)
                .await
                .unwrap();
        }
    }
    let placeholders = vec!["?"; attrs.len() + 1].join(", ");
    let insert = format!(
        "INSERT INTO {} (gml_id, {}) VALUES ({placeholders})",
        quote(table),
        attrs
            .iter()
            .map(|a| quote(&a.name))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut extent: Option<geo::Rect<f64>> = None;
    for line in &lines {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') || line.starts_with("_=") {
            continue;
        }
        let Some((id, values)) = line.split_once('=') else {
            continue;
        };
        let values: Vec<&str> = values.split('|').collect();
        let mut q = sqlx::query(&insert).bind(id.trim().to_string());
        for (i, a) in attrs.iter().enumerate() {
            let v = values.get(i).map(|v| v.trim()).unwrap_or("");
            // GeoServer keeps empty strings, other empty values are null
            let null =
                v == "<null>" || (v.is_empty() && (a.geometry.is_some() || a.sql_type != "TEXT"));
            q = match (&a.geometry, a.sql_type) {
                _ if null => q.bind(None::<String>),
                (Some((_, srid)), _) => {
                    let g = parse_wkt(v).unwrap_or_else(|e| panic!("{table} {id}: {e}"));
                    use geo::BoundingRect;
                    if srid.unwrap_or(table_srid) == table_srid {
                        if let Some(r) = g.bounding_rect() {
                            extent = Some(match extent {
                                None => r,
                                Some(e) => geo::Rect::new(
                                    geo::coord! { x: e.min().x.min(r.min().x), y: e.min().y.min(r.min().y) },
                                    geo::coord! { x: e.max().x.max(r.max().x), y: e.max().y.max(r.max().y) },
                                ),
                            });
                        }
                    }
                    q.bind(encode_gpkg(&g, srid.unwrap_or(table_srid) as i32))
                }
                (None, "INTEGER") => match v.parse::<i64>() {
                    Ok(n) => q.bind(n),
                    Err(_) => q.bind(v.to_string()),
                },
                (None, "REAL") => match v.parse::<f64>() {
                    Ok(n) => q.bind(n),
                    Err(_) => q.bind(v.to_string()),
                },
                (None, "BOOLEAN") => q.bind(matches!(v, "true" | "TRUE" | "True" | "1")),
                (None, "DATETIME") => q.bind(timestamp(v)),
                (None, _) => q.bind(v.to_string()),
            };
        }
        q.execute(pool)
            .await
            .unwrap_or_else(|e| panic!("{table} {id}: {e}"));
    }
    if let Some(r) = extent {
        sqlx::query(
            "UPDATE gpkg_contents SET min_x=?, min_y=?, max_x=?, max_y=? WHERE table_name=?",
        )
        .bind(r.min().x)
        .bind(r.min().y)
        .bind(r.max().x)
        .bind(r.max().y)
        .bind(table)
        .execute(pool)
        .await
        .unwrap();
    }
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

/// Build one GeoPackage per namespace below `dir`; returns the namespace directories
pub async fn build(dir: &Path) -> Vec<PathBuf> {
    let data = data_dir().join("main");
    let mut dirs = Vec::new();
    for ns in NAMESPACES {
        let ns_dir = dir.join(ns.prefix);
        std::fs::create_dir_all(&ns_dir).unwrap();
        let pool = create_gpkg(&ns_dir.join(format!("{}.gpkg", ns.prefix))).await;
        for t in ns.types {
            load_type(&pool, &data.join(format!("{t}.properties")), t, ns.srid).await;
        }
        pool.close().await;
        dirs.push(ns_dir);
    }
    dirs
}

/// Server with the default GeoServer WFS test catalog (cite, cdf, cgf, sf)
pub async fn server() -> TestServer {
    server_with("").await
}

/// Server with the default GeoServer WFS test catalog and additional `[wfs]` settings
pub async fn server_with(wfs_cfg: &str) -> TestServer {
    static DIRS: tokio::sync::OnceCell<Vec<PathBuf>> = tokio::sync::OnceCell::const_new();
    let dirs = DIRS
        .get_or_init(|| async { build(&tmp_dir().join("geoserver")).await })
        .await;
    let mut cfg = format!("[webserver]\npublic_server_url = \"{BASE_URL}\"\n");
    for d in dirs {
        cfg.push_str(&format!(
            "\n[[collections.directory]]\ndir = \"{}\"\n",
            d.display()
        ));
    }
    cfg.push_str("\n[wfs]\ntitle = \"GeoServer test catalog\"\nnamespace_prefix = \"gs\"\nnamespace_uri = \"http://geoserver.org\"\nother_crs = [4326, 3857, 32615, 4269]\n");
    cfg.push_str(wfs_cfg);
    cfg.push('\n');
    for ns in NAMESPACES {
        let collections: Vec<String> = ns.types.iter().map(|t| format!("\"{t}\"")).collect();
        cfg.push_str(&format!(
            "\n[[wfs.namespace]]\nprefix = \"{}\"\nuri = \"{}\"\ncollections = [{}]\n",
            ns.prefix,
            ns.uri,
            collections.join(", ")
        ));
    }
    TestServer::from_toml(&cfg).await
}
