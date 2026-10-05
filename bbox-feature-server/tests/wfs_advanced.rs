//! Joins, inheritance, xlink resolution, GetGmlObject and SOAP.
//! Assertions follow ets-wfs20 (joins, SOAP), ets-wfs11 WFS-XLink class and GeoServer join tests.

mod common;
use common::fixtures::observation;
use common::*;

fn enc(s: &str) -> String {
    serde_urlencoded::to_string([("x", s)]).unwrap()[2..].to_string()
}

const NS: &str = r#"xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:app="http://example.com/app""#;

// ---------------------------------------------------------------- joins (WFS 2.0)

#[actix_web::test]
async fn standard_join() {
    let srv = synth_server().await;
    let body = format!(
        r#"<wfs:GetFeature version="2.0.0" service="WFS" {NS}><wfs:Query typeNames="app:footprints app:observations" aliases="a b"><fes:Filter><fes:And>
        <fes:PropertyIsEqualTo><fes:ValueReference>a/app:image_id</fes:ValueReference><fes:ValueReference>b/app:image_id</fes:ValueReference></fes:PropertyIsEqualTo>
        <fes:PropertyIsLessThan><fes:ValueReference>b/app:cloud</fes:ValueReference><fes:Literal>2</fes:Literal></fes:PropertyIsLessThan>
        </fes:And></fes:Filter></wfs:Query></wfs:GetFeature>"#
    );
    let resp = srv.post_wfs(&body).await;
    resp.assert_ok();
    let xml = resp.xml();
    let expected = (0..SYNTH_ROWS)
        .filter(|i| observation(*i).cloud < 2.0)
        .count();
    xml.assert(&format!(
        "/wfs2:FeatureCollection[@numberMatched='{expected}' and @numberReturned='{expected}']"
    ));
    xml.assert_count("//wfs2:member/wfs2:Tuple", expected);
    xml.assert_count(
        "//wfs2:Tuple/wfs2:member/*[local-name()='footprints']",
        expected,
    );
    xml.assert_count(
        "//wfs2:Tuple/wfs2:member/*[local-name()='observations']",
        expected,
    );
    // matching image ids in each tuple
    let a = xml.strings("//wfs2:Tuple/wfs2:member[1]/*/*[local-name()='image_id']");
    let b = xml.strings("//wfs2:Tuple/wfs2:member[2]/*/*[local-name()='image_id']");
    assert_eq!(a, b);
    xml.assert_valid("http://schemas.opengis.net/wfs/2.0/wfs.xsd");
}

#[actix_web::test]
async fn spatial_join_kvp() {
    let srv = synth_server().await;
    let filter = r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:app="http://example.com/app" xmlns:gml="http://www.opengis.net/gml/3.2"><fes:And><fes:Intersects><fes:ValueReference>a/app:geom</fes:ValueReference><fes:ValueReference>b/app:geom</fes:ValueReference></fes:Intersects><fes:BBOX><fes:ValueReference>a/app:geom</fes:ValueReference><gml:Envelope srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>0 0</gml:lowerCorner><gml:upperCorner>20 20</gml:upperCorner></gml:Envelope></fes:BBOX></fes:And></fes:Filter>"#;
    let resp = srv
        .get(&format!("/wfs?service=WFS&version=2.0.0&request=GetFeature&typenames=app:footprints,app:observations&aliases=a,b&filter={}", enc(filter)))
        .await;
    resp.assert_ok();
    let xml = resp.xml();
    // expected: pairs (footprint i, observation j) with observation j inside footprint i, footprint in bbox
    let obs: Vec<_> = (0..SYNTH_ROWS).map(observation).collect();
    let mut expected = 0;
    for f in &obs {
        let (minx, maxx, miny, maxy) = (f.lon - 0.05, f.lon + 0.05, f.lat - 0.05, f.lat + 0.05);
        if !(maxx >= 0.0 && minx <= 20.0 && maxy >= 0.0 && miny <= 20.0) {
            continue;
        }
        expected += obs
            .iter()
            .filter(|o| o.lon >= minx && o.lon <= maxx && o.lat >= miny && o.lat <= maxy)
            .count();
    }
    assert!(expected > 0);
    xml.assert_count("//wfs2:Tuple", expected);
}

