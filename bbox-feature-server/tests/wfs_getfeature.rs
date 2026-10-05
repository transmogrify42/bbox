//! GetFeature output (GML 2, GML 3.1.1, GML 3.2) and basic query parameters.
//! Assertions follow ets-wfs10 basic-getfeature, ets-wfs11 basic-cc and ets-wfs20 BasicGetFeatureTests.

mod common;
use common::*;

const WFS10_BASIC: &str = "http://schemas.opengis.net/wfs/1.0.0/WFS-basic.xsd";
const WFS11: &str = "http://schemas.opengis.net/wfs/1.1.0/wfs.xsd";
const WFS20: &str = "http://schemas.opengis.net/wfs/2.0/wfs.xsd";

async fn dft(srv: &TestServer, version: &str, type_names: &str) -> String {
    let resp = srv
        .get(&format!(
            "/wfs?service=WFS&version={version}&request=DescribeFeatureType&typeName={type_names}"
        ))
        .await;
    resp.assert_ok();
    resp.body
}

// ---------------------------------------------------------------- WFS 1.0 / GML 2

#[actix_web::test]
async fn wfs10_get_feature_other() {
    let srv = cite10_server().await;
    for req in [
        "/wfs?service=WFS&version=1.0.0&request=GetFeature&typename=cdf:Other&outputformat=GML2",
        "/wfs?service=WFS&version=1.0.0&request=GetFeature&typename=cdf:Other",
    ] {
        let resp = srv.get(req).await;
        resp.assert_ok();
        let xml = resp.xml();
        // streamed collections can not know their extent in advance
        xml.assert("/wfs:FeatureCollection/gml:boundedBy/gml:Box or /wfs:FeatureCollection/gml:boundedBy/gml:null");
        xml.assert_count("//gml:featureMember/cdf:Other", 1);
        let other = "//gml:featureMember/cdf:Other";
        // all properties in schema order
        let names = xml.child_names(other);
        assert_eq!(
            names,
            vec![
                "gml:description",
                "gml:name",
                "gml:boundedBy",
                "gml:pointProperty",
                "cdf:string1",
                "cdf:string2",
                "cdf:integers",
                "cdf:dates"
            ],
            "{}",
            resp.body
        );
        assert_eq!(xml.string(&format!("{other}/@fid")), "Other.1");
        assert_eq!(xml.string(&format!("{other}/gml:name")), "singleFeature");
        assert_eq!(xml.string(&format!("{other}/cdf:integers")), "7");
        assert_eq!(xml.string(&format!("{other}/cdf:dates")), "2002-12-02");
        assert_eq!(
            xml.string(&format!(
                "{other}/gml:pointProperty/gml:Point/gml:coordinates"
            )),
            "500050,500050"
        );
        xml.assert(&format!(
            "{other}/gml:pointProperty/gml:Point[@srsName='EPSG:32615']"
        ));
        assert_eq!(
            xml.string(&format!("{other}/gml:boundedBy/gml:Box/gml:coordinates")),
            "500000,500000 500100,500100"
        );
        xml.assert_valid_with_schema_doc(WFS10_BASIC, &dft(&srv, "1.0.0", "cdf:Other").await);
    }
}

