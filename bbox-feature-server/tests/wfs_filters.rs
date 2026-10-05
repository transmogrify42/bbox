//! Filters, sorting, paging and resource ids end-to-end (KVP and XML), WFS 2.0 on the synthetic
//! dataset plus ets-wfs10 comparison operators over HTTP. Assertions follow ets-wfs20 basic.filter.*.

mod common;
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

async fn get_obs(srv: &TestServer, filter_body: &str, extra: &str) -> Xml {
    let resp = srv
        .get(&format!(
            "/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations{}&filter={}{extra}",
            if extra.contains("count=") { "" } else { "&count=5000" },
            enc(&fes(filter_body))
        ))
        .await;
    resp.assert_ok();
    resp.xml()
}

async fn post_obs(srv: &TestServer, filter_body: &str) -> Xml {
    let resp = srv
        .post_wfs(&format!(
            r#"<wfs:GetFeature version="2.0.0" service="WFS" count="5000" xmlns:wfs="http://www.opengis.net/wfs/2.0"><wfs:Query typeNames="app:observations" xmlns:app="http://example.com/app">{}</wfs:Query></wfs:GetFeature>"#,
            fes(filter_body)
        ))
        .await;
    resp.assert_ok();
    resp.xml()
}

fn matched(xml: &Xml) -> usize {
    xml.string("/wfs2:FeatureCollection/@numberMatched")
        .parse()
        .unwrap()
}

fn expected<F: Fn(&common::fixtures::Observation) -> bool>(f: F) -> usize {
    (0..SYNTH_ROWS).map(observation).filter(|o| f(o)).count()
}

/// Check KVP and POST give the same count, which equals the expected count
async fn check(srv: &TestServer, body: &str, expect: usize) {
    let a = get_obs(srv, body, "").await;
    assert_eq!(matched(&a), expect, "GET {body}");
    assert_eq!(
        a.count("//wfs2:member"),
        expect.min(5000),
        "GET members {body}"
    );
    let b = post_obs(srv, body).await;
    assert_eq!(matched(&b), expect, "POST {body}");
}

fn vr(p: &str) -> String {
    format!("<fes:ValueReference>app:{p}</fes:ValueReference>")
}

fn lit(v: &str) -> String {
    format!("<fes:Literal>{v}</fes:Literal>")
}

