//! PostGIS backend: cross-backend equivalence with GeoPackage, SQL pushdown and index usage.
//!
//! Requires a throwaway PostGIS database (it creates schemas `synth` and `synth_big`):
//!   docker run -d --name bbox-wfs-postgis -e POSTGRES_PASSWORD=wfstest -e POSTGRES_USER=wfstest \
//!     -e POSTGRES_DB=wfstest -p 127.0.0.1:55432:5432 imresamu/postgis:17-3.5
//!   BBOX_WFS_TEST_PG=postgresql://wfstest:wfstest@127.0.0.1:55432/wfstest cargo test --test wfs_postgis -- --ignored

mod common;
use bbox_feature_server::wfs::filter::{parse_filter_str, prepare, FilterVersion, ParseContext};
use bbox_feature_server::wfs::store::sql::SqlValue;
use bbox_feature_server::wfs::store::StoreQuery;
use common::fixtures::observation;
use common::*;

fn enc(s: &str) -> String {
    serde_urlencoded::to_string([("x", s)]).unwrap()[2..].to_string()
}

fn fes(body: &str) -> String {
    format!(
        r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:app="http://example.com/app">{body}</fes:Filter>"#
    )
}

fn filters() -> Vec<String> {
    let id = |i: i64| observation(i).image_id;
    let vr = |p: &str| format!("<fes:ValueReference>app:{p}</fes:ValueReference>");
    let lit = |v: &str| format!("<fes:Literal>{v}</fes:Literal>");
    vec![
        format!("<fes:PropertyIsEqualTo>{}{}</fes:PropertyIsEqualTo>", vr("image_id"), lit(&id(42))),
        format!("<fes:Or>{}</fes:Or>", (1..20).map(|i| format!("<fes:PropertyIsEqualTo>{}{}</fes:PropertyIsEqualTo>", vr("image_id"), lit(&id(i)))).collect::<String>()),
        format!("<fes:PropertyIsLessThan>{}{}</fes:PropertyIsLessThan>", vr("cloud"), lit("10")),
        format!("<fes:PropertyIsBetween>{}<fes:LowerBoundary>{}</fes:LowerBoundary><fes:UpperBoundary>{}</fes:UpperBoundary></fes:PropertyIsBetween>", vr("cloud"), lit("10"), lit("19")),
        format!(r#"<fes:PropertyIsLike wildCard="*" singleChar="?" escapeChar="\">{}{}</fes:PropertyIsLike>"#, vr("label"), lit("*s-1")),
        format!(r#"<fes:PropertyIsLike wildCard="*" singleChar="?" escapeChar="\" matchCase="false">{}{}</fes:PropertyIsLike>"#, vr("label"), lit("CLASS-2")),
        format!("<fes:PropertyIsNull>{}</fes:PropertyIsNull>", vr("label")),
        format!("<fes:Not><fes:PropertyIsNull>{}</fes:PropertyIsNull></fes:Not>", vr("label")),
        format!("<fes:PropertyIsGreaterThanOrEqualTo>{}{}</fes:PropertyIsGreaterThanOrEqualTo>", vr("acquired"), lit("2020-01-02T00:00:00Z")),
        format!(r#"<fes:During>{}<gml:TimePeriod gml:id="p"><gml:beginPosition>2020-01-01T10:00:00Z</gml:beginPosition><gml:endPosition>2020-01-01T20:00:00Z</gml:endPosition></gml:TimePeriod></fes:During>"#, vr("acquired")),
        format!(r#"<fes:During>{}<gml:TimePeriod gml:id="p"><gml:beginPosition>2020-03-01T00:00:00Z</gml:beginPosition><gml:endPosition indeterminatePosition="unknown"/></gml:TimePeriod></fes:During>"#, vr("acquired")),
        format!(r#"<fes:After>{}<gml:TimeInstant gml:id="i"><gml:timePosition>2020-01-01T12:00:00+09:00</gml:timePosition></gml:TimeInstant></fes:After>"#, vr("acquired")),
        format!(r#"<fes:BBOX>{}<gml:Envelope srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>10 0</gml:lowerCorner><gml:upperCorner>40 30</gml:upperCorner></gml:Envelope></fes:BBOX>"#, vr("geom")),
        format!(r#"<fes:Intersects>{}<gml:Polygon gml:id="p1" srsName="urn:ogc:def:crs:EPSG::4326"><gml:exterior><gml:LinearRing><gml:posList>10 0 40 0 40 30 10 30 10 0</gml:posList></gml:LinearRing></gml:exterior></gml:Polygon></fes:Intersects>"#, vr("geom")),
        format!(r#"<fes:Within>{}<gml:Polygon gml:id="p1" srsName="urn:ogc:def:crs:EPSG::4326"><gml:exterior><gml:LinearRing><gml:posList>10 0 40 0 40 30 10 30 10 0</gml:posList></gml:LinearRing></gml:exterior></gml:Polygon></fes:Within>"#, vr("geom")),
        format!(r#"<fes:DWithin>{}<gml:Point gml:id="pt" srsName="urn:ogc:def:crs:EPSG::4326"><gml:pos>{} {}</gml:pos></gml:Point><fes:Distance uom="m">50000</fes:Distance></fes:DWithin>"#, vr("geom"), observation(7).lat, observation(7).lon),
        format!("<fes:And><fes:PropertyIsLessThan>{}{}</fes:PropertyIsLessThan><fes:PropertyIsEqualTo><fes:Function name=\"strToUpperCase\">{}</fes:Function>{}</fes:PropertyIsEqualTo></fes:And>", vr("cloud"), lit("50"), vr("label"), lit("CLASS-3")),
        r#"<fes:ResourceId rid="observations.10"/><fes:ResourceId rid="observations.11"/>"#.to_string(),
    ]
}

async fn ids(srv: &TestServer, filter_body: &str, extra: &str) -> (String, Vec<String>) {
    let resp = srv
        .get(&format!(
            "/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&count=5000{extra}&filter={}",
            enc(&fes(filter_body))
        ))
        .await;
    resp.assert_ok();
    let xml = resp.xml();
    let mut ids = xml.strings("//wfs2:member/*/@gml32:id");
    if !extra.contains("sortBy") {
        ids.sort();
    }
    (xml.string("/wfs2:FeatureCollection/@numberMatched"), ids)
}

#[actix_web::test]
#[ignore]
async fn cross_backend_equivalence() {
    let Some(url) = pg_url() else { return };
    let gpkg = synth_server().await;
    let pg = pg_synth_server(&url, "synth", SYNTH_ROWS).await;
    for body in filters() {
        let a = ids(&gpkg, &body, "").await;
        let b = ids(&pg, &body, "").await;
        assert_eq!(a, b, "filter {body}");
    }
    // sorting and paging
    for extra in [
        "&sortBy=app:cloud+DESC,app:acquired+ASC&count=25",
        "&sortBy=app:acquired+DESC&count=10&startIndex=100",
    ] {
        let a = ids(&gpkg, "<fes:PropertyIsLessThan><fes:ValueReference>app:cloud</fes:ValueReference><fes:Literal>50</fes:Literal></fes:PropertyIsLessThan>", extra).await;
        let b = ids(&pg, "<fes:PropertyIsLessThan><fes:ValueReference>app:cloud</fes:ValueReference><fes:Literal>50</fes:Literal></fes:PropertyIsLessThan>", extra).await;
        assert_eq!(a, b, "{extra}");
    }
    // joins, CQL, formats
    let cql = format!(
        "image_id IN ('{}', '{}') OR cloud > 98",
        observation(3).image_id,
        observation(4).image_id
    );
    let q = format!("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&count=5000&cql_filter={}", enc(&cql));
    let (a, b) = (gpkg.get(&q).await.xml(), pg.get(&q).await.xml());
    assert_eq!(
        a.string("/wfs2:FeatureCollection/@numberMatched"),
        b.string("/wfs2:FeatureCollection/@numberMatched")
    );
    let body = r#"<wfs:GetFeature version="2.0.0" service="WFS" xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:app="http://example.com/app"><wfs:Query typeNames="app:footprints app:observations" aliases="a b"><fes:Filter><fes:And><fes:PropertyIsEqualTo><fes:ValueReference>a/app:image_id</fes:ValueReference><fes:ValueReference>b/app:image_id</fes:ValueReference></fes:PropertyIsEqualTo><fes:PropertyIsLessThan><fes:ValueReference>b/app:cloud</fes:ValueReference><fes:Literal>2</fes:Literal></fes:PropertyIsLessThan></fes:And></fes:Filter></wfs:Query></wfs:GetFeature>"#;
    let (a, b) = (
        gpkg.post_wfs(body).await.xml(),
        pg.post_wfs(body).await.xml(),
    );
    assert_eq!(a.count("//wfs2:Tuple"), b.count("//wfs2:Tuple"));
    let q = "/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&count=5&outputFormat=application/json";
    let (a, b): (serde_json::Value, serde_json::Value) = (
        serde_json::from_str(&gpkg.get(q).await.body).unwrap(),
        serde_json::from_str(&pg.get(q).await.body).unwrap(),
    );
    assert_eq!(a["features"][0]["geometry"], b["features"][0]["geometry"]);
    assert_eq!(
        a["features"][0]["properties"]["image_id"],
        b["features"][0]["properties"]["image_id"]
    );
    // DescribeFeatureType types
    let dft = pg
        .get(
            "/wfs?service=WFS&version=2.0.0&request=DescribeFeatureType&typeNames=app:observations",
        )
        .await
        .xml();
    dft.assert("//xs:element[@name='image_id' and @type='xs:string' and @minOccurs='1']");
    dft.assert("//xs:element[@name='acquired' and @type='xs:dateTime']");
    dft.assert("//xs:element[@name='geom' and @type='gml:PointPropertyType']");
}

async fn explain(url: &str, sql: &str, binds: &[SqlValue]) -> String {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(url)
        .await
        .unwrap();
    let mut q = sqlx::query_scalar::<_, serde_json::Value>(Box::leak(
        format!("EXPLAIN (FORMAT JSON) {sql}").into_boxed_str(),
    ));
    for b in binds {
        q = match b {
            SqlValue::Text(t) => q.bind(t.clone()),
            SqlValue::Int(i) => q.bind(*i),
            SqlValue::Float(f) => q.bind(*f),
            SqlValue::Bool(b) => q.bind(*b),
            SqlValue::Bytes(v) => q.bind(v.clone()),
            SqlValue::TextArray(a) => q.bind(a.clone()),
        };
    }
    q.fetch_one(&pool).await.unwrap().to_string()
}

#[actix_web::test]
#[ignore]
async fn index_usage_on_large_table() {
    let Some(url) = pg_url() else { return };
    let n = 1_000_000;
    let srv = pg_synth_server(&url, "synth_big", n).await;
    let wfs = srv.service.wfs.as_ref().unwrap();
    let entry = wfs
        .types
        .iter()
        .find(|t| t.def.name.local == "observations")
        .unwrap();
    let mut ctx = ParseContext::new(FilterVersion::V200);
    ctx.namespaces
        .insert("app".into(), "http://example.com/app".into());
    let id = |i: i64| observation(i).image_id;
    let cases = vec![
        (format!("<fes:PropertyIsEqualTo><fes:ValueReference>app:image_id</fes:ValueReference><fes:Literal>{}</fes:Literal></fes:PropertyIsEqualTo>", id(424_242)), "observations_image_id_idx"),
        (format!("<fes:Or>{}</fes:Or>", (1..50).map(|i| format!("<fes:PropertyIsEqualTo><fes:ValueReference>app:image_id</fes:ValueReference><fes:Literal>{}</fes:Literal></fes:PropertyIsEqualTo>", id(i * 1000))).collect::<String>()), "observations_image_id_idx"),
        (format!("<fes:And><fes:PropertyIsEqualTo><fes:ValueReference>app:image_id</fes:ValueReference><fes:Literal>{}</fes:Literal></fes:PropertyIsEqualTo><fes:PropertyIsEqualTo><fes:Function name=\"strToUpperCase\"><fes:ValueReference>app:label</fes:ValueReference></fes:Function><fes:Literal>CLASS-1</fes:Literal></fes:PropertyIsEqualTo></fes:And>", id(7)), "observations_image_id_idx"),
        (r#"<fes:BBOX><fes:ValueReference>app:geom</fes:ValueReference><gml:Envelope srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>10 0</gml:lowerCorner><gml:upperCorner>10.5 0.5</gml:upperCorner></gml:Envelope></fes:BBOX>"#.to_string(), "observations_geom_idx"),
        (r#"<fes:ResourceId rid="observations.500000"/>"#.to_string(), "observations_pkey"),
    ];
    for (body, index) in cases {
        let f = parse_filter_str(&fes(&body), &ctx).unwrap();
        let f = prepare(&f, &[&entry.def]).unwrap();
        let plan = entry.store.plan(&StoreQuery {
            filter: Some(f),
            limit: Some(1000),
            ..Default::default()
        });
        let json = explain(&url, &plan.sql, &plan.binds).await;
        assert!(
            json.contains(index),
            "index {index} not used for {body}\nSQL: {}\nPLAN: {json}",
            plan.sql
        );
        assert!(
            !json.contains("\"Seq Scan\""),
            "sequential scan for {body}\nPLAN: {json}"
        );
    }
    // end-to-end lookup latency
    let q = format!(
        "/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&filter={}",
        enc(&fes(&format!("<fes:PropertyIsEqualTo><fes:ValueReference>app:image_id</fes:ValueReference><fes:Literal>{}</fes:Literal></fes:PropertyIsEqualTo>", id(999_999))))
    );
    let _ = srv.get(&q).await;
    let t = std::time::Instant::now();
    for _ in 0..20 {
        let xml = srv.get(&q).await.xml();
        xml.assert("/wfs2:FeatureCollection[@numberMatched='1']");
    }
    let per_request = t.elapsed() / 20;
    eprintln!("image_id lookup on {n} rows: {per_request:?} per request");
    assert!(per_request.as_millis() < 200, "{per_request:?}");
}

#[tokio::test]
#[ignore]
async fn decode_postgis_curves_and_z() {
    // read-only: geometries are computed in SELECT expressions
    let Some(url) = pg_url() else { return };
    use bbox_feature_server::wfs::wkb::decode_wkb_z;
    use geo::CoordsIter;
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    for (wkt, n_z) in [
        ("POINT Z (1 2 3)", Some(1)),
        ("LINESTRING Z (0 0 1, 1 1 2)", Some(2)),
        ("POLYGON Z ((0 0 1, 1 0 2, 1 1 3, 0 0 1))", Some(4)),
        ("CIRCULARSTRING (0 0, 1 1, 2 0)", None),
        ("COMPOUNDCURVE (CIRCULARSTRING (0 0, 1 1, 2 0), (2 0, 3 0))", None),
        ("CURVEPOLYGON (COMPOUNDCURVE (CIRCULARSTRING (0 0, 1 1, 2 0), (2 0, 0 0)))", None),
        ("MULTICURVE ((0 0, 1 1), CIRCULARSTRING (0 0, 1 1, 2 0))", None),
        ("MULTISURFACE (CURVEPOLYGON (CIRCULARSTRING (0 0, 1 1, 2 0, 1 -1, 0 0)), ((5 5, 6 5, 6 6, 5 5)))", None),
        ("CIRCULARSTRING Z (0 0 1, 1 1 2, 2 0 3)", Some(0)),
        ("TIN Z (((0 0 0, 0 1 0, 1 1 0, 0 0 0)))", Some(4)),
        ("POLYHEDRALSURFACE (((0 0, 0 1, 1 1, 0 0)))", None),
    ] {
        let wkb: Vec<u8> = sqlx::query_scalar("SELECT ST_AsBinary(ST_GeomFromText($1))")
            .bind(wkt)
            .fetch_one(&pool)
            .await
            .unwrap();
        let (g, z) = decode_wkb_z(&wkb).unwrap_or_else(|e| panic!("{wkt}: {e}"));
        let n = g.coords_count();
        assert!(n > 0, "{wkt}");
        match n_z {
            None => assert!(z.is_none(), "{wkt}"),
            // Some(0): linearized, one Z per output coordinate
            Some(0) => assert_eq!(z.unwrap().len(), n, "{wkt}"),
            Some(k) => assert_eq!(z.unwrap().len(), k, "{wkt}"),
        }
    }
}

#[actix_web::test]
#[ignore]
async fn postgis_3d_and_curves() {
    // creates schema `z3d` in the throwaway test database
    let Some(url) = pg_url() else { return };
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    for sql in [
        "DROP SCHEMA IF EXISTS z3d CASCADE",
        "CREATE SCHEMA z3d",
        "CREATE TABLE z3d.tracks (fid serial PRIMARY KEY, name text, geom geometry(LineStringZ, 4326))",
        "INSERT INTO z3d.tracks (name, geom) VALUES ('t1', ST_GeomFromText('LINESTRING Z (9.799 46.074 600.2, 10.466 46.652 781.4)', 4326))",
        "CREATE TABLE z3d.arcs (fid serial PRIMARY KEY, geom geometry(CompoundCurve, 4326))",
        "INSERT INTO z3d.arcs (geom) VALUES (ST_GeomFromText('COMPOUNDCURVE (CIRCULARSTRING (0 0, 1 1, 2 0), (2 0, 3 0))', 4326))",
    ] {
        sqlx::query(sql).execute(&pool).await.unwrap();
    }
    let cfg = format!(
        r#"
[webserver]
public_server_url = "{BASE_URL}"

[[datasource]]
name = "pg"
[datasource.postgis]
url = "{url}"

[[collection]]
name = "tracks"
[collection.postgis]
datasource = "pg"
table_schema = "z3d"
table_name = "tracks"

[[collection]]
name = "arcs"
[collection.postgis]
datasource = "pg"
table_schema = "z3d"
table_name = "arcs"

[wfs]
namespace_prefix = "app"
namespace_uri = "http://example.com/app"
"#
    );
    let srv = TestServer::from_toml(&cfg).await;
    let xml = srv
        .get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:tracks")
        .await
        .xml();
    xml.assert("//app:tracks/app:geom/gml32:LineString[@srsDimension='3']");
    assert_eq!(
        xml.string("//app:tracks/app:geom/gml32:LineString/gml32:posList"),
        "46.074 9.799 600.2 46.652 10.466 781.4"
    );
    let xml = srv
        .get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:arcs")
        .await
        .xml();
    xml.assert("//app:arcs/app:geom/gml32:LineString");
    assert!(
        xml.string("//app:arcs/app:geom/gml32:LineString/gml32:posList")
            .split_whitespace()
            .count()
            > 20
    );
    sqlx::query("DROP SCHEMA z3d CASCADE")
        .execute(&pool)
        .await
        .unwrap();
}
