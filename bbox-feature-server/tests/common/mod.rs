//! Shared helpers for WFS integration tests.
#![allow(dead_code)]

pub mod fixtures;
pub mod geoserver;
pub mod xml;

use actix_web::dev::{Service, ServiceResponse};
use actix_web::{test, App};
use bbox_core::config::CoreServiceCfg;
use bbox_core::service::{OgcApiService, ServiceEndpoints};
use bbox_feature_server::config::FeatureServiceCfg;
use bbox_feature_server::FeatureService;
use std::path::PathBuf;
use std::sync::OnceLock;
pub use xml::Xml;

pub const BASE_URL: &str = "http://localhost:8080";

/// Per-process temporary directory for fixture files
pub fn tmp_dir() -> PathBuf {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    DIR.get_or_init(|| tempfile::tempdir().unwrap())
        .path()
        .to_path_buf()
}

/// HTTP response captured from the service
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub content_type: String,
    pub body: String,
    pub bytes: Vec<u8>,
    /// Response headers (lowercase names)
    pub headers: Vec<(String, String)>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    pub fn xml(&self) -> Xml {
        Xml::new(self.body.clone())
    }
    #[track_caller]
    pub fn assert_ok(&self) -> &Self {
        assert_eq!(self.status, 200, "unexpected status, body:\n{}", self.body);
        self
    }
    /// Assert an OWS / OGC exception report with the given code
    #[track_caller]
    pub fn assert_exception(&self, code: &str) -> Xml {
        let xml = self.xml();
        let found = xml.string(
            "(//ows:Exception/@exceptionCode | //ows11:Exception/@exceptionCode | //ogc:ServiceException/@code)[1]",
        );
        assert_eq!(found, code, "exception code mismatch:\n{}", self.body);
        xml
    }
}

/// In-process WFS test server
pub struct TestServer {
    pub service: FeatureService,
}

impl TestServer {
    /// Create service from a TOML configuration (feature service and core settings in one document)
    pub async fn from_toml(toml_cfg: &str) -> Self {
        let cfg: FeatureServiceCfg = toml::from_str(toml_cfg).expect("feature service config");
        let core_cfg: CoreServiceCfg = toml::from_str(&core_part(toml_cfg)).expect("core config");
        let service = FeatureService::create(&cfg, &core_cfg).await;
        TestServer { service }
    }

    async fn call(&self, req: test::TestRequest) -> Response {
        let req = req.to_request();
        let app =
            test::init_service(App::new().configure(|cfg| self.service.register_endpoints(cfg)))
                .await;
        let resp: ServiceResponse = app.call(req).await.expect("service call");
        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap_or("").to_string())
            .unwrap_or_default();
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let body = test::read_body(resp).await;
        Response {
            status,
            content_type,
            body: String::from_utf8_lossy(&body).to_string(),
            bytes: body.to_vec(),
            headers,
        }
    }

    /// GET request with raw (not URL encoded) query string, as sent by TEAM Engine
    pub async fn get(&self, path_and_query: &str) -> Response {
        let req = test::TestRequest::get().uri(path_and_query);
        self.call(req).await
    }

    /// GET /wfs with KVP parameters (values are percent-encoded)
    pub async fn kvp(&self, params: &[(&str, &str)]) -> Response {
        let query = serde_urlencoded::to_string(params).unwrap();
        self.get(&format!("/wfs?{query}")).await
    }

    /// GET request with additional headers
    pub async fn get_with_headers(
        &self,
        path_and_query: &str,
        headers: &[(&str, &str)],
    ) -> Response {
        let mut req = test::TestRequest::get().uri(path_and_query);
        for (k, v) in headers {
            req = req.insert_header((*k, *v));
        }
        self.call(req).await
    }

    /// POST XML body
    pub async fn post(&self, path: &str, body: &str) -> Response {
        self.post_with_type(path, body, "application/xml; charset=UTF-8")
            .await
    }

    pub async fn post_with_type(&self, path: &str, body: &str, content_type: &str) -> Response {
        let req = test::TestRequest::post()
            .uri(path)
            .insert_header(("content-type", content_type))
            .set_payload(body.to_string());
        self.call(req).await
    }

    pub async fn post_wfs(&self, body: &str) -> Response {
        self.post("/wfs", body).await
    }
}

/// Extract core settings (webserver) from combined TOML
fn core_part(toml_cfg: &str) -> String {
    let value: toml::Table = toml::from_str(toml_cfg).unwrap();
    let mut core = toml::Table::new();
    if let Some(ws) = value.get("webserver") {
        core.insert("webserver".to_string(), ws.clone());
    }
    toml::to_string(&core).unwrap()
}