#[actix_web::test]
async fn comparison_operators() {
    let srv = synth_server().await;
    let id42 = observation(42).image_id;
    check(
        &srv,
        &format!(
            "<fes:PropertyIsEqualTo>{}{}</fes:PropertyIsEqualTo>",
            vr("image_id"),
            lit(&id42)
        ),
        1,
    )
    .await;
    // literal first, matchCase and matchAction attributes
    check(&srv, &format!(r#"<fes:PropertyIsEqualTo matchCase="true" matchAction="Any">{}{}</fes:PropertyIsEqualTo>"#, lit(&id42), vr("image_id")), 1).await;
    check(&srv, &format!(r#"<fes:PropertyIsNotEqualTo matchCase="true" matchAction="All">{}{}</fes:PropertyIsNotEqualTo>"#, lit(&id42), vr("image_id")), SYNTH_ROWS as usize - 1).await;
    let ids: String = [1, 2, 3]
        .iter()
        .map(|i| {
            format!(
                "<fes:PropertyIsEqualTo>{}{}</fes:PropertyIsEqualTo>",
                vr("image_id"),
                lit(&observation(*i).image_id)
            )
        })
        .collect();
    check(&srv, &format!("<fes:Or>{ids}</fes:Or>"), 3).await;
    check(
        &srv,
        &format!(
            "<fes:PropertyIsLessThan>{}{}</fes:PropertyIsLessThan>",
            vr("cloud"),
            lit("10")
        ),
        expected(|o| o.cloud < 10.0),
    )
    .await;
    check(
        &srv,
        &format!(
            "<fes:PropertyIsGreaterThanOrEqualTo>{}{}</fes:PropertyIsGreaterThanOrEqualTo>",
            vr("cloud"),
            lit("95")
        ),
        expected(|o| o.cloud >= 95.0),
    )
    .await;
    check(&srv, &format!("<fes:PropertyIsBetween>{}<fes:LowerBoundary>{}</fes:LowerBoundary><fes:UpperBoundary>{}</fes:UpperBoundary></fes:PropertyIsBetween>", vr("cloud"), lit("10"), lit("19")), expected(|o| (10.0..=19.0).contains(&o.cloud))).await;
    check(
        &srv,
        &format!(
            "<fes:PropertyIsGreaterThanOrEqualTo>{}{}</fes:PropertyIsGreaterThanOrEqualTo>",
            vr("acquired"),
            lit("2020-01-02T00:00:00Z")
        ),
        expected(|o| o.acquired.as_str() >= "2020-01-02T00:00:00Z"),
    )
    .await;
    check(&srv, &format!("<fes:And><fes:PropertyIsLessThan>{}{}</fes:PropertyIsLessThan><fes:Not><fes:PropertyIsNull>{}</fes:PropertyIsNull></fes:Not></fes:And>", vr("cloud"), lit("50"), vr("label")), expected(|o| o.cloud < 50.0 && o.label.is_some())).await;
}

#[actix_web::test]
async fn like_null_nil() {
    let srv = synth_server().await;
    check(&srv, &format!(r#"<fes:PropertyIsLike wildCard="*" singleChar="?" escapeChar="\">{}{}</fes:PropertyIsLike>"#, vr("label"), lit("*s-1")), expected(|o| o.label.as_deref() == Some("class-1"))).await;
    check(&srv, &format!(r#"<fes:PropertyIsLike wildCard="%" singleChar="_" escapeChar="!">{}{}</fes:PropertyIsLike>"#, vr("label"), lit("class-_")), expected(|o| o.label.is_some())).await;
    check(&srv, &format!(r#"<fes:Not><fes:PropertyIsLike wildCard="*" singleChar="?" escapeChar="\">{}{}</fes:PropertyIsLike></fes:Not>"#, vr("label"), lit("*s-1")), expected(|o| o.label.as_deref() != Some("class-1"))).await;
    check(
        &srv,
        &format!("<fes:PropertyIsNull>{}</fes:PropertyIsNull>", vr("label")),
        expected(|o| o.label.is_none()),
    )
    .await;
    // gml:name is not stored: always null (ets gmlNameIsNull)
    let xml = get_obs(&srv, "<fes:PropertyIsNull><fes:ValueReference>gml:name</fes:ValueReference></fes:PropertyIsNull>", "&count=10").await;
    xml.assert_count("//wfs2:member", 10);
    xml.assert_count("//gml32:name", 0);
    check(
        &srv,
        &format!("<fes:PropertyIsNil>{}</fes:PropertyIsNil>", vr("label")),
        0,
    )
    .await;
}

#[actix_web::test]
async fn nillable_schema_property() {
    // `label` is declared minOccurs=1 nillable=true: NULL is encoded as xsi:nil and is nil, not null
    let srv = synth_nillable_server().await;
    let nulls = expected(|o| o.label.is_none());
    assert!(nulls > 0);
    let resp = srv
        .kvp(&[
            ("service", "WFS"),
            ("version", "2.0.0"),
            ("request", "DescribeFeatureType"),
            ("typeNames", "app:observations"),
        ])
        .await;
    resp.assert_ok();
    let xsd = resp.xml();
    xsd.assert(
        "//xs:complexType[@name='observationsType']//xs:element[@name='label'][@nillable='true']",
    );
    xsd.assert("//xs:complexType[@name='observationsType']//xs:element[@name='label'][not(@minOccurs) or @minOccurs='1']");
    check(
        &srv,
        &format!("<fes:PropertyIsNil>{}</fes:PropertyIsNil>", vr("label")),
        nulls,
    )
    .await;
    check(
        &srv,
        &format!(
            "<fes:Not><fes:PropertyIsNil>{}</fes:PropertyIsNil></fes:Not>",
            vr("label")
        ),
        SYNTH_ROWS as usize - nulls,
    )
    .await;
    check(
        &srv,
        &format!("<fes:PropertyIsNull>{}</fes:PropertyIsNull>", vr("label")),
        0,
    )
    .await;
    let xml = get_obs(
        &srv,
        &format!("<fes:PropertyIsNil>{}</fes:PropertyIsNil>", vr("label")),
        "&count=3",
    )
    .await;
    xml.assert_count("//wfs2:member", 3);
    xml.assert_count(
        "//wfs2:member/app:observations/app:label[@xsi:nil='true']",
        3,
    );
    // other types of the namespace are still auto-mapped
    let resp = srv
        .kvp(&[
            ("service", "WFS"),
            ("version", "2.0.0"),
            ("request", "GetFeature"),
            ("typeNames", "app:footprints"),
            ("count", "1"),
        ])
        .await;
    resp.assert_ok();
    resp.xml().assert_count("//wfs2:member/app:footprints", 1);
}

#[actix_web::test]
async fn invalid_operands() {
    let srv = synth_server().await;
    // undefined property
    let resp = srv
        .get(&format!(
            "/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&filter={}",
            enc(r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0"><fes:PropertyIsLessThanOrEqualTo><fes:Literal>1355941270</fes:Literal><fes:ValueReference xmlns:ex="http://example.org">ex:undefined</fes:ValueReference></fes:PropertyIsLessThanOrEqualTo></fes:Filter>"#)
        ))
        .await;
    assert_eq!(resp.status, 400);
    resp.assert_exception("InvalidParameterValue");
    // comparison on gml:boundedBy with an envelope
    let resp = srv
        .get(&format!(
            "/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&filter={}",
            enc(&fes(r#"<fes:PropertyIsLessThanOrEqualTo><fes:ValueReference>gml:boundedBy</fes:ValueReference><fes:Literal><gml:Envelope srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>-90 -180</gml:lowerCorner><gml:upperCorner>90 180</gml:upperCorner></gml:Envelope></fes:Literal></fes:PropertyIsLessThanOrEqualTo>"#))
        ))
        .await;
    assert!([400, 403, 500].contains(&resp.status), "{}", resp.status);
    resp.assert_exception("OperationProcessingFailed");
    // BBOX on a non-geometry property
    let resp = srv
        .get(&format!(
            "/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&filter={}",
            enc(&fes(r#"<fes:BBOX><fes:ValueReference>gml:description</fes:ValueReference><gml:Envelope srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>-90 -180</gml:lowerCorner><gml:upperCorner>90 180</gml:upperCorner></gml:Envelope></fes:BBOX>"#))
        ))
        .await;
    let code = resp.xml().string("//ows11:Exception/@exceptionCode");
    assert!(
        (resp.status == 400 && code == "InvalidParameterValue")
            || (resp.status == 403 && code == "OperationProcessingFailed"),
        "{} {code}",
        resp.status
    );
    // malformed filter
    let resp = srv.get(&format!("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&filter={}", enc("<fes:Filter"))).await;
    assert_eq!(resp.status, 400);
}

#[actix_web::test]
async fn spatial_operators() {
    let srv = synth_server().await;
    let env = |a: f64, b: f64, c: f64, d: f64| {
        format!(
            r#"<gml:Envelope srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>{b} {a}</gml:lowerCorner><gml:upperCorner>{d} {c}</gml:upperCorner></gml:Envelope>"#
        )
    };
    let in_box = |o: &common::fixtures::Observation| {
        o.lon >= 0.0 && o.lon <= 30.0 && o.lat >= 10.0 && o.lat <= 40.0
    };
    check(
        &srv,
        &format!(
            "<fes:BBOX>{}{}</fes:BBOX>",
            vr("geom"),
            env(0.0, 10.0, 30.0, 40.0)
        ),
        expected(in_box),
    )
    .await;
    // without ValueReference
    check(
        &srv,
        &format!("<fes:BBOX>{}</fes:BBOX>", env(0.0, 10.0, 30.0, 40.0)),
        expected(in_box),
    )
    .await;
    // EPSG:4326 short form is lon/lat
    check(&srv, &format!(r#"<fes:BBOX>{}<gml:Envelope srsName="EPSG:4326"><gml:lowerCorner>0 10</gml:lowerCorner><gml:upperCorner>30 40</gml:upperCorner></gml:Envelope></fes:BBOX>"#, vr("geom")), expected(in_box)).await;
    // KVP BBOX parameter (lat/lon for the default CRS urn:ogc:def:crs:EPSG::4326)
    let xml = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&count=5000&bbox=10,0,40,30").await.xml();
    assert_eq!(matched(&xml), expected(in_box));
    let xml = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&count=5000&bbox=0,10,30,40,urn:ogc:def:crs:OGC:1.3:CRS84").await.xml();
    assert_eq!(matched(&xml), expected(in_box));
    // Intersects with polygon, Disjoint, Within
    let poly = r#"<gml:Polygon gml:id="p1" srsName="urn:ogc:def:crs:EPSG::4326"><gml:exterior><gml:LinearRing><gml:posList>10 0 40 0 40 30 10 30 10 0</gml:posList></gml:LinearRing></gml:exterior></gml:Polygon>"#;
    check(
        &srv,
        &format!("<fes:Intersects>{}{poly}</fes:Intersects>", vr("geom")),
        expected(in_box),
    )
    .await;
    check(
        &srv,
        &format!("<fes:Disjoint>{}{poly}</fes:Disjoint>", vr("geom")),
        expected(|o| !in_box(o)),
    )
    .await;
    check(
        &srv,
        &format!("<fes:Within>{}{poly}</fes:Within>", vr("geom")),
        expected(|o| o.lon > 0.0 && o.lon < 30.0 && o.lat > 10.0 && o.lat < 40.0),
    )
    .await;
    // DWithin in metres around observation 7
    let o7 = observation(7);
    let point = format!(
        r#"<gml:Point gml:id="pt" srsName="urn:ogc:def:crs:EPSG::4326"><gml:pos>{} {}</gml:pos></gml:Point>"#,
        o7.lat, o7.lon
    );
    let xml = get_obs(
        &srv,
        &format!(
            r#"<fes:DWithin>{}{point}<fes:Distance uom="m">100</fes:Distance></fes:DWithin>"#,
            vr("geom")
        ),
        "",
    )
    .await;
    assert!(matched(&xml) >= 1);
    xml.assert("//wfs2:member/*[@gml32:id='observations.8']");
    // reprojected literal (UTM 32N)
    let xml = get_obs(&srv, r#"<fes:BBOX><fes:ValueReference>app:geom</fes:ValueReference><gml:Envelope srsName="urn:ogc:def:crs:EPSG::32632"><gml:lowerCorner>200000 4000000</gml:lowerCorner><gml:upperCorner>800000 6000000</gml:upperCorner></gml:Envelope></fes:BBOX>"#, "").await;
    assert!(matched(&xml) > 0);
}

#[actix_web::test]
async fn temporal_operators() {
    let srv = synth_server().await;
    let period = |b: &str, e: &str| {
        format!(
            r#"<gml:TimePeriod gml:id="TP1" frame="http://www.iso.org/iso/iso8601"><gml:beginPosition>{b}</gml:beginPosition><gml:endPosition>{e}</gml:endPosition></gml:TimePeriod>"#
        )
    };
    let instant = |t: &str| {
        format!(
            r#"<gml:TimeInstant gml:id="TI1"><gml:timePosition>{t}</gml:timePosition></gml:TimeInstant>"#
        )
    };
    check(
        &srv,
        &format!(
            "<fes:During>{}{}</fes:During>",
            vr("acquired"),
            period("2020-01-01T10:00:00Z", "2020-01-01T20:00:00Z")
        ),
        9,
    )
    .await;
    check(
        &srv,
        &format!(
            "<fes:After>{}{}</fes:After>",
            vr("acquired"),
            instant("2020-03-01T00:00:00Z")
        ),
        expected(|o| o.acquired.as_str() > "2020-03-01T00:00:00Z"),
    )
    .await;
    check(
        &srv,
        &format!(
            "<fes:After>{}{}</fes:After>",
            vr("acquired"),
            instant("2020-01-01T12:00:00+09:00")
        ),
        expected(|o| o.acquired.as_str() > "2020-01-01T03:00:00Z"),
    )
    .await;
    check(
        &srv,
        &format!(
            "<fes:Before>{}{}</fes:Before>",
            vr("acquired"),
            period("2020-01-01T05:00:00Z", "2020-02-01T00:00:00Z")
        ),
        5,
    )
    .await;
    check(
        &srv,
        &format!(
            "<fes:TEquals>{}{}</fes:TEquals>",
            vr("acquired"),
            instant("2020-01-01T03:00:00Z")
        ),
        1,
    )
    .await;
    check(
        &srv,
        &format!(
            "<fes:AnyInteracts>{}{}</fes:AnyInteracts>",
            vr("acquired"),
            period("2020-01-01T10:00:00Z", "2020-01-01T20:00:00Z")
        ),
        11,
    )
    .await;
    // open periods (gml:indeterminatePosition, DGIWG ETS)
    let open_end = r#"<gml:TimePeriod gml:id="o"><gml:beginPosition>2020-03-01T00:00:00.000</gml:beginPosition><gml:endPosition indeterminatePosition="unknown"/></gml:TimePeriod>"#;
    check(
        &srv,
        &format!("<fes:During>{}{open_end}</fes:During>", vr("acquired")),
        expected(|o| o.acquired.as_str() > "2020-03-01T00:00:00Z"),
    )
    .await;
    let open_begin = r#"<gml:TimePeriod gml:id="o"><gml:beginPosition indeterminatePosition="unknown"/><gml:endPosition>2020-01-01T05:00:00Z</gml:endPosition></gml:TimePeriod>"#;
    check(
        &srv,
        &format!("<fes:During>{}{open_begin}</fes:During>", vr("acquired")),
        5,
    )
    .await;
}

#[actix_web::test]
async fn resource_ids() {
    let srv = synth_server().await;
    let xml = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&resourceId=observations.5,observations.7").await.xml();
    xml.assert_count("//wfs2:member", 2);
    let xml = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&resourceId=test-0000").await.xml();
    xml.assert("/wfs2:FeatureCollection");
    xml.assert_count("//wfs2:member", 0);
    // id of another type (ets inconsistentFeatureIdentifierAndType)
    let resp = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&resourceId=footprints.5").await;
    assert_eq!(resp.status, 400);
    let xml = resp.assert_exception("InvalidParameterValue");
    assert!(xml
        .string("//ows11:Exception/@locator")
        .to_lowercase()
        .contains("resourceid"));
    // fes:ResourceId in POST
    let xml = post_obs(
        &srv,
        r#"<fes:ResourceId rid="observations.10"/><fes:ResourceId rid="observations.11"/>"#,
    )
    .await;
    xml.assert_count("//wfs2:member", 2);
    // version navigation without history
    let xml = post_obs(
        &srv,
        r#"<fes:ResourceId rid="observations.10" version="LAST"/>"#,
    )
    .await;
    xml.assert_count("//wfs2:member", 1);
    let xml = post_obs(
        &srv,
        r#"<fes:ResourceId rid="observations.10" version="NEXT"/>"#,
    )
    .await;
    xml.assert("/wfs2:FeatureCollection[@numberMatched='0']");
}

#[actix_web::test]
async fn sorting() {
    let srv = synth_server().await;
    let xml = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&count=3&sortBy=app:cloud+DESC,acquired+ASC").await.xml();
    let clouds = xml.strings("//*[local-name()='cloud']");
    assert_eq!(clouds, vec!["99", "99", "99"]);
    let times = xml.strings("//*[local-name()='acquired']");
    let mut sorted = times.clone();
    sorted.sort();
    assert_eq!(times, sorted);
    let xml = srv
        .post_wfs(r#"<wfs:GetFeature version="2.0.0" service="WFS" count="2" xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:fes="http://www.opengis.net/fes/2.0"><wfs:Query typeNames="app:observations" xmlns:app="http://example.com/app"><fes:SortBy><fes:SortProperty><fes:ValueReference>app:acquired</fes:ValueReference><fes:SortOrder>DESC</fes:SortOrder></fes:SortProperty></fes:SortBy></wfs:Query></wfs:GetFeature>"#)
        .await
        .xml();
    assert_eq!(
        xml.string("(//*[local-name()='acquired'])[1]"),
        observation(SYNTH_ROWS - 1).acquired
    );
    let resp = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&sortBy=app:unknown").await;
    resp.assert_exception("InvalidParameterValue");
}

fn local_path(url: &str) -> String {
    url.strip_prefix(BASE_URL).unwrap_or(url).to_string()
}

#[actix_web::test]
async fn paging_links() {
    let srv = synth_server().await;
    let xml = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&count=10&startIndex=0").await.xml();
    xml.assert("/wfs2:FeatureCollection[@numberReturned='10']");
    xml.assert_not("/wfs2:FeatureCollection/@previous");
    let next = xml.string("/wfs2:FeatureCollection/@next");
    assert!(next.starts_with(BASE_URL), "{next}");
    let first_ids = xml.strings("//wfs2:member/*/@gml32:id");
    let page2 = srv.get(&local_path(&next)).await.xml();
    page2.assert("/wfs2:FeatureCollection[@numberReturned='10']");
    let prev = page2.string("/wfs2:FeatureCollection/@previous");
    assert!(!prev.is_empty());
    let ids2 = page2.strings("//wfs2:member/*/@gml32:id");
    assert!(ids2.iter().all(|id| !first_ids.contains(id)));
    let back = srv.get(&local_path(&prev)).await.xml();
    assert_eq!(back.strings("//wfs2:member/*/@gml32:id"), first_ids);
    // hits: next pointing to results
    let xml = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&count=1&resultType=hits").await.xml();
    xml.assert("/wfs2:FeatureCollection[@numberReturned='0']");
    xml.assert_not("/wfs2:FeatureCollection/@previous");
    let next = xml.string("/wfs2:FeatureCollection/@next");
    let results = srv.get(&local_path(&next)).await.xml();
    results.assert("/wfs2:FeatureCollection[@numberReturned='1']");
    // POST requests get KVP links too
    let xml = srv
        .post_wfs(r#"<wfs:GetFeature version="2.0.0" service="WFS" count="5" startIndex="5" xmlns:wfs="http://www.opengis.net/wfs/2.0"><wfs:Query typeNames="app:observations" xmlns:app="http://example.com/app"/></wfs:GetFeature>"#)
        .await
        .xml();
    let next = xml.string("/wfs2:FeatureCollection/@next");
    let page = srv.get(&local_path(&next)).await.xml();
    page.assert("//wfs2:member/*[@gml32:id='observations.11']");
    // last page has no next
    let xml = srv.get(&format!("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&count=10&startIndex={}", SYNTH_ROWS - 5)).await.xml();
    xml.assert("/wfs2:FeatureCollection[@numberReturned='5']");
    xml.assert_not("/wfs2:FeatureCollection/@next");
}

// ---------------------------------------------------------------- ets-wfs10 comparison tests over HTTP

#[actix_web::test]
async fn wfs10_comparison_operators_http() {
    let srv = cite10_server().await;
    let cases: &[(&str, &str, usize)] = &[
        ("cdf:Other", "<ogc:PropertyIsGreaterThanOrEqualTo><ogc:PropertyName>cdf:integers</ogc:PropertyName><ogc:Literal>7</ogc:Literal></ogc:PropertyIsGreaterThanOrEqualTo>", 1),
        ("cdf:Other", "<ogc:PropertyIsBetween><ogc:PropertyName>cdf:dates</ogc:PropertyName><ogc:LowerBoundary><ogc:Literal>2002-12-02</ogc:Literal></ogc:LowerBoundary><ogc:UpperBoundary><ogc:Literal>2002-12-02</ogc:Literal></ogc:UpperBoundary></ogc:PropertyIsBetween>", 1),
        ("cdf:Other", "<ogc:PropertyIsLessThan><ogc:PropertyName>cdf:string2</ogc:PropertyName><ogc:Literal>tometimes</ogc:Literal></ogc:PropertyIsLessThan>", 1),
        ("cdf:Other", r#"<ogc:PropertyIsLike wildCard="*" singleChar="." escape="\"><ogc:PropertyName>cdf:string2</ogc:PropertyName><ogc:Literal>s.met*s</ogc:Literal></ogc:PropertyIsLike>"#, 1),
        ("cdf:Other", "<ogc:PropertyIsNotEqualTo><ogc:PropertyName>cdf:dates</ogc:PropertyName><ogc:Literal>2002-12-02</ogc:Literal></ogc:PropertyIsNotEqualTo>", 0),
        ("cdf:Nulls", "<ogc:PropertyIsNull><ogc:PropertyName>gml:name</ogc:PropertyName></ogc:PropertyIsNull>", 1),
        ("cdf:Nulls", "<ogc:PropertyIsNull><ogc:PropertyName>gml:pointProperty</ogc:PropertyName></ogc:PropertyIsNull>", 1),
    ];
    for (type_name, body, count) in cases {
        let filter = format!(
            r#"<ogc:Filter xmlns:ogc="http://www.opengis.net/ogc" xmlns:cdf="http://www.opengis.net/cite/data" xmlns:gml="http://www.opengis.net/gml">{body}</ogc:Filter>"#
        );
        // TEAM Engine form: filter percent-encoded once with '+' for spaces
        let xml = srv
            .get(&format!("/wfs?service=WFS&version=1.0.0&request=GetFeature&typename={type_name}&outputformat=GML2&filter={}", enc(&filter)))
            .await
            .xml();
        xml.assert_not("/ogc:ServiceExceptionReport");
        let local = type_name.split(':').nth(1).unwrap();
        assert_eq!(
            xml.count(&format!("//gml:featureMember/cdf:{local}")),
            *count,
            "GET {body}"
        );
        let xml = srv
            .post_wfs(&format!(r#"<wfs:GetFeature service="WFS" version="1.0.0" outputFormat="GML2" xmlns:wfs="http://www.opengis.net/wfs" xmlns:ogc="http://www.opengis.net/ogc" xmlns:cdf="http://www.opengis.net/cite/data" xmlns:gml="http://www.opengis.net/gml"><wfs:Query typeName="{type_name}">{filter}</wfs:Query></wfs:GetFeature>"#))
            .await
            .xml();
        assert_eq!(
            xml.count(&format!("//gml:featureMember/cdf:{local}")),
            *count,
            "POST {body}"
        );
    }
}

#[actix_web::test]
async fn wfs10_spatial_operators_http() {
    // all 138 ets-wfs10 spatial cases via POST
    let srv = cite10_server().await;
    let cases =
        std::fs::read_to_string(fixtures::data_dir().join("cite/wfs10/spatial-cases.tsv")).unwrap();
    for line in cases.lines().filter(|l| !l.starts_with('#')) {
        let cols: Vec<&str> = line.split('\t').collect();
        let (test, type_name, filter, id, expected) =
            (cols[0], cols[1], cols[3], cols[4], cols[5] == "true");
        let xml = srv
            .post_wfs(&format!(r#"<wfs:GetFeature service="WFS" version="1.0.0" outputFormat="GML2" xmlns:wfs="http://www.opengis.net/wfs" xmlns:ogc="http://www.opengis.net/ogc" xmlns:gml="http://www.opengis.net/gml" xmlns:cgf="http://www.opengis.net/cite/geometry"><wfs:Query typeName="{type_name}"><ogc:Filter>{filter}</ogc:Filter></wfs:Query></wfs:GetFeature>"#))
            .await
            .xml();
        xml.assert_not("/ogc:ServiceExceptionReport");
        let present = xml.boolean(&format!(
            "boolean(/wfs:FeatureCollection/gml:featureMember/{type_name}/cgf:id[text()='{id}'])"
        ));
        assert_eq!(present, expected, "{test}");
    }
}

#[actix_web::test]
async fn cql_filter_vendor_parameter() {
    let srv = synth_server().await;
    let id42 = observation(42).image_id;
    let cql = format!("image_id = '{id42}'");
    let xml = srv.get(&format!("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&cql_filter={}", enc(&cql))).await.xml();
    assert_eq!(matched(&xml), 1);
    // WFS 2.0: EPSG:4326 coordinates in latitude/longitude order (as in GeoServer)
    let cql = "cloud < 10 AND label IS NOT NULL AND BBOX(geom, -90, -180, 90, 180)";
    let xml = srv.get(&format!("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&count=5000&CQL_FILTER={}", enc(cql))).await.xml();
    assert_eq!(
        matched(&xml),
        expected(|o| o.cloud < 10.0 && o.label.is_some())
    );
    let cql = "acquired DURING 2020-01-01T10:00:00Z/2020-01-01T20:00:00Z";
    let xml = srv.get(&format!("/wfs?service=WFS&version=1.1.0&request=GetFeature&typeName=app:observations&cql_filter={}", enc(cql))).await.xml();
    xml.assert("/wfs:FeatureCollection[@numberOfFeatures='9']");
    let resp = srv.get(&format!("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=app:observations&cql_filter={}", enc("cloud <"))).await;
    resp.assert_exception("InvalidParameterValue");
}

#[actix_web::test]
async fn extension_operators() {
    let srv = synth_server().await;
    let ids: String = [3, 4]
        .iter()
        .map(|i| format!("<fes:Literal>{}</fes:Literal>", observation(*i).image_id))
        .collect();
    check(
        &srv,
        &format!(
            r#"<x:PropertyIsIn xmlns:x="https://www.bbox.earth/fes">{}{ids}</x:PropertyIsIn>"#,
            vr("image_id")
        ),
        2,
    )
    .await;
    check(&srv, &format!(r#"<x:PropertyIsILike xmlns:x="https://www.bbox.earth/fes" wildCard="*" singleChar="?" escapeChar="\">{}{}</x:PropertyIsILike>"#, vr("label"), lit("CLASS-1")), expected(|o| o.label.as_deref() == Some("class-1"))).await;
}
