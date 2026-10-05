//! ClickHouse backend (native protocol and HTTP): cross-backend equivalence with GeoPackage,
//! geometry layouts, SQL pushdown and sort key pruning.
//!
//! Requires a throwaway ClickHouse server:
//!   docker run -d --name bbox-wfs-clickhouse -p 127.0.0.1:59000:9000 -p 127.0.0.1:58123:8123 \
//!     -e CLICKHOUSE_USER=wfstest -e CLICKHOUSE_PASSWORD=wfstest clickhouse/clickhouse-server
//!   BBOX_WFS_TEST_CH_HTTP=http://127.0.0.1:58123/ cargo test --test wfs_clickhouse -- --ignored

mod common;
use common::fixtures::observation;
use common::*;

const NATIVE: &str = "tcp://wfstest:wfstest@127.0.0.1:59000";
const HTTP: &str = "http://wfstest:wfstest@127.0.0.1:58123";

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
        String::new(),
        format!(
            "<fes:PropertyIsEqualTo>{}{}</fes:PropertyIsEqualTo>",
            vr("image_id"),
            lit(&id(42))
        ),
        format!(
            "<fes:Or>{}</fes:Or>",
            (1..20)
                .map(|i| format!(
                    "<fes:PropertyIsEqualTo>{}{}</fes:PropertyIsEqualTo>",
                    vr("image_id"),
                    lit(&id(i))
                ))
                .collect::<String>()
        ),
        format!(
            "<fes:PropertyIsLessThan>{}{}</fes:PropertyIsLessThan>",
            vr("cloud"),
            lit("10")
        ),
        format!(
            r#"<fes:BBOX>{}<gml:Envelope srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>10 0</gml:lowerCorner><gml:upperCorner>40 30</gml:upperCorner></gml:Envelope></fes:BBOX>"#,
            vr("geom")
        ),
        format!(
            r#"<fes:Intersects>{}<gml:Polygon gml:id="p1" srsName="urn:ogc:def:crs:EPSG::4326"><gml:exterior><gml:LinearRing><gml:posList>10 0 40 0 30 30 10 0</gml:posList></gml:LinearRing></gml:exterior></gml:Polygon></fes:Intersects>"#,
            vr("geom")
        ),
        format!(
            r#"<fes:DWithin>{}<gml:Point gml:id="pt" srsName="urn:ogc:def:crs:EPSG::4326"><gml:pos>{} {}</gml:pos></gml:Point><fes:Distance uom="m">50000</fes:Distance></fes:DWithin>"#,
            vr("geom"),
            observation(7).lat,
            observation(7).lon
        ),
        r#"<fes:ResourceId rid="TYPE.10"/><fes:ResourceId rid="TYPE.11"/>"#.to_string(),
    ]
}

fn full_filters() -> Vec<String> {
    let vr = |p: &str| format!("<fes:ValueReference>app:{p}</fes:ValueReference>");
    let lit = |v: &str| format!("<fes:Literal>{v}</fes:Literal>");
    vec![
        format!(
            r#"<fes:PropertyIsLike wildCard="*" singleChar="?" escapeChar="\">{}{}</fes:PropertyIsLike>"#,
            vr("label"),
            lit("*s-1")
        ),
        format!("<fes:PropertyIsNull>{}</fes:PropertyIsNull>", vr("label")),
        format!(
            "<fes:PropertyIsGreaterThanOrEqualTo>{}{}</fes:PropertyIsGreaterThanOrEqualTo>",
            vr("acquired"),
            lit("2020-01-02T00:00:00Z")
        ),
        format!(
            r#"<fes:During>{}<gml:TimePeriod gml:id="p"><gml:beginPosition>2020-01-01T10:00:00Z</gml:beginPosition><gml:endPosition>2020-01-01T20:00:00Z</gml:endPosition></gml:TimePeriod></fes:During>"#,
            vr("acquired")
        ),
        format!(
            r#"<fes:During>{}<gml:TimePeriod gml:id="p"><gml:beginPosition>2020-03-01T00:00:00Z</gml:beginPosition><gml:endPosition indeterminatePosition="unknown"/></gml:TimePeriod></fes:During>"#,
            vr("acquired")
        ),
    ]
}

async fn ids(srv: &TestServer, type_name: &str, filter_body: &str) -> (String, Vec<String>) {
    let body = filter_body.replace("TYPE", type_name);
    let filter = if body.is_empty() {
        String::new()
    } else {
        format!("&filter={}", enc(&fes(&body)))
    };
    let resp = srv
        .get(&format!("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:{type_name}&count=5000{filter}"))
        .await;
    resp.assert_ok();
    let xml = resp.xml();
    let mut ids: Vec<String> = xml
        .strings("//wfs2:member/*/@gml32:id")
        .into_iter()
        .map(|id| {
            id.rsplit_once('.')
                .map(|(_, k)| k.to_string())
                .unwrap_or(id)
        })
        .collect();
    ids.sort();
    (xml.string("/wfs2:FeatureCollection/@numberMatched"), ids)
}