/// Server with the CITE WFS 1.0 dataset (cdf, cgf, ccf)
pub async fn cite10_server() -> TestServer {
    static GPKG: tokio::sync::OnceCell<PathBuf> = tokio::sync::OnceCell::const_new();
    let gpkg = GPKG
        .get_or_init(|| async {
            let dir = tmp_dir().join("cite10");
            std::fs::create_dir_all(&dir).unwrap();
            fixtures::cite10_gpkg(&dir).await
        })
        .await;
    let dir = gpkg.parent().unwrap().display().to_string();
    let schemas = fixtures::cite10_schemas();
    let mut cfg = format!(
        r#"
[webserver]
public_server_url = "{BASE_URL}"

[[collections.directory]]
dir = "{dir}"

[wfs]
cite_compliant = true
title = "CITE WFS 1.0 test server"
"#
    );
    for s in schemas {
        cfg.push_str(&format!(
            "\n[[wfs.namespace]]\nprefix = \"{}\"\nuri = \"{}\"\nschema = \"{}\"\n",
            s.prefix,
            s.uri,
            s.xsd.display()
        ));
    }
    TestServer::from_toml(&cfg).await
}

/// Server with the CITE WFS 1.1 dataset (sf)
pub async fn cite11_server() -> TestServer {
    static GPKG: tokio::sync::OnceCell<PathBuf> = tokio::sync::OnceCell::const_new();
    let gpkg = GPKG
        .get_or_init(|| async {
            let dir = tmp_dir().join("cite11");
            std::fs::create_dir_all(&dir).unwrap();
            fixtures::cite11_gpkg(&dir).await
        })
        .await;
    let dir = gpkg.parent().unwrap().display().to_string();
    let s = &fixtures::cite11_schemas()[0];
    let cfg = format!(
        r#"
[webserver]
public_server_url = "{BASE_URL}"

[[collections.directory]]
dir = "{dir}"

[wfs]
cite_compliant = true
title = "CITE WFS 1.1 test server"

[[wfs.namespace]]
prefix = "{}"
uri = "{}"
schema = "{}"
"#,
        s.prefix,
        s.uri,
        s.xsd.display()
    );
    TestServer::from_toml(&cfg).await
}

/// Server with the Natural Earth sample GeoPackage (auto-mapped tables, default namespace)
pub async fn ne_server() -> TestServer {
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../assets");
    let cfg = format!(
        r#"
[webserver]
public_server_url = "{BASE_URL}"

[[datasource]]
name = "ne"
[datasource.gpkg]
path = "{}"

[[collection]]
name = "populated_places"
title = "Populated places"
[collection.gpkg]
datasource = "ne"
table_name = "ne_10m_populated_places"

[[collection]]
name = "lakes"
title = "Lakes"
[collection.gpkg]
datasource = "ne"
table_name = "ne_10m_lakes"

[[collection]]
name = "rivers"
title = "Rivers"
[collection.gpkg]
datasource = "ne"
table_name = "ne_10m_rivers_lake_centerlines"

[wfs]
namespace_prefix = "ne"
namespace_uri = "http://www.naturalearthdata.com"
count_default = 1000
"#,
        assets.join("ne_extracts.gpkg").display()
    );
    TestServer::from_toml(&cfg).await
}

pub const SYNTH_ROWS: i64 = 2000;

/// Server with the synthetic observations/footprints dataset
async fn synth_dir() -> String {
    static GPKG: tokio::sync::OnceCell<PathBuf> = tokio::sync::OnceCell::const_new();
    let gpkg = GPKG
        .get_or_init(|| async {
            let dir = tmp_dir().join("synth");
            std::fs::create_dir_all(&dir).unwrap();
            fixtures::synthetic_gpkg(&dir.join("synth.gpkg"), SYNTH_ROWS).await
        })
        .await;
    gpkg.parent().unwrap().display().to_string()
}

pub async fn synth_server() -> TestServer {
    synth_server_with("").await
}

/// Synthetic dataset server with additional `[wfs]` configuration
pub async fn synth_server_with(wfs_cfg: &str) -> TestServer {
    let dir = synth_dir().await;
    let cfg = format!(
        r#"
[webserver]
public_server_url = "{BASE_URL}"

[[collections.directory]]
dir = "{dir}"

[wfs]
namespace_prefix = "app"
namespace_uri = "http://example.com/app"
count_default = 1000
other_crs = [3857, 32632]
{wfs_cfg}
"#
    );
    TestServer::from_toml(&cfg).await
}

/// Synthetic dataset with the application schema tests/data/synth/observations.xsd
/// (`label` mandatory and nillable)
pub async fn synth_nillable_server() -> TestServer {
    let xsd = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/synth/observations.xsd");
    synth_server_with(&format!(
        "\n[[wfs.namespace]]\nprefix = \"app\"\nuri = \"http://example.com/app\"\nschema = \"{}\"\n",
        xsd.display()
    ))
    .await
}