#[actix_web::test]
async fn temporal_join() {
    let srv = synth_server().await;
    let body = format!(
        r#"<wfs:GetFeature version="2.0.0" service="WFS" {NS}><wfs:Query typeNames="app:observations app:observations" aliases="a b"><fes:Filter><fes:And>
        <fes:PropertyIsEqualTo><fes:ValueReference>a/app:cloud</fes:ValueReference><fes:Literal>0</fes:Literal></fes:PropertyIsEqualTo>
        <fes:PropertyIsEqualTo><fes:ValueReference>b/app:cloud</fes:ValueReference><fes:Literal>0</fes:Literal></fes:PropertyIsEqualTo>
        <fes:After><fes:ValueReference>a/app:acquired</fes:ValueReference><fes:ValueReference>b/app:acquired</fes:ValueReference></fes:After>
        </fes:And></fes:Filter></wfs:Query></wfs:GetFeature>"#
    );
    let xml = srv.post_wfs(&body).await.xml();
    let n = (0..SYNTH_ROWS)
        .filter(|i| observation(*i).cloud == 0.0)
        .count();
    xml.assert_count("//wfs2:Tuple", n * (n - 1) / 2);
}

// ---------------------------------------------------------------- inheritance

#[actix_web::test]
async fn query_abstract_supertype() {
    let srv = cite10_server().await;
    // cgf:_SimpleFeature is the abstract head of all cgf geometry types
    let xml = srv
        .get("/wfs?service=WFS&version=1.1.0&request=GetFeature&typeName=cgf:_SimpleFeature")
        .await
        .xml();
    xml.assert_count("//gml:featureMember/*", 6);
    let ids = xml.strings("//gml:featureMember/*/cgf:id");
    assert_eq!(ids.len(), 6);
    // filter on a property of the supertype
    let filter = r#"<ogc:Filter xmlns:ogc="http://www.opengis.net/ogc" xmlns:cgf="http://www.opengis.net/cite/geometry"><ogc:PropertyIsEqualTo><ogc:PropertyName>cgf:id</ogc:PropertyName><ogc:Literal>t0003</ogc:Literal></ogc:PropertyIsEqualTo></ogc:Filter>"#;
    let xml = srv.get(&format!("/wfs?service=WFS&version=1.1.0&request=GetFeature&typeName=cgf:_SimpleFeature&filter={}", enc(filter))).await.xml();
    xml.assert_count("//gml:featureMember/cgf:MPoints", 1);
    xml.assert_count("//gml:featureMember/*", 1);
    // schema-element() function
    let filter = r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:cgf="http://www.opengis.net/cite/geometry"><fes:PropertyIsEqualTo><fes:ValueReference>schema-element(cgf:_SimpleFeature)/cgf:id</fes:ValueReference><fes:Literal>t0001</fes:Literal></fes:PropertyIsEqualTo></fes:Filter>"#;
    let xml = srv.get(&format!("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=cgf:_SimpleFeature&filter={}", enc(filter))).await.xml();
    xml.assert_count("//wfs2:member/*", 1);
    xml.assert("//wfs2:member/cgf:Lines");
}

// ---------------------------------------------------------------- xlink (WFS 1.1 XLink class)