#[actix_web::test]
#[ignore]
async fn cross_backend_equivalence() {
    if ch_config().is_none() {
        return;
    }
    let gpkg = synth_server().await;
    for base in [NATIVE, HTTP] {
        let ch = ch_synth_server(base, "synth", SYNTH_ROWS).await;
        for body in filters().iter().chain(full_filters().iter()) {
            let expected = ids(&gpkg, "observations", body).await;
            assert_eq!(
                ids(&ch, "observations", body).await,
                expected,
                "{base} observations {body}"
            );
        }
        // other geometry layouts
        for t in ["observations_wkb", "observations_lonlat"] {
            for body in filters() {
                let expected =
                    ids(&gpkg, "observations", &body.replace("TYPE", "observations")).await;
                assert_eq!(ids(&ch, t, &body).await, expected, "{base} {t} {body}");
            }
        }
        // polygons (footprints have no cloud property)
        for body in filters().iter().filter(|f| !f.contains("app:cloud")) {
            assert_eq!(
                ids(&ch, "footprints", body).await,
                ids(&gpkg, "footprints", body).await,
                "{base} footprints {body}"
            );
        }
        // output formats and OGC API Features on the same collection
        let json: serde_json::Value = serde_json::from_str(&ch.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&count=1&outputFormat=application/json&sortBy=app:image_id").await.body).unwrap();
        let g: serde_json::Value = serde_json::from_str(&gpkg.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&count=1&outputFormat=application/json&sortBy=app:image_id").await.body).unwrap();
        assert_eq!(
            json["features"][0]["geometry"],
            g["features"][0]["geometry"]
        );
        let items = ch.get("/collections/observations/items?limit=3").await;
        items.assert_ok();
        let items: serde_json::Value = serde_json::from_str(&items.body).unwrap();
        assert_eq!(items["features"].as_array().unwrap().len(), 3);
        assert_eq!(items["numberMatched"], SYNTH_ROWS);
    }
}

#[actix_web::test]
#[ignore]
async fn pushdown_and_sort_key_pruning() {
    if ch_config().is_none() {
        return;
    }
    use bbox_feature_server::wfs::filter::{
        parse_filter_str, prepare, FilterVersion, ParseContext,
    };
    use bbox_feature_server::wfs::store::StoreQuery;
    let n = 1_000_000;
    let srv = ch_synth_server(NATIVE, "synth_big", n).await;
    let wfs = srv.service.wfs.as_ref().unwrap();
    let mut ctx = ParseContext::new(FilterVersion::V200);
    ctx.namespaces
        .insert("app".into(), "http://example.com/app".into());
    let id = observation(777_777).image_id;
    for (type_name, body, expect_sql, exact) in [
        ("observations", format!("<fes:PropertyIsEqualTo><fes:ValueReference>app:image_id</fes:ValueReference><fes:Literal>{id}</fes:Literal></fes:PropertyIsEqualTo>"), format!("\"image_id\" = '{id}'"), true),
        ("observations", r#"<fes:BBOX><fes:ValueReference>app:geom</fes:ValueReference><gml:Envelope srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>10 0</gml:lowerCorner><gml:upperCorner>10.5 0.5</gml:upperCorner></gml:Envelope></fes:BBOX>"#.to_string(), "tupleElement(\"geom\",1) BETWEEN".to_string(), true),
        ("observations_lonlat", r#"<fes:BBOX><fes:ValueReference>app:geom</fes:ValueReference><gml:Envelope srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>10 0</gml:lowerCorner><gml:upperCorner>10.5 0.5</gml:upperCorner></gml:Envelope></fes:BBOX>"#.to_string(), "\"lon\" BETWEEN".to_string(), true),
        ("observations", r#"<fes:During><fes:ValueReference>app:acquired</fes:ValueReference><gml:TimePeriod gml:id="p"><gml:beginPosition>2020-01-01T10:00:00Z</gml:beginPosition><gml:endPosition>2020-01-01T20:00:00Z</gml:endPosition></gml:TimePeriod></fes:During>"#.to_string(), "parseDateTime64BestEffort".to_string(), true),
    ] {
        let entry = wfs.types.iter().find(|t| t.def.name.local == type_name).unwrap();
        let f = prepare(&parse_filter_str(&fes(&body), &ctx).unwrap(), &[&entry.def]).unwrap();
        let plan = entry.store.plan(&StoreQuery { filter: Some(f), limit: Some(100), ..Default::default() });
        assert!(plan.sql.contains(&expect_sql), "{}", plan.sql);
        assert_eq!(plan.residual.is_none(), exact, "{body}");
    }
    // sort key pruning: an image_id lookup reads only a few granules
    let q = format!(
        "/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&filter={}",
        enc(&fes(&format!("<fes:PropertyIsEqualTo><fes:ValueReference>app:image_id</fes:ValueReference><fes:Literal>{id}</fes:Literal></fes:PropertyIsEqualTo>")))
    );
    let _ = srv.get(&q).await;
    let t = std::time::Instant::now();
    for _ in 0..20 {
        srv.get(&q)
            .await
            .xml()
            .assert("/wfs2:FeatureCollection[@numberMatched='1']");
    }
    let per = t.elapsed() / 20;
    eprintln!("ClickHouse image_id lookup on {n} rows: {per:?} per request");
    assert!(per.as_millis() < 200, "{per:?}");
}