/// CITE WFS 1.1 server with an additional LinkedFeature `f299` referencing a remote URL
pub async fn cite11_remote_server(href: &str) -> TestServer {
    let dir = tmp_dir().join(format!(
        "cite11-remote-{}",
        href.len() + href.bytes().map(|b| b as usize).sum::<usize>()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cite11.gpkg");
    let data = fixtures::data_dir().join("cite/wfs11");
    let extra = dir.join("remote-insert.xml");
    std::fs::write(
        &extra,
        format!(
            r#"<wfs:Transaction service="WFS" version="1.1.0" xmlns:wfs="http://www.opengis.net/wfs" xmlns:gml="http://www.opengis.net/gml" xmlns:sf="http://cite.opengeospatial.org/gmlsf" xmlns:xlink="http://www.w3.org/1999/xlink"><wfs:Insert srsName="urn:ogc:def:crs:EPSG::4326"><sf:LinkedFeature gml:id="f299"><gml:name codeSpace="http://cite.opengeospatial.org/gmlsf">name-f299</gml:name><sf:reference xlink:type="simple" xlink:href="{href}"/></sf:LinkedFeature></wfs:Insert></wfs:Transaction>"#
        ),
    )
    .unwrap();
    fixtures::build_gpkg(
        &path,
        &fixtures::cite11_schemas(),
        &[
            data.join("dataset-sf0-insert.xml"),
            data.join("dataset-sf1-insert.xml"),
            data.join("dataset-sf2-insert.xml"),
            extra,
        ],
        4326,
    )
    .await;
    let s = &fixtures::cite11_schemas()[0];
    let cfg = format!(
        r#"
[webserver]
public_server_url = "{BASE_URL}"

[[collections.directory]]
dir = "{}"

[[wfs.namespace]]
prefix = "{}"
uri = "{}"
schema = "{}"
"#,
        dir.display(),
        s.prefix,
        s.uri,
        s.xsd.display()
    );
    TestServer::from_toml(&cfg).await
}

/// Throwaway PostGIS test database (see tests/README), None if not configured
pub fn pg_url() -> Option<String> {
    std::env::var("BBOX_WFS_TEST_PG").ok()
}

/// Server with the synthetic dataset in PostGIS schema `schema` (n rows)
pub async fn pg_synth_server(url: &str, schema: &str, n: i64) -> TestServer {
    fixtures::postgis_synthetic(url, schema, n).await;
    let cfg = format!(
        r#"
[webserver]
public_server_url = "{BASE_URL}"

[[datasource]]
name = "pg"
[datasource.postgis]
url = "{url}"

[[collection]]
name = "observations"
[collection.postgis]
datasource = "pg"
table_schema = "{schema}"
table_name = "observations"

[[collection]]
name = "footprints"
[collection.postgis]
datasource = "pg"
table_schema = "{schema}"
table_name = "footprints"

[wfs]
namespace_prefix = "app"
namespace_uri = "http://example.com/app"
count_default = 1000
other_crs = [3857, 32632]
"#
    );
    TestServer::from_toml(&cfg).await
}

/// Throwaway ClickHouse test server: (http url, user, password), None if not configured
pub fn ch_config() -> Option<(String, String, String)> {
    let http = std::env::var("BBOX_WFS_TEST_CH_HTTP").ok()?;
    Some((http, "wfstest".to_string(), "wfstest".to_string()))
}

/// Server with the synthetic dataset in ClickHouse, using `base` (tcp://.. or http://.. without database)
pub async fn ch_synth_server(base: &str, db: &str, n: i64) -> TestServer {
    let url = format!("{base}/{db}");
    let (http, user, pw) = ch_config().expect("ClickHouse configured");
    fixtures::clickhouse_synthetic(&http, &user, &pw, db, n).await;
    let collection = |name: &str, table: &str, extra: &str| {
        format!("\n[[collection]]\nname = \"{name}\"\n[collection.clickhouse]\ndatasource = \"ch\"\ntable_name = \"{table}\"\nfid_field = \"fid\"\n{extra}\n")
    };
    let mut cfg = format!(
        r#"
[webserver]
public_server_url = "{BASE_URL}"

[[datasource]]
name = "ch"
[datasource.clickhouse]
url = "{url}"

[wfs]
namespace_prefix = "app"
namespace_uri = "http://example.com/app"
count_default = 1000
other_crs = [3857, 32632]
"#
    );
    cfg.push_str(&collection("observations", "observations", ""));
    cfg.push_str(&collection("footprints", "footprints", ""));
    cfg.push_str(&collection(
        "observations_wkb",
        "observations_wkb",
        "geometry_field = \"geom\"",
    ));
    cfg.push_str(&collection(
        "observations_lonlat",
        "observations_lonlat",
        "lon_field = \"lon\"\nlat_field = \"lat\"",
    ));
    TestServer::from_toml(&cfg).await
}