fn xlink_query(depth: &str, type_name: &str, name: &str) -> String {
    format!(
        r#"<wfs:GetFeature xmlns:wfs="http://www.opengis.net/wfs" version="1.1.0" service="WFS" traverseXlinkDepth="{depth}"><wfs:Query xmlns:sf="http://cite.opengeospatial.org/gmlsf" typeName="{type_name}"><ogc:Filter xmlns:ogc="http://www.opengis.net/ogc"><ogc:PropertyIsEqualTo><ogc:PropertyName xmlns:gml="http://www.opengis.net/gml">gml:name</ogc:PropertyName><ogc:Literal>{name}</ogc:Literal></ogc:PropertyIsEqualTo></ogc:Filter></wfs:Query></wfs:GetFeature>"#
    )
}

#[actix_web::test]
async fn xlink_depth_one() {
    let srv = cite11_server().await;
    let xml = srv
        .post_wfs(&xlink_query("1", "sf:LinkedFeature", "name-f202"))
        .await
        .xml();
    let m = "//gml:featureMember/sf:LinkedFeature";
    xml.assert(&format!("{m}/gml:name = 'name-f202'"));
    xml.assert(&format!(
        "{m}/sf:reference/sf:PrimitiveGeoFeature/gml:name = 'name-f092'"
    ));
    xml.assert_not(&format!("{m}/sf:reference/@xlink:href"));
    xml.assert(&format!("{m}/sf:reference/comment()"));
    // nested link of the inlined feature stays unresolved at depth 1
    xml.assert(&format!(
        "{m}/sf:reference/sf:PrimitiveGeoFeature/sf:relatedFeature/@xlink:href"
    ));
}

#[actix_web::test]
async fn xlink_depth_zero() {
    let srv = cite11_server().await;
    let xml = srv
        .post_wfs(&xlink_query("0", "sf:LinkedFeature", "name-f202"))
        .await
        .xml();
    let m = "//gml:featureMember/sf:LinkedFeature";
    xml.assert_count(&format!("{m}/sf:reference/*"), 0);
    xml.assert(&format!("{m}/sf:reference/@xlink:href"));
}

#[actix_web::test]
async fn xlink_depth_and_cycles() {
    let srv = cite11_server().await;
    let m = "//gml:featureMember/sf:LinkedFeature";
    let xml = srv
        .post_wfs(&xlink_query("1", "sf:LinkedFeature", "name-f206"))
        .await
        .xml();
    xml.assert(&format!(
        "{m}/sf:reference/sf:LinkedFeature/gml:name = 'name-f207'"
    ));
    xml.assert_count(
        &format!("{m}/sf:reference/sf:LinkedFeature/sf:reference/*"),
        0,
    );
    let xml = srv
        .post_wfs(&xlink_query("2", "sf:LinkedFeature", "name-f206"))
        .await
        .xml();
    xml.assert(&format!(
        "{m}/sf:reference/sf:LinkedFeature/sf:reference/sf:LinkedFeature/gml:name = 'name-f210'"
    ));
    xml.assert_count(
        &format!("{m}/sf:reference/sf:LinkedFeature/sf:reference/sf:LinkedFeature/sf:reference/*"),
        0,
    );
    // unlimited depth stops at the cycle f206 -> f207 -> f210 -> f206
    let resp = srv
        .post_wfs(&xlink_query("*", "sf:LinkedFeature", "name-f206"))
        .await;
    let xml = resp.xml();
    xml.assert(&format!(
        "{m}/sf:reference/sf:LinkedFeature/sf:reference/sf:LinkedFeature/gml:name = 'name-f210'"
    ));
    xml.assert(&format!("{m}/sf:reference/sf:LinkedFeature/sf:reference/sf:LinkedFeature/sf:reference/@xlink:href = '#f206'"));
    let ids = xml.strings("//@gml:id");
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(ids.len(), unique.len(), "duplicate gml:id");
}

