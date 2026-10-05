//! Response cache, count cache, ETag / If-None-Match and Cache-Control.

mod common;
use common::*;
use std::path::PathBuf;

/// Server on a private copy of the synthetic dataset (n rows) with `[wfs.cache]` settings
async fn cached_server(name: &str, n: i64, cache_cfg: &str) -> (TestServer, PathBuf) {
    let dir = tmp_dir().join(format!("cache-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let gpkg = dir.join("synth.gpkg");
    fixtures::synthetic_gpkg(&gpkg, n).await;
    let cfg = format!(
        r#"
[webserver]
public_server_url = "{BASE_URL}"

[[collections.directory]]
dir = "{}"

[wfs]
namespace_prefix = "app"
namespace_uri = "http://example.com/app"

[wfs.cache]
{cache_cfg}
"#,
        dir.display()
    );
    (TestServer::from_toml(&cfg).await, gpkg)
}

/// Delete features from the private fixture copy
async fn delete_rows(gpkg: &std::path::Path, where_clause: &str) {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", gpkg.display()))
        .await
        .unwrap();
    sqlx::query(&format!("DELETE FROM observations WHERE {where_clause}"))
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}

const GF: &str =
    "/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&count=5";
const GF_REORDERED: &str =
    "/wfs?COUNT=5&typenames=app:observations&REQUEST=GetFeature&version=2.0.0&service=WFS";

fn numbers(r: &Response) -> (String, String) {
    let xml = r.xml();
    (
        xml.string("/wfs2:FeatureCollection/@numberMatched"),
        xml.string("/wfs2:FeatureCollection/@numberReturned"),
    )
}

#[actix_web::test]
async fn etag_and_not_modified() {
    let (srv, _) = cached_server("etag", 20, "max_age = 300").await;
    for path in [
        "/wfs?service=WFS&version=2.0.0&request=GetCapabilities",
        "/wfs?service=WFS&version=2.0.0&request=DescribeFeatureType&typeNames=app:observations",
        "/wfs?service=WFS&version=2.0.0&request=ListStoredQueries",
    ] {
        let r = srv.get(path).await;
        r.assert_ok();
        let etag = r
            .header("etag")
            .unwrap_or_else(|| panic!("ETag for {path}"))
            .to_string();
        assert!(etag.starts_with('"') && etag.ends_with('"'), "{etag}");
        assert_eq!(r.header("cache-control"), Some("max-age=300"));
        let again = srv.get(path).await;
        assert_eq!(
            again.header("etag"),
            Some(etag.as_str()),
            "stable ETag for {path}"
        );
        let r304 = srv
            .get_with_headers(path, &[("If-None-Match", &etag)])
            .await;
        assert_eq!(r304.status, 304, "{path}");
        assert!(r304.bytes.is_empty());
        assert_eq!(r304.header("etag"), Some(etag.as_str()));
        let any = srv
            .get_with_headers(path, &[("If-None-Match", "\"other\", *")])
            .await;
        assert_eq!(any.status, 304);
        let other = srv
            .get_with_headers(path, &[("If-None-Match", "\"other\"")])
            .await;
        assert_eq!(other.status, 200);
    }
    // features are not cacheable without a response cache: no ETag
    let r = srv.get(GF).await;
    r.assert_ok();
    assert_eq!(r.header("etag"), None);
}

#[actix_web::test]
async fn no_cache_headers_by_default() {
    let (srv, _) = cached_server("default", 20, "").await;
    let r = srv
        .get("/wfs?service=WFS&version=2.0.0&request=GetCapabilities")
        .await;
    assert!(r.header("etag").is_some());
    assert_eq!(r.header("cache-control"), None);
    assert_eq!(r.header("x-bbox-cache"), None);
}

#[actix_web::test]
async fn response_cache_hits_and_expiry() {
    let (srv, gpkg) = cached_server("ttl", 30, "response_ttl = 2\nmax_age = 2").await;
    let first = srv.get(GF).await;
    first.assert_ok();
    assert_eq!(first.header("x-bbox-cache"), Some("miss"));
    assert_eq!(numbers(&first), ("30".to_string(), "5".to_string()));
    let etag = first
        .header("etag")
        .expect("ETag of cached response")
        .to_string();
    assert_eq!(first.header("cache-control"), Some("max-age=2"));
    // data changes are not visible while cached; parameter order/case is canonicalized
    delete_rows(&gpkg, "fid <= 10").await;
    for path in [GF, GF_REORDERED] {
        let r = srv.get(path).await;
        assert_eq!(r.header("x-bbox-cache"), Some("hit"), "{path}");
        assert_eq!(r.bytes, first.bytes);
        assert_eq!(r.header("etag"), Some(etag.as_str()));
    }
    let r304 = srv.get_with_headers(GF, &[("If-None-Match", &etag)]).await;
    assert_eq!(r304.status, 304);
    // a different request is a miss
    let other = srv.get(&GF.replace("count=5", "count=6")).await;
    assert_eq!(other.header("x-bbox-cache"), Some("miss"));
    assert_eq!(numbers(&other).0, "20");
    // expiry
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    let r = srv.get(GF).await;
    assert_eq!(r.header("x-bbox-cache"), Some("miss"));
    assert_eq!(numbers(&r).0, "20");
}

#[actix_web::test]
async fn response_cache_post_and_formats() {
    let (srv, gpkg) = cached_server("post", 30, "response_ttl = 60").await;
    let body = r#"<wfs:GetFeature service="WFS" version="2.0.0" count="3" xmlns:wfs="http://www.opengis.net/wfs/2.0"><wfs:Query typeNames="app:observations" xmlns:app="http://example.com/app"/></wfs:GetFeature>"#;
    let first = srv.post_wfs(body).await;
    first.assert_ok();
    assert_eq!(first.header("x-bbox-cache"), Some("miss"));
    let json_path = format!("{GF}&outputFormat=application/json");
    let json = srv.get(&json_path).await;
    assert_eq!(json.header("x-bbox-cache"), Some("miss"));
    assert!(json.content_type.starts_with("application/json"));
    delete_rows(&gpkg, "fid <= 10").await;
    let again = srv.post_wfs(body).await;
    assert_eq!(again.header("x-bbox-cache"), Some("hit"));
    assert_eq!(again.bytes, first.bytes);
    let json2 = srv.get(&json_path).await;
    assert_eq!(json2.header("x-bbox-cache"), Some("hit"));
    assert_eq!(json2.content_type, json.content_type);
    assert_eq!(json2.bytes, json.bytes);
    // exceptions are never cached
    let bad = srv
        .get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:nope")
        .await;
    assert_eq!(bad.status, 400);
    let bad2 = srv
        .get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:nope")
        .await;
    assert_eq!(bad2.header("x-bbox-cache"), None);
}

#[actix_web::test]
async fn response_size_limit_and_collections() {
    let (srv, _) = cached_server(
        "limits",
        200,
        "response_ttl = 60\nmax_response_bytes = 20000\ncollections = [\"observations\"]",
    )
    .await;
    // large response streamed but not cached
    let big = format!("{}&count=200", GF.replace("&count=5", ""));
    let r = srv.get(&big).await;
    r.assert_ok();
    assert!(r.bytes.len() > 20000);
    assert_eq!(srv.get(&big).await.header("x-bbox-cache"), Some("miss"));
    // small response cached
    srv.get(GF).await;
    assert_eq!(srv.get(GF).await.header("x-bbox-cache"), Some("hit"));
    // collection not configured for caching
    let fp = "/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:footprints&count=2";
    srv.get(fp).await;
    assert_eq!(srv.get(fp).await.header("x-bbox-cache"), None);
}

#[actix_web::test]
async fn stored_query_changes_clear_cache() {
    let (srv, gpkg) = cached_server("sq", 30, "response_ttl = 60").await;
    srv.get(GF).await;
    assert_eq!(srv.get(GF).await.header("x-bbox-cache"), Some("hit"));
    let create = r#"<wfs:CreateStoredQuery service="WFS" version="2.0.0" xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:app="http://example.com/app"><wfs:StoredQueryDefinition id="urn:test:all"><wfs:QueryExpressionText returnFeatureTypes="app:observations" language="urn:ogc:def:queryLanguage:OGC-WFS::WFSQueryExpression" isPrivate="false"><wfs:Query typeNames="app:observations"/></wfs:QueryExpressionText></wfs:StoredQueryDefinition></wfs:CreateStoredQuery>"#;
    srv.post_wfs(create).await.assert_ok();
    delete_rows(&gpkg, "fid <= 10").await;
    let r = srv.get(GF).await;
    assert_eq!(r.header("x-bbox-cache"), Some("miss"));
    assert_eq!(numbers(&r).0, "20");
}

#[actix_web::test]
async fn count_cache() {
    let (srv, gpkg) = cached_server("count", 30, "count_ttl = 60").await;
    let hits = "/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&resultType=hits";
    assert_eq!(
        srv.get(hits)
            .await
            .xml()
            .string("/wfs2:FeatureCollection/@numberMatched"),
        "30"
    );
    delete_rows(&gpkg, "fid <= 10").await;
    // numberMatched from the count cache, features are current
    assert_eq!(
        srv.get(hits)
            .await
            .xml()
            .string("/wfs2:FeatureCollection/@numberMatched"),
        "30"
    );
    let r = srv.get(&format!("{GF}&startIndex=0")).await;
    assert_eq!(numbers(&r).0, "30");
    assert_eq!(r.xml().count("//wfs2:member"), 5);
    // a different filter is counted
    let filtered = format!("{hits}&resourceId=observations.15,observations.3");
    assert_eq!(
        srv.get(&filtered)
            .await
            .xml()
            .string("/wfs2:FeatureCollection/@numberMatched"),
        "1"
    );
}