#[actix_web::test]
async fn wfs10_post_get_feature() {
    let srv = cite10_server().await;
    let resp = srv
        .post_wfs(r#"<?xml version="1.0" encoding="UTF-8"?><wfs:GetFeature service="WFS" version="1.0.0" outputFormat="GML2" xmlns:wfs="http://www.opengis.net/wfs" xmlns:ogc="http://www.opengis.net/ogc" xmlns:cdf="http://www.opengis.net/cite/data"><wfs:Query typeName="cdf:Other"><ogc:PropertyName>cdf:string2</ogc:PropertyName></wfs:Query></wfs:GetFeature>"#)
        .await;
    resp.assert_ok();
    let xml = resp.xml();
    // mandatory string1 is returned although not requested (basic-getfeature-4)
    xml.assert("//cdf:Other[cdf:string1 and cdf:string2]");
    xml.assert_not("//cdf:Other/cdf:integers");
    xml.assert_valid_with_schema_doc(WFS10_BASIC, &dft(&srv, "1.0.0", "cdf:Other").await);
}

#[actix_web::test]
async fn wfs10_counts_and_max_features() {
    let srv = cite10_server().await;
    let xml = srv
        .get("/wfs?service=WFS&version=1.0.0&request=GetFeature&typename=cdf:Fifteen")
        .await
        .xml();
    xml.assert_count("//gml:featureMember", 15);
    xml.assert_count("//gml:featureMember/cdf:Fifteen", 15);
    let xml = srv
        .get("/wfs?service=WFS&version=1.0.0&request=GetFeature&typename=cdf:Fifteen,cdf:Seven")
        .await
        .xml();
    xml.assert_count("//gml:featureMember/cdf:Fifteen", 15);
    xml.assert_count("//gml:featureMember/cdf:Seven", 7);
    let xml = srv
        .get(
            "/wfs?service=WFS&version=1.0.0&request=GetFeature&typename=cdf:Fifteen&maxfeatures=10",
        )
        .await
        .xml();
    xml.assert_count("//gml:featureMember", 10);
    // maxFeatures is global across queries, filled in query order
    let xml = srv
        .post_wfs(r#"<wfs:GetFeature service="WFS" version="1.0.0" maxFeatures="20" xmlns:wfs="http://www.opengis.net/wfs" xmlns:cdf="http://www.opengis.net/cite/data"><wfs:Query typeName="cdf:Fifteen"/><wfs:Query typeName="cdf:Seven"/></wfs:GetFeature>"#)
        .await
        .xml();
    xml.assert_count("//gml:featureMember/cdf:Fifteen", 15);
    xml.assert_count("//gml:featureMember/cdf:Seven", 5);
    // fids are unique NCNames and stable
    let ids = xml.strings("//gml:featureMember/*/@fid");
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(ids.len(), unique.len());
    assert!(ids
        .iter()
        .all(|id| id.chars().next().unwrap().is_alphabetic()));
}

#[actix_web::test]
async fn wfs10_errors_and_empty_result() {
    let srv = cite10_server().await;
    let resp = srv.get("/wfs?service=WFS&version=1.0.0&request=GetFeature&typename=cdf:Other&outputformat=DUMMYFORMAT").await;
    assert_eq!(resp.status, 200);
    resp.xml().assert("/ogc:ServiceExceptionReport");
    let resp = srv
        .get("/wfs?service=WFS&version=1.0.0&request=GetFeature&typename=cdf:Unknown")
        .await;
    resp.xml().assert("/ogc:ServiceExceptionReport");
    // empty result is a schema valid collection with gml:boundedBy/gml:null
    let filter = "%3Cogc%3AFilter+xmlns%3Aogc%3D%22http%3A%2F%2Fwww.opengis.net%2Fogc%22%3E%3Cogc%3AFeatureId+fid%3D%22NONE%22%2F%3E%3C%2Fogc%3AFilter%3E";
    let xml = srv
        .get(&format!(
            "/wfs?service=WFS&version=1.0.0&request=GetFeature&typename=cdf:Other&filter={filter}"
        ))
        .await
        .xml();
    xml.assert("/wfs:FeatureCollection/gml:boundedBy/gml:null");
    xml.assert_count("//gml:featureMember", 0);
    xml.assert_valid_with_schema_doc(WFS10_BASIC, &dft(&srv, "1.0.0", "cdf:Other").await);
}

#[actix_web::test]
async fn wfs10_geometry_types() {
    let srv = cite10_server().await;
    let xml = srv.get("/wfs?service=WFS&version=1.0.0&request=GetFeature&typename=cgf:MPolygons,cgf:Lines,cgf:MPoints").await.xml();
    xml.assert("//cgf:MPolygons/gml:multiPolygonProperty/gml:MultiPolygon/gml:polygonMember/gml:Polygon/gml:outerBoundaryIs/gml:LinearRing/gml:coordinates");
    xml.assert("//cgf:Lines/gml:lineStringProperty/gml:LineString/gml:coordinates = '500125,500025 500175,500075'");
    xml.assert_count(
        "//cgf:MPoints/gml:multiPointProperty/gml:MultiPoint/gml:pointMember",
        2,
    );
    assert_eq!(xml.string("//cgf:Lines/cgf:id"), "t0001");
    xml.assert_valid_with_schema_doc(
        WFS10_BASIC,
        &dft(&srv, "1.0.0", "cgf:MPolygons,cgf:Lines,cgf:MPoints").await,
    );
}

#[actix_web::test]
async fn wfs10_complex_values() {
    let srv = cite10_server().await;
    let get = |p: &'static str| {
        let srv = &srv;
        async move {
            srv.get(&format!("/wfs?service=WFS&version=1.0.0&request=GetFeature&typename=ccf:Complex&outputformat=GML2&propertyname={p}"))
                .await
                .xml()
        }
    };
    get("ccf:resident[2]")
        .await
        .assert("//ccf:Complex/ccf:resident[text()='Beth']");
    get("ccf:Complex/ccf:resident[2]")
        .await
        .assert("//ccf:Complex/ccf:resident[text()='Beth']");
    get("ccf:resident")
        .await
        .assert("//ccf:Complex[count(ccf:resident)=2]");
    get("ccf:Complex/ccf:address/ccf:Address/ccf:street/@number")
        .await
        .assert("//ccf:Complex/ccf:address/ccf:Address/ccf:street/@number = '10'");
    get("ccf:address/ccf:Address/ccf:street/@number")
        .await
        .assert("//ccf:Complex/ccf:address/ccf:Address/ccf:street/@number = '10'");
    get("ccf:address")
        .await
        .assert("//ccf:Complex[ccf:address]");
    get("ccf:address/ccf:Address/ccf:city")
        .await
        .assert("//ccf:Complex[ccf:address/ccf:Address/ccf:city]");
}

// ---------------------------------------------------------------- WFS 1.1 / GML 3.1.1

#[actix_web::test]
async fn wfs11_get_feature() {
    let srv = cite11_server().await;
    let resp = srv
        .get("/wfs?service=WFS&version=1.1.0&request=GetFeature&typename=sf:PrimitiveGeoFeature")
        .await;
    resp.assert_ok();
    assert_eq!(resp.content_type, "text/xml; subtype=gml/3.1.1");
    let xml = resp.xml();
    xml.assert("/wfs:FeatureCollection[@numberOfFeatures='9' and @timeStamp]");
    xml.assert_count("//gml:featureMember/sf:PrimitiveGeoFeature", 9);
    let f001 = "//sf:PrimitiveGeoFeature[@gml:id='f001']";
    xml.assert(f001);
    assert_eq!(xml.string(&format!("{f001}/gml:name")), "name-f001");
    assert_eq!(
        xml.string(&format!("{f001}/gml:name/@codeSpace")),
        "http://cite.opengeospatial.org/gmlsf"
    );
    // lat/lon axis order of urn EPSG::4326, geometry gml:id preserved
    assert_eq!(
        xml.string(&format!("{f001}/sf:pointProperty/gml:Point/gml:pos")),
        "39.73245 2.00342"
    );
    xml.assert(&format!("{f001}/sf:pointProperty/gml:Point[@gml:id='g003' and @srsName='urn:ogc:def:crs:EPSG::4326']"));
    assert_eq!(xml.string(&format!("{f001}/sf:intProperty")), "155");
    assert_eq!(
        xml.string(&format!("{f001}/sf:dateProperty")),
        "2006-10-25Z"
    );
    assert_eq!(xml.string(&format!("{f001}/sf:decimalProperty")), "5.03");
    // i64 values
    xml.assert("//sf:PrimitiveGeoFeature[@gml:id='f091']/sf:intProperty = '-12678967543233'");
    xml.assert_valid_with_schema_doc(WFS11, &dft(&srv, "1.1.0", "sf:PrimitiveGeoFeature").await);
}

#[actix_web::test]
async fn wfs11_hits_featureid_maxfeatures() {
    let srv = cite11_server().await;
    let xml = srv.get("/wfs?service=WFS&version=1.1.0&request=GetFeature&typename=sf:PrimitiveGeoFeature&resultType=hits").await.xml();
    xml.assert("/wfs:FeatureCollection[@numberOfFeatures='9']");
    xml.assert_count("//gml:featureMember", 0);
    // featureid without typename (helper used by Transaction/Locking ETS classes)
    let xml = srv
        .get("/wfs?service=WFS&version=1.1.0&request=GetFeature&featureid=f008")
        .await
        .xml();
    xml.assert_count("//gml:featureMember", 1);
    xml.assert("//gml:featureMember/*[1]/@gml:id = 'f008'");
    let xml = srv
        .get("/wfs?service=WFS&version=1.1.0&request=GetFeature&featureid=f008,f004")
        .await
        .xml();
    xml.assert_count("//gml:featureMember", 2);
    let xml = srv.get("/wfs?service=WFS&version=1.1.0&request=GetFeature&typename=sf:PrimitiveGeoFeature&maxFeatures=3").await.xml();
    xml.assert("/wfs:FeatureCollection[@numberOfFeatures='3']");
    let xml = srv.get("/wfs?service=WFS&version=1.1.0&request=GetFeature&typename=sf:Entit%C3%A9G%C3%A9n%C3%A9rique").await.xml();
    xml.assert_count("//gml:featureMember", 3);
    // WFS 1.1 KVP NAMESPACE parameter binding a client prefix (ets-wfs11 GetFeatureTestExtension)
    for q in [
        "typename=app:PrimitiveGeoFeature&namespace=xmlns(app=http://cite.opengeospatial.org/gmlsf)",
        "typename=app:PrimitiveGeoFeature&NAMESPACE=xmlns(app=http://cite.opengeospatial.org/gmlsf)",
    ] {
        let xml = srv.get(&format!("/wfs?service=WFS&version=1.1.0&request=GetFeature&{q}")).await.xml();
        xml.assert_count("//gml:featureMember/sf:PrimitiveGeoFeature", 9);
    }
    let resp = srv.get("/wfs?service=WFS&version=1.1.0&request=DescribeFeatureType&typename=app:PrimitiveGeoFeature&namespace=xmlns(app=http://cite.opengeospatial.org/gmlsf)").await;
    resp.assert_ok();
    resp.xml()
        .assert("//xs:complexType[@name='PrimitiveGeoFeatureType']");
}

#[actix_web::test]
async fn wfs11_aggregate_geometries() {
    let srv = cite11_server().await;
    let xml = srv
        .get("/wfs?service=WFS&version=1.1.0&request=GetFeature&typename=sf:AggregateGeoFeature")
        .await
        .xml();
    xml.assert("//sf:AggregateGeoFeature[@gml:id='f005']/sf:multiPointProperty/gml:MultiPoint");
    xml.assert("//sf:AggregateGeoFeature[@gml:id='f009']/sf:multiCurveProperty/gml:MultiCurve/gml:curveMember/gml:LineString/gml:posList");
    xml.assert("//sf:AggregateGeoFeature[@gml:id='f010']/sf:multiSurfaceProperty/gml:MultiSurface/gml:surfaceMember/gml:Polygon/gml:interior");
    xml.assert_valid_with_schema_doc(WFS11, &dft(&srv, "1.1.0", "sf:AggregateGeoFeature").await);
}

// ---------------------------------------------------------------- WFS 2.0 / GML 3.2

#[actix_web::test]
async fn wfs20_get_feature() {
    let srv = ne_server().await;
    let resp = srv
        .get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=ne:lakes&count=10")
        .await;
    resp.assert_ok();
    assert!(
        resp.content_type
            .starts_with("application/gml+xml; version=3.2"),
        "{}",
        resp.content_type
    );
    let xml = resp.xml();
    xml.assert(
        "/wfs2:FeatureCollection[@numberMatched='1355' and @numberReturned='10' and @timeStamp]",
    );
    xml.assert_count(
        "/wfs2:FeatureCollection/wfs2:member/*[local-name()='lakes']",
        10,
    );
    let first = "/wfs2:FeatureCollection/wfs2:member[1]/*";
    assert!(xml
        .string(&format!("{first}/@gml32:id"))
        .starts_with("lakes."));
    xml.assert(&format!("{first}/*[local-name()='geom']/gml32:MultiSurface[@srsName='urn:ogc:def:crs:EPSG::4326' and @gml32:id]"));
    // every gml:id unique
    let ids = xml.strings("//@gml32:id");
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(ids.len(), unique.len());
    xml.assert_valid_with_schema_doc(WFS20, &dft(&srv, "2.0.0", "ne:lakes").await);
}

#[actix_web::test]
async fn wfs20_axis_order_and_reprojection() {
    let srv = ne_server().await;
    // Bern (lon 7.47, lat 46.92); default urn CRS: lat lon
    let filter = urlencode(
        r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:ne="http://www.naturalearthdata.com"><fes:PropertyIsEqualTo><fes:ValueReference>ne:NAME</fes:ValueReference><fes:Literal>Bern</fes:Literal></fes:PropertyIsEqualTo></fes:Filter>"#,
    );
    let xml = srv.get(&format!("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=ne:populated_places&filter={filter}")).await.xml();
    let pos = xml.string("//gml32:Point/gml32:pos");
    let c: Vec<f64> = pos.split_whitespace().map(|v| v.parse().unwrap()).collect();
    assert!(
        (c[0] - 46.92).abs() < 0.1 && (c[1] - 7.47).abs() < 0.1,
        "{pos}"
    );
    // EPSG:4326 short form: lon lat
    let xml = srv.get(&format!("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=ne:populated_places&srsName=EPSG:4326&filter={filter}")).await.xml();
    let c: Vec<f64> = xml
        .string("//gml32:Point/gml32:pos")
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect();
    assert!((c[0] - 7.47).abs() < 0.1, "{c:?}");
    // other CRS: all srsName attributes equal the requested name
    let srs = "urn:ogc:def:crs:EPSG::3857";
    let xml = srv.get(&format!("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=ne:lakes&count=2&srsName={srs}")).await.xml();
    let names = xml.strings("//@srsName");
    assert!(!names.is_empty());
    assert!(names.iter().all(|n| n == srs), "{names:?}");
    let c: Vec<f64> = xml
        .string("(//gml32:posList)[1]")
        .split_whitespace()
        .take(2)
        .map(|v| v.parse().unwrap())
        .collect();
    assert!(c[0].abs() > 1000.0, "projected coordinates expected: {c:?}");
    // unsupported CRS
    let resp = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=ne:lakes&srsName=urn:ogc:def:crs:EPSG::32690").await;
    assert_eq!(resp.status, 400);
    let xml = resp.assert_exception("InvalidParameterValue");
    assert!(xml
        .string("//ows11:Exception/@locator")
        .to_lowercase()
        .contains("srsname"));
}

#[actix_web::test]
async fn wfs20_hits_and_post() {
    let srv = ne_server().await;
    let xml = srv
        .get(
            "/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=ne:rivers&resultType=hits",
        )
        .await
        .xml();
    xml.assert("/wfs2:FeatureCollection[@numberMatched='1473' and @numberReturned='0']");
    xml.assert_count("//wfs2:member", 0);
    let xml = srv
        .post_wfs(r#"<wfs:GetFeature version="2.0.0" service="WFS" count="5" xmlns:wfs="http://www.opengis.net/wfs/2.0"><wfs:Query xmlns:ns42="http://www.naturalearthdata.com" typeNames="ns42:rivers"/></wfs:GetFeature>"#)
        .await
        .xml();
    xml.assert("/wfs2:FeatureCollection[@numberReturned='5']");
    // KVP with NAMESPACES binding
    let xml = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&count=25&typenames=ns42:rivers&namespaces=xmlns(xml,http://www.w3.org/XML/1998/namespace),xmlns(wfs,http://www.opengis.net/wfs/2.0),xmlns(ns42,http://www.naturalearthdata.com)").await.xml();
    xml.assert_count("//wfs2:member", 25);
    // CountDefault applies without count
    let xml = srv
        .get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=ne:populated_places")
        .await
        .xml();
    xml.assert("/wfs2:FeatureCollection[@numberReturned='1000' and @numberMatched='7342']");
}

#[actix_web::test]
async fn wfs20_property_name_projection() {
    let srv = ne_server().await;
    let xml = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=ne:lakes&count=1&propertyName=ne:name").await.xml();
    let member = "//wfs2:member/*";
    xml.assert(&format!("{member}/*[local-name()='name']"));
    xml.assert_not(&format!("{member}/*[local-name()='geom']"));
    xml.assert_not(&format!("{member}/*[local-name()='scalerank']"));
}

pub fn urlencode(s: &str) -> String {
    serde_urlencoded::to_string([("x", s)]).unwrap()[2..].to_string()
}

#[actix_web::test]
async fn three_d_coordinates() {
    // sf-1 f101: LineString in EPSG:4979 (lat lon h)
    let srv = cite11_server().await;
    let q = "typename=sf:ComplexGeoFeature&featureid=f101";
    let xml = srv
        .get(&format!(
            "/wfs?service=WFS&version=1.1.0&request=GetFeature&{q}"
        ))
        .await
        .xml();
    let ls = "//sf:ComplexGeoFeature/sf:geometryProperty/gml:LineString";
    xml.assert(&format!("{ls}[@srsDimension='3']"));
    assert_eq!(
        xml.string(&format!("{ls}/gml:posList")),
        "46.074 9.799 600.2 46.652 10.466 781.4"
    );
    xml.assert_valid_with_schema_doc(WFS11, &dft(&srv, "1.1.0", "sf:ComplexGeoFeature").await);
    let xml = srv
        .get(&format!(
            "/wfs?service=WFS&version=2.0.0&request=GetFeature&{}",
            q.replace("typename", "typeNames")
                .replace("featureid", "resourceId")
        ))
        .await
        .xml();
    let ls32 = "//sf:ComplexGeoFeature/sf:geometryProperty/gml32:LineString";
    xml.assert(&format!("{ls32}[@srsDimension='3']"));
    assert_eq!(
        xml.string(&format!("{ls32}/gml32:posList")),
        "46.074 9.799 600.2 46.652 10.466 781.4"
    );
    // GML 2: x,y,z tuples
    let xml = srv
        .get(&format!(
            "/wfs?service=WFS&version=1.0.0&request=GetFeature&{q}"
        ))
        .await
        .xml();
    assert_eq!(
        xml.string("//sf:geometryProperty/gml:LineString/gml:coordinates"),
        "9.799,46.074,600.2 10.466,46.652,781.4"
    );
    // 2D features stay 2D
    let xml = srv.get("/wfs?service=WFS&version=1.1.0&request=GetFeature&typename=sf:PrimitiveGeoFeature&featureid=f001").await.xml();
    xml.assert_count("//@srsDimension", 0);
    // GeoJSON, CSV (WKT), KML
    let r = srv
        .get(&format!(
            "/wfs?service=WFS&version=1.1.0&request=GetFeature&{q}&outputFormat=application/json"
        ))
        .await;
    let json: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(
        json["features"][0]["geometry"]["coordinates"],
        serde_json::json!([[9.799, 46.074, 600.2], [10.466, 46.652, 781.4]])
    );
    let r = srv
        .get(&format!(
            "/wfs?service=WFS&version=1.1.0&request=GetFeature&{q}&outputFormat=csv"
        ))
        .await;
    assert!(
        r.body
            .contains("LINESTRING Z (9.799 46.074 600.2, 10.466 46.652 781.4)"),
        "{}",
        r.body
    );
    let r = srv.get(&format!("/wfs?service=WFS&version=1.1.0&request=GetFeature&{q}&outputFormat=application/vnd.google-earth.kml%2Bxml")).await;
    assert!(
        r.body
            .contains("<coordinates>9.799,46.074,600.2 10.466,46.652,781.4</coordinates>"),
        "{}",
        r.body
    );
}