#[actix_web::test]
async fn xlink_property_name_depth() {
    // wfs:XlinkPropertyName with its own traverseXlinkDepth (ets-wfs11 GetFeature.XLink-POST-XML-10/11)
    let srv = cite11_server().await;
    let query = |global: &str, prop: &str, depth: &str, name: &str| {
        format!(
            r#"<wfs:GetFeature xmlns:wfs="http://www.opengis.net/wfs" service="WFS" traverseXlinkDepth="{global}" version="1.1.0"><wfs:Query xmlns:sf="http://cite.opengeospatial.org/gmlsf" xmlns:gml="http://www.opengis.net/gml" typeName="sf:LinkedFeature"><wfs:PropertyName>gml:name</wfs:PropertyName><wfs:XlinkPropertyName traverseXlinkDepth="{depth}">{prop}</wfs:XlinkPropertyName><ogc:Filter xmlns:ogc="http://www.opengis.net/ogc"><ogc:PropertyIsEqualTo><ogc:PropertyName>gml:name</ogc:PropertyName><ogc:Literal>{name}</ogc:Literal></ogc:PropertyIsEqualTo></ogc:Filter></wfs:Query></wfs:GetFeature>"#
        )
    };
    let m = "//gml:featureMember/sf:LinkedFeature";
    // global depth 0, sf:extent resolved to depth 1; sf:reference stays a link
    let xml = srv
        .post_wfs(&query("0", "sf:extent", "1", "name-f205"))
        .await
        .xml();
    xml.assert_not(&format!("{m}/sf:extent/@xlink:href"));
    xml.assert(&format!("{m}/sf:extent/comment()"));
    xml.assert(&format!("{m}/sf:extent/*/gml:name = 'MU1'"));
    xml.assert(&format!("{m}/sf:reference/@xlink:href = '#f203'"));
    // global depth 1, sf:reference resolved two levels deep
    let xml = srv
        .post_wfs(&query("1", "sf:reference", "2", "name-f210"))
        .await
        .xml();
    let inner = format!("{m}/sf:reference/sf:LinkedFeature/sf:reference");
    xml.assert(&format!(
        "{m}/sf:reference/sf:LinkedFeature/gml:name = 'name-f206'"
    ));
    xml.assert_not(&format!("{inner}/@xlink:href"));
    xml.assert(&format!("{inner}/comment()"));
    xml.assert(&format!("{inner}/sf:LinkedFeature/gml:name = 'name-f207'"));
    // without a global traverseXlinkDepth
    let q = query("0", "sf:extent", "1", "name-f205")
        .replace(r#" traverseXlinkDepth="0" version"#, r#" version"#);
    let xml = srv.post_wfs(&q).await.xml();
    xml.assert(&format!("{m}/sf:extent/*/gml:name = 'MU1'"));
    // KVP: PROPTRAVXLINKDEPTH aligned with PROPERTYNAME
    let xml = srv.get("/wfs?service=WFS&version=1.1.0&request=GetFeature&typename=sf:LinkedFeature&featureid=f205&propertyname=sf:reference,sf:extent&proptravxlinkdepth=0,1").await.xml();
    xml.assert(&format!("{m}/sf:extent/*/gml:name = 'MU1'"));
    xml.assert(&format!("{m}/sf:reference/@xlink:href = '#f203'"));
}

#[actix_web::test]
async fn xlink_geometry_references() {
    let srv = cite11_server().await;
    let xml = srv
        .post_wfs(&xlink_query("1", "sf:LinkedFeature", "name-f204"))
        .await
        .xml();
    let m = "//gml:featureMember/sf:LinkedFeature";
    xml.assert(&format!(
        "{m}/sf:reference/sf:LinkedFeature/gml:name = 'name-f201'"
    ));
    xml.assert_count(
        &format!("{m}/sf:extent/gml:MultiPoint/gml:pointMember/gml:Point"),
        2,
    );
    xml.assert(&format!("{m}/sf:extent/gml:MultiPoint/gml:pointMember/gml:Point/gml:description = 'description-g003'"));
}

#[actix_web::test]
async fn xlink_errors() {
    let srv = cite11_server().await;
    // non-local scheme (ftp) -> NoApplicableCode
    let resp = srv
        .post_wfs(&xlink_query("*", "sf:LinkedFeature", "name-f208"))
        .await;
    resp.assert_exception("NoApplicableCode");
    // dangling local link -> NoApplicableCode
    let resp = srv
        .post_wfs(&xlink_query("*", "sf:LinkedFeature", "name-f209"))
        .await;
    resp.assert_exception("NoApplicableCode");
    // expiry 0 -> exception with text
    let resp = srv.get("/wfs?request=GetFeature&service=WFS&version=1.1.0&typename=sf%3ALinkedFeature&traverseXlinkExpiry=0").await;
    resp.xml().assert("//ows:Exception/ows:ExceptionText");
    // unresolvable remote http link is kept, no exception
    let resp = srv
        .post_wfs(r#"<wfs:GetFeature xmlns:wfs="http://www.opengis.net/wfs" version="1.1.0" service="WFS" traverseXlinkDepth="*" traverseXlinkExpiry="1"><wfs:Query xmlns:sf="http://cite.opengeospatial.org/gmlsf" typeName="sf:PrimitiveGeoFeature"/></wfs:GetFeature>"#)
        .await;
    resp.assert_ok();
    resp.xml().assert("//wfs:FeatureCollection");
}

#[actix_web::test]
async fn wfs20_resolve_local() {
    let srv = cite11_server().await;
    let filter = r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:gml="http://www.opengis.net/gml"><fes:PropertyIsEqualTo><fes:ValueReference>gml:name</fes:ValueReference><fes:Literal>name-f202</fes:Literal></fes:PropertyIsEqualTo></fes:Filter>"#;
    let xml = srv
        .get(&format!("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=sf:LinkedFeature&resolve=local&resolveDepth=1&filter={}", enc(filter)))
        .await
        .xml();
    xml.assert("//sf:LinkedFeature/sf:reference/sf:PrimitiveGeoFeature");
    let xml = srv
        .get(&format!("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=sf:LinkedFeature&filter={}", enc(filter)))
        .await
        .xml();
    xml.assert("//sf:LinkedFeature/sf:reference/@xlink:href");
}

#[actix_web::test]
async fn resolve_remote() {
    // serve a remote GML document
    let server = actix_web::HttpServer::new(|| {
        actix_web::App::new().route(
            "/remote.xml",
            actix_web::web::get().to(|| async {
                actix_web::HttpResponse::Ok().content_type("text/xml").body(
                    r#"<sf:PrimitiveGeoFeature xmlns:sf="http://cite.opengeospatial.org/gmlsf" xmlns:gml="http://www.opengis.net/gml" gml:id="r1"><gml:name>remote-feature</gml:name></sf:PrimitiveGeoFeature>"#,
                )
            }),
        )
    })
    .bind("127.0.0.1:0")
    .unwrap();
    let port = server.addrs()[0].port();
    let handle = server.run();
    let srv_handle = handle.handle();
    actix_web::rt::spawn(handle);
    let srv = cite11_remote_server(&format!("http://127.0.0.1:{port}/remote.xml#r1")).await;
    let xml = srv
        .post_wfs(&xlink_query("1", "sf:LinkedFeature", "name-f299"))
        .await
        .xml();
    xml.assert(
        "//sf:LinkedFeature/sf:reference/sf:PrimitiveGeoFeature/gml:name = 'remote-feature'",
    );
    xml.assert_not("//sf:LinkedFeature/sf:reference/@xlink:href");
    srv_handle.stop(true).await;
}

// ---------------------------------------------------------------- GetGmlObject (WFS 1.1)

#[actix_web::test]
async fn get_gml_object() {
    let srv = cite11_server().await;
    let resp = srv
        .post_wfs(r#"<wfs:GetGmlObject service="WFS" version="1.1.0" xmlns:wfs="http://www.opengis.net/wfs" xmlns:ogc="http://www.opengis.net/ogc" xmlns:gml="http://www.opengis.net/gml"><ogc:GmlObjectId gml:id="f001"/></wfs:GetGmlObject>"#)
        .await;
    resp.assert_ok();
    resp.xml().assert("/sf:PrimitiveGeoFeature[@gml:id='f001']");
    // geometry object
    let xml = srv
        .post_wfs(r#"<wfs:GetGmlObject service="WFS" version="1.1.0" xmlns:wfs="http://www.opengis.net/wfs" xmlns:ogc="http://www.opengis.net/ogc" xmlns:gml="http://www.opengis.net/gml"><ogc:GmlObjectId gml:id="g003"/></wfs:GetGmlObject>"#)
        .await
        .xml();
    xml.assert("/gml:Point[@gml:id='g003']");
    // KVP
    let xml = srv
        .get("/wfs?service=WFS&version=1.1.0&request=GetGmlObject&gmlobjectid=f004")
        .await
        .xml();
    xml.assert("/*[@gml:id='f004']");
    // unknown
    let resp = srv
        .get("/wfs?service=WFS&version=1.1.0&request=GetGmlObject&gmlobjectid=nothing")
        .await;
    resp.xml().assert("/ows:ExceptionReport");
}

// ---------------------------------------------------------------- SOAP

#[actix_web::test]
async fn soap12_get_feature() {
    let srv = synth_server().await;
    let body = r#"<?xml version="1.0"?><soap:Envelope xmlns:soap="http://www.w3.org/2003/05/soap-envelope"><soap:Body><wfs:GetFeature version="2.0.2" service="WFS" count="2" xmlns:wfs="http://www.opengis.net/wfs/2.0"><wfs:Query typeNames="app:observations" xmlns:app="http://example.com/app"/></wfs:GetFeature></soap:Body></soap:Envelope>"#;
    let resp = srv
        .post_with_type("/wfs", body, "application/soap+xml; charset=UTF-8")
        .await;
    resp.assert_ok();
    assert!(
        resp.content_type.starts_with("application/soap+xml"),
        "{}",
        resp.content_type
    );
    let xml = resp.xml();
    xml.assert("/*[local-name()='Envelope']/*[local-name()='Body']/wfs2:FeatureCollection[@numberReturned='2']");
    // fault
    let body = r#"<soap:Envelope xmlns:soap="http://www.w3.org/2003/05/soap-envelope"><soap:Body><wfs:GetFeature version="2.0.2" service="WFS" xmlns:wfs="http://www.opengis.net/wfs/2.0"><wfs:Query typeNames="app:unknown" xmlns:app="http://example.com/app"/></wfs:GetFeature></soap:Body></soap:Envelope>"#;
    let resp = srv
        .post_with_type("/wfs", body, "application/soap+xml")
        .await;
    let xml = resp.xml();
    xml.assert("//*[local-name()='Fault']//ows11:ExceptionReport/ows11:Exception[@exceptionCode='InvalidParameterValue']");
}

#[actix_web::test]
async fn soap11_describe_feature_type() {
    let srv = synth_server().await;
    let body = r#"<soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/"><soapenv:Body><wfs:DescribeFeatureType version="2.0.0" service="WFS" xmlns:wfs="http://www.opengis.net/wfs/2.0"/></soapenv:Body></soapenv:Envelope>"#;
    let resp = srv.post_with_type("/wfs", body, "text/xml").await;
    resp.assert_ok();
    let xml = resp.xml();
    // schema is returned base64 encoded in wfs:DescribeFeatureTypeResponse
    let b64 = xml.string("//wfs2:DescribeFeatureTypeResponse");
    assert!(!b64.trim().is_empty());
    let schema = String::from_utf8(base64_decode(b64.trim())).unwrap();
    assert!(schema.contains("targetNamespace=\"http://example.com/app\""));
}

fn base64_decode(s: &str) -> Vec<u8> {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
        let v = T.iter().position(|t| *t == c).expect("base64") as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    out
}
