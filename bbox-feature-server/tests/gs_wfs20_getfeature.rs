//! GeoServer WFS 2.0 GetFeature family test behaviours (GetFeatureTest, GetFeatureJoinTest,
//! GetFeaturePagingTest, StoredQueryTest, GetPropertyValueTest, ExtendedOperatorTest), re-implemented
//! against the GeoServer default test catalog (tests/common/geoserver.rs).
//!
//! Not ported (reasons):
//! - GetFeatureTest: testSkipNumberMatched, testResultTypeHitsNumberMatched, testNumReturnedMatchedWithMaxFeatures,
//!   testPostWithBoundsEnabled/Disabled, testAfterFeatureTypeAdded, testLayerQualified, testNumberOfDecimals,
//!   testCustomizeFeatureType, testPostWithMisconfiguredTypeAndSkip, testGMLAttributeMapping(override=true part)
//!   (GeoServer catalog / WFSInfo settings); testGetIAULayer (IAU planetary CRS, layer not in catalog);
//!   testGetWithIdentifier (sf:PrimitiveGeoFeatureId), testWithGMLProperties (sf:WithGMLProperties): layers not in
//!   the default catalog; testSOAPWithEntity (security); testGml32MimeType mimeTypeToForce part (GS setting).
//! - GetFeatureJoinTest: uses GeoServer-only PostGIS layers gs:Forests/gs:Lakes (with extra rows), gs:TimeFeature,
//!   gs:t1..t3. Ported against cite:Forests/cite:Lakes (one feature each) with adjusted expectations; not ported:
//!   testSpatialJoinGETWorkspaceQualifier (virtual service), testStandardJoinMainTypeRenamed (catalog rename),
//!   testStandardJoinThreeWays*, testTemporalJoin, testSelfJoin*/LocalNamespaces (need several features per type;
//!   self join covered by testSelfJoinNoAliases), CSV join tests other than the two-type ones.
//! - GetFeaturePagingTest: gs:Fifteen/gs:Seven GeoPackage copies with `num` attribute not in catalog; ported on
//!   cdf:Fifteen/cdf:Seven only; testSortingGET (needs `num`) and testNextPreviousSkipNumberMatchedGET (GS setting) not ported.
//! - StoredQueryTest: testCreateStoredQueryXXE (security), testCreateLocalStoredQuery, testDisallowGlobalQueries,
//!   testDisallowPerWorkspaceQueries, testDisabledStoredQueriesManagement (workspace stored queries / GS settings).
//! - GetFeatureCurvesTest, BoundingBox3DTest, MultiDimensionTest: layers (cite:curve*, sf:With3D, gs:tasmania_roads)
//!   are not in the default catalog.

mod common;
use common::*;

const CITE: &str = "http://www.opengis.net/cite";
const WFS2_XSD: &str = "http://schemas.opengis.net/wfs/2.0/wfs.xsd";

/// XPath step for a cite: element (prefix not registered in the XPath context)
fn cite(name: &str) -> String {
    format!("*[local-name()='{name}' and namespace-uri()='{CITE}']")
}

fn enc(s: &str) -> String {
    serde_urlencoded::to_string([("x", s)]).unwrap()[2..].to_string()
}

async fn srv() -> TestServer {
    geoserver::server().await
}

/// WFS 2.0 FeatureCollection with schemaLocation pointing at the WFS 2.0 schema
#[track_caller]
fn assert_gml32(r: &Response) -> Xml {
    r.assert_ok();
    let xml = r.xml();
    xml.assert("/wfs2:FeatureCollection");
    let loc = xml.string("/wfs2:FeatureCollection/@xsi:schemaLocation");
    let parts: Vec<&str> = loc.split_whitespace().collect();
    let i = parts
        .iter()
        .position(|p| *p == "http://www.opengis.net/wfs/2.0")
        .unwrap_or_else(|| panic!("no WFS 2.0 schemaLocation: {loc}"));
    assert!(
        parts
            .get(i + 1)
            .map(|l| l.ends_with("2.0/wfs.xsd"))
            .unwrap_or(false),
        "{loc}"
    );
    xml
}

/// All 15 cdf:Fifteen features, each with gml:id
#[track_caller]
fn assert_fifteen_all(r: &Response) {
    let xml = assert_gml32(r);
    xml.assert_count("//cdf:Fifteen", 15);
    xml.assert_count("//cdf:Fifteen[@gml32:id]", 15);
}

/// OWS 1.1 exception report of WFS 2.0 with code and (case-insensitive) locator
#[track_caller]
fn assert_ex(r: &Response, code: &str, locator: Option<&str>) {
    let xml = r.xml();
    xml.assert("/ows11:ExceptionReport[@version='2.0.0']");
    assert_eq!(
        xml.string("//ows11:Exception/@exceptionCode"),
        code,
        "{}",
        r.body
    );
    if let Some(l) = locator {
        assert_eq!(
            xml.string("//ows11:Exception/@locator").to_lowercase(),
            l.to_lowercase(),
            "{}",
            r.body
        );
    }
}

/// Query parameters of a next/previous link (keys lower case)
fn link_params(href: &str) -> Vec<(String, String)> {
    let q = href.split_once('?').map(|(_, q)| q).unwrap_or("");
    serde_urlencoded::from_str::<Vec<(String, String)>>(q)
        .unwrap()
        .into_iter()
        .map(|(k, v)| (k.to_lowercase(), v))
        .collect()
}

fn param<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

// ------------------------------------------------------------------ GetFeatureTest

const GF: &str = "/wfs?request=GetFeature&version=2.0.0&service=wfs";

/// GeoServer GetFeatureTest.testGML32OutputFormatWithV110
#[actix_web::test]
async fn gs_get_feature_gml32_output_format_with_v110() {
    let r = srv().await.get("/wfs?request=getfeature&typename=cdf:Fifteen&version=1.1.0&service=wfs&outputFormat=gml32").await;
    assert_fifteen_all(&r);
}

/// GeoServer GetFeatureTest.testGML32OutputFormatWithV200
#[actix_web::test]
async fn gs_get_feature_gml32_output_format_with_v200() {
    let r = srv().await.get("/wfs?request=getfeature&typename=cdf:Fifteen&version=2.0.0&service=wfs&outputFormat=gml32").await;
    assert_fifteen_all(&r);
}

/// GeoServer GetFeatureTest.testGet
#[actix_web::test]
async fn gs_get_feature_get() {
    let s = srv().await;
    assert_fifteen_all(&s.get(&format!("{GF}&typenames=cdf:Fifteen")).await);
    assert_fifteen_all(&s.get(&format!("{GF}&typenames=(cdf:Fifteen)")).await);
}

/// GeoServer GetFeatureTest.testAlternatePrefix
#[actix_web::test]
async fn gs_get_feature_alternate_prefix() {
    let s = srv().await;
    for q in [
        "typenames=abc:Fifteen&namespaces=xmlns(abc,http://www.opengis.net/cite/data)",
        "typenames=abc:Fifteen&namespaces=xmlns(abc,http://www.opengis.net/cite/data),xmlns(wfs,http://www.opengis.net/wfs/2.0)",
        "typenames=Fifteen&namespaces=xmlns(http://www.opengis.net/cite/data),xmlns(wfs,http://www.opengis.net/wfs/2.0)",
    ] {
        let r = s.get(&format!("{GF}&{q}")).await;
        assert_fifteen_all(&r);
    }
}

/// GeoServer GetFeatureTest.testConcurrentGet
#[actix_web::test]
async fn gs_get_feature_concurrent_get() {
    let s = srv().await;
    let path = format!("{GF}&typenames=cdf:Fifteen");
    let rs = futures::future::join_all((0..50).map(|_| s.get(&path))).await;
    for r in &rs {
        assert_fifteen_all(r);
    }
}

const POST_OTHER: &str = "<wfs:GetFeature service='WFS' version='2.0.0' xmlns:cdf='http://www.opengis.net/cite/data' xmlns:wfs='http://www.opengis.net/wfs/2.0'><wfs:Query typeNames='cdf:Other'><wfs:PropertyName>cdf:string2</wfs:PropertyName></wfs:Query></wfs:GetFeature>";

#[track_caller]
fn assert_other(r: &Response) {
    let xml = assert_gml32(r);
    assert!(xml.count("//cdf:Other") > 0, "{}", r.body);
    assert_eq!(
        xml.count("//cdf:Other"),
        xml.count("//cdf:Other[@gml32:id]")
    );
}

/// GeoServer GetFeatureTest.testConcurrentPost
#[actix_web::test]
async fn gs_get_feature_concurrent_post() {
    let s = srv().await;
    let rs = futures::future::join_all((0..50).map(|_| s.post_wfs(POST_OTHER))).await;
    for r in &rs {
        assert_other(r);
    }
}

/// GeoServer GetFeatureTest.testGetTypeNames
#[actix_web::test]
async fn gs_get_feature_get_type_names() {
    let xml = srv()
        .await
        .get(&format!("{GF}&typenames=(cdf:Fifteen)(cdf:Seven)"))
        .await
        .xml();
    xml.assert_count("//cdf:Fifteen", 15);
    xml.assert_count("//cdf:Seven", 7);
}

/// GeoServer GetFeatureTest.testGetTypeName
#[actix_web::test]
async fn gs_get_feature_get_type_name() {
    assert_fifteen_all(&srv().await.get(&format!("{GF}&typename=cdf:Fifteen")).await);
}

#[track_caller]
fn assert_counts(r: &Response, features: usize, returned: &str, matched: &str) {
    let xml = assert_gml32(r);
    xml.assert_count("//cdf:Fifteen", features);
    assert_eq!(
        xml.string("/wfs2:FeatureCollection/@numberReturned"),
        returned
    );
    assert_eq!(
        xml.string("/wfs2:FeatureCollection/@numberMatched"),
        matched
    );
}

/// GeoServer GetFeatureTest.testGetWithCount
#[actix_web::test]
async fn gs_get_feature_get_with_count() {
    assert_counts(
        &srv()
            .await
            .get(&format!("{GF}&typenames=cdf:Fifteen&count=5"))
            .await,
        5,
        "5",
        "15",
    );
}

/// GeoServer GetFeatureTest.testGetWithCountAndStartIndex0
#[actix_web::test]
async fn gs_get_feature_get_with_count_and_start_index0() {
    assert_counts(
        &srv()
            .await
            .get(&format!("{GF}&typenames=cdf:Fifteen&count=5&startIndex=0"))
            .await,
        5,
        "5",
        "15",
    );
}

/// GeoServer GetFeatureTest.testGetWithCountAndStartIndexMiddle
#[actix_web::test]
async fn gs_get_feature_get_with_count_and_start_index_middle() {
    assert_counts(
        &srv()
            .await
            .get(&format!("{GF}&typenames=cdf:Fifteen&count=5&startIndex=7"))
            .await,
        5,
        "5",
        "15",
    );
}

/// GeoServer GetFeatureTest.testGetWithCountAndStartIndexEnd
#[actix_web::test]
async fn gs_get_feature_get_with_count_and_start_index_end() {
    assert_counts(
        &srv()
            .await
            .get(&format!("{GF}&typenames=cdf:Fifteen&count=5&startIndex=11"))
            .await,
        4,
        "4",
        "15",
    );
}

/// GeoServer GetFeatureTest.testGetPropertyNameEmpty
#[actix_web::test]
async fn gs_get_feature_get_property_name_empty() {
    assert_fifteen_all(
        &srv()
            .await
            .get(&format!("{GF}&typename=cdf:Fifteen&propertyname="))
            .await,
    );
}

/// GeoServer GetFeatureTest.testGetPropertyNameStar
#[actix_web::test]
async fn gs_get_feature_get_property_name_star() {
    assert_fifteen_all(
        &srv()
            .await
            .get(&format!("{GF}&typename=cdf:Fifteen&propertyname=*"))
            .await,
    );
}

/// GeoServer GetFeatureTest.testGetPropertyNameOneValueServiceNotSet
#[actix_web::test]
async fn gs_get_feature_get_property_name_one_value_service_not_set() {
    let s = srv().await;
    for path in ["/wfs", "/wfs/"] {
        let r = s
            .get(&format!(
                "{path}?request=GetFeature&typename=cite:Ponds&version=2.0.0&cql_filter={}&propertyname=TYPE",
                enc("TYPE='Stock Pond'")
            ))
            .await;
        r.assert_ok();
        let xml = r.xml();
        let ponds = format!("//{}", cite("Ponds"));
        let n = xml.count(&ponds);
        assert!(n > 0, "{path}: {}", r.body);
        xml.assert_count(&format!("{ponds}/{}", cite("TYPE")), n);
        xml.assert_count(&format!("{ponds}/{}", cite("NAME")), 0);
    }
}

/// GeoServer GetFeatureTest.testGetWithFeatureId
#[actix_web::test]
async fn gs_get_feature_get_with_feature_id() {
    let s = srv().await;
    let r = s
        .get(&format!("{GF}&typeName=cdf:Fifteen&featureid=Fifteen.2"))
        .await;
    let xml = assert_gml32(&r);
    xml.assert_count("//wfs2:FeatureCollection/wfs2:member/cdf:Fifteen", 1);
    xml.assert("//wfs2:FeatureCollection/wfs2:member/cdf:Fifteen/@gml32:id = 'Fifteen.2'");
    let xml = s
        .get(&format!(
            "{GF}&typeName=cite:NamedPlaces&featureId=NamedPlaces.1107531895891"
        ))
        .await
        .xml();
    xml.assert("/wfs2:FeatureCollection");
    let m = format!(
        "//wfs2:FeatureCollection/wfs2:member/{}",
        cite("NamedPlaces")
    );
    xml.assert_count(&m, 1);
    xml.assert(&format!("{m}/@gml32:id = 'NamedPlaces.1107531895891'"));
}

/// GeoServer GetFeatureTest.testGetWithResourceId
#[actix_web::test]
async fn gs_get_feature_get_with_resource_id() {
    let s = srv().await;
    let r = s
        .get(&format!("{GF}&typeNames=cdf:Fifteen&resourceid=Fifteen.2"))
        .await;
    let xml = assert_gml32(&r);
    xml.assert_count("//wfs2:FeatureCollection/wfs2:member/cdf:Fifteen", 1);
    xml.assert("//wfs2:member/cdf:Fifteen/@gml32:id = 'Fifteen.2'");
    let xml = s
        .get(&format!(
            "{GF}&typeName=cite:NamedPlaces&resourceid=NamedPlaces.1107531895891"
        ))
        .await
        .xml();
    let m = format!(
        "//wfs2:FeatureCollection/wfs2:member/{}",
        cite("NamedPlaces")
    );
    xml.assert_count(&m, 1);
    xml.assert(&format!("{m}/@gml32:id = 'NamedPlaces.1107531895891'"));
}

/// GeoServer GetFeatureTest.testGetWithInconsistentResourceId (cite compliant mode)
#[actix_web::test]
async fn gs_get_feature_get_with_inconsistent_resource_id() {
    let r = srv()
        .await
        .get(&format!(
            "{GF}&typeNames=sf:AggregateGeoFeature&resourceid=Fifteen.2"
        ))
        .await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert_ex(&r, "InvalidParameterValue", Some("RESOURCEID"));
}

/// GeoServer GetFeatureTest.testGetWithConsistentResourceId (cite compliant mode)
#[actix_web::test]
async fn gs_get_feature_get_with_consistent_resource_id() {
    let r = srv()
        .await
        .get(&format!("{GF}&typeNames=cdf:Fifteen&resourceid=Fifteen.2"))
        .await;
    r.assert_ok();
    let xml = r.xml();
    xml.assert_count("//wfs2:member/cdf:Fifteen", 1);
    xml.assert("//wfs2:member/cdf:Fifteen/@gml32:id = 'Fifteen.2'");
}

/// GeoServer GetFeatureTest.testGetWithBBOX
#[actix_web::test]
async fn gs_get_feature_get_with_bbox() {
    let xml = srv()
        .await
        .get("/wfs?request=GetFeature&version=2.0.0&typeName=sf:PrimitiveGeoFeature&BBOX=57.0,-4.5,62.0,1.0,EPSG:4326")
        .await
        .xml();
    xml.assert_count("//sf:PrimitiveGeoFeature", 1);
    xml.assert("//sf:PrimitiveGeoFeature/gml32:name[text()='name-f002']");
}

/// GeoServer GetFeatureTest.testGetWithFilter
#[actix_web::test]
async fn gs_get_feature_get_with_filter() {
    let filter = r#"<fes:Filter xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:fes="http://www.opengis.net/fes/2.0"><fes:BBOX><gml:Envelope srsName="EPSG:4326"><gml:lowerCorner>57.0 -4.5</gml:lowerCorner><gml:upperCorner>62.0 1.0</gml:upperCorner></gml:Envelope></fes:BBOX></fes:Filter>"#;
    let xml = srv()
        .await
        .get(&format!(
            "/wfs?request=GetFeature&version=2.0.0&typeName=sf:PrimitiveGeoFeature&FILTER={}",
            enc(filter)
        ))
        .await
        .xml();
    xml.assert_count("//sf:PrimitiveGeoFeature", 1);
    xml.assert("//sf:PrimitiveGeoFeature/gml32:name[text()='name-f002']");
}

/// GeoServer GetFeatureTest.testPost
#[actix_web::test]
async fn gs_get_feature_post() {
    assert_other(&srv().await.post_wfs(POST_OTHER).await);
}

/// GeoServer GetFeatureTest.testPostMultipleQueriesDifferentNamespaces
#[actix_web::test]
async fn gs_get_feature_post_multiple_queries_different_namespaces() {
    let body = "<wfs:GetFeature service='WFS' version='2.0.0' xmlns:cdf='http://www.opengis.net/cite/data' xmlns:wfs='http://www.opengis.net/wfs/2.0'><wfs:Query typeNames='cdf:Other'/><wfs:Query typeNames='sf:PrimitiveGeoFeature'/></wfs:GetFeature>";
    assert_gml32(&srv().await.post_wfs(body).await);
}

/// GeoServer GetFeatureTest.testPostFormEncoded
#[actix_web::test]
async fn gs_get_feature_post_form_encoded() {
    let body = "service=WFS&version=2.0.0&request=GetFeature&typename=sf:PrimitiveGeoFeature&namespace=xmlns(sf%3Dhttp%3A%2F%2Fcite.opengeospatial.org%2Fgmlsf)";
    let r = srv()
        .await
        .post_with_type("/wfs", body, "application/x-www-form-urlencoded")
        .await;
    let xml = assert_gml32(&r);
    xml.assert_count("//sf:PrimitiveGeoFeature", 5);
}

/// GeoServer GetFeatureTest.testPostWithFilter
#[actix_web::test]
async fn gs_get_feature_post_with_filter() {
    let body = "<wfs:GetFeature service='WFS' version='2.0.0' outputFormat='text/xml; subtype=gml/3.2' xmlns:cdf='http://www.opengis.net/cite/data' xmlns:fes='http://www.opengis.net/fes/2.0' xmlns:wfs='http://www.opengis.net/wfs/2.0'><wfs:Query typeNames='cdf:Other'><fes:Filter><fes:PropertyIsEqualTo><fes:ValueReference>cdf:integers</fes:ValueReference><fes:Literal>7</fes:Literal></fes:PropertyIsEqualTo></fes:Filter></wfs:Query></wfs:GetFeature>";
    assert_other(&srv().await.post_wfs(body).await);
}

fn bbox_post(value_ref: &str, srs: &str, lower: &str, upper: &str, elem: &str) -> String {
    format!(
        "<wfs:GetFeature service='WFS' version='2.0.0' xmlns:sf='http://cite.opengeospatial.org/gmlsf' xmlns:fes='http://www.opengis.net/fes/2.0' xmlns:gml='http://www.opengis.net/gml/3.2' xmlns:wfs='http://www.opengis.net/wfs/2.0'><wfs:Query typeNames='sf:PrimitiveGeoFeature'><fes:Filter><fes:BBOX><fes:{elem}>{value_ref}</fes:{elem}><gml:Envelope srsName='{srs}'><gml:lowerCorner>{lower}</gml:lowerCorner><gml:upperCorner>{upper}</gml:upperCorner></gml:Envelope></fes:BBOX></fes:Filter></wfs:Query></wfs:GetFeature>"
    )
}

/// GeoServer GetFeatureTest.testPostWithBboxFilter
#[actix_web::test]
async fn gs_get_feature_post_with_bbox_filter() {
    let r = srv()
        .await
        .post_wfs(&bbox_post(
            "pointProperty",
            "EPSG:4326",
            "57.0 -4.5",
            "62.0 1.0",
            "ValueReference",
        ))
        .await;
    r.xml().assert_count("//sf:PrimitiveGeoFeature", 1);
}

/// GeoServer GetFeatureTest.testPostWithBboxFilterOnBoundedBy
#[actix_web::test]
async fn gs_get_feature_post_with_bbox_filter_on_bounded_by() {
    let r = srv()
        .await
        .post_wfs(&bbox_post(
            "gml:boundedBy",
            "EPSG:4326",
            "57.0 -4.5",
            "62.0 1.0",
            "ValueReference",
        ))
        .await;
    r.xml().assert_count("//sf:PrimitiveGeoFeature", 1);
}

/// GeoServer GetFeatureTest.testPostWithLessThanOnBoundedBy (cite compliant mode)
#[actix_web::test]
async fn gs_get_feature_post_with_less_than_on_bounded_by() {
    let body = r#"<wfs:GetFeature service="WFS" version="2.0.0" xmlns:sf="http://cite.opengeospatial.org/gmlsf" xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:wfs="http://www.opengis.net/wfs/2.0"><wfs:Query typeNames="sf:PrimitiveGeoFeature"><fes:Filter><fes:PropertyIsLessThanOrEqualTo matchAction="Any" matchCase="true"><fes:Literal><gml:Envelope srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>-90 -180</gml:lowerCorner><gml:upperCorner>90 180</gml:upperCorner></gml:Envelope></fes:Literal><fes:ValueReference>gml:boundedBy</fes:ValueReference></fes:PropertyIsLessThanOrEqualTo></fes:Filter></wfs:Query></wfs:GetFeature>"#;
    let r = srv().await.post_wfs(body).await;
    // accepted difference: bbox uses HTTP 403 for OperationProcessingFailed (WFS 2.0 Table 3)
    // and locates the error at the filter; GeoServer answers 500 with locator GetFeature
    assert!(r.status == 500 || r.status == 403, "{}", r.body);
    assert_ex(&r, "OperationProcessingFailed", None);
}

/// GeoServer GetFeatureTest.testPostWithFailingUrnBboxFilter
#[actix_web::test]
async fn gs_get_feature_post_with_failing_urn_bbox_filter() {
    let r = srv()
        .await
        .post_wfs(&bbox_post(
            "pointProperty",
            "urn:ogc:def:crs:EPSG:6.11.2:4326",
            "57.0 -4.5",
            "62.0 1.0",
            "PropertyName",
        ))
        .await;
    r.assert_ok();
    r.xml().assert_count("//sf:PrimitiveGeoFeature", 0);
}

/// GeoServer GetFeatureTest.testPostWithMatchingUrnBboxFilter
#[actix_web::test]
async fn gs_get_feature_post_with_matching_urn_bbox_filter() {
    let r = srv()
        .await
        .post_wfs(&bbox_post(
            "pointProperty",
            "urn:ogc:def:crs:EPSG:6.11.2:4326",
            "-4.5 57.0",
            "1.0 62.0",
            "PropertyName",
        ))
        .await;
    r.xml().assert_count("//sf:PrimitiveGeoFeature", 1);
}

/// GeoServer GetFeatureTest.testPostWithFunctionFilter
#[actix_web::test]
async fn gs_get_feature_post_with_function_filter() {
    let body = "<wfs:GetFeature service='WFS' version='2.0.0' xmlns:cdf='http://www.opengis.net/cite/data' xmlns:fes='http://www.opengis.net/fes/2.0' xmlns:wfs='http://www.opengis.net/wfs/2.0'><wfs:Query typeNames='cdf:Other'><fes:Filter><fes:PropertyIsLessThan><fes:Function name='random'></fes:Function><fes:Literal>0.5</fes:Literal></fes:PropertyIsLessThan></fes:Filter></wfs:Query></wfs:GetFeature>";
    assert_gml32(&srv().await.post_wfs(body).await);
}

/// GeoServer GetFeatureTest.testResultTypeHitsGet
#[actix_web::test]
async fn gs_get_feature_result_type_hits_get() {
    let r = srv().await.get("/wfs?request=GetFeature&typename=cdf:Fifteen&version=2.0.0&resultType=hits&service=wfs").await;
    let xml = assert_gml32(&r);
    xml.assert_count("//cdf:Fifteen", 0);
    xml.assert("/wfs2:FeatureCollection[@numberMatched='15']");
}

/// GeoServer GetFeatureTest.testResultTypeHitsGetWithCount
#[actix_web::test]
async fn gs_get_feature_result_type_hits_get_with_count() {
    let r = srv().await.get("/wfs?request=GetFeature&typename=cdf:Fifteen&version=2.0.0&resultType=hits&service=wfs&count=2").await;
    let xml = assert_gml32(&r);
    xml.assert_count("//cdf:Fifteen", 0);
    xml.assert("/wfs2:FeatureCollection[@numberMatched='15']");
    let next = xml.string("/wfs2:FeatureCollection/@next");
    assert!(!next.is_empty(), "no next link: {}", r.body);
    let p = link_params(&next);
    assert_eq!(
        param(&p, "resulttype").map(str::to_lowercase).as_deref(),
        Some("results"),
        "{next}"
    );
    assert_eq!(param(&p, "count"), Some("2"), "{next}");
    assert_eq!(param(&p, "startindex"), Some("0"), "{next}");
}

const HITS_SEVEN: &str = "<wfs:GetFeature service='WFS' version='2.0.0' resultType='hits' xmlns:cdf='http://www.opengis.net/cite/data' xmlns:wfs='http://www.opengis.net/wfs/2.0'><wfs:Query typeNames='cdf:Seven'/></wfs:GetFeature>";

/// GeoServer GetFeatureTest.testResultTypeHitsPost
#[actix_web::test]
async fn gs_get_feature_result_type_hits_post() {
    let xml = assert_gml32(&srv().await.post_wfs(HITS_SEVEN).await);
    xml.assert_count("//cdf:Seven", 0);
    xml.assert("/wfs2:FeatureCollection[@numberMatched='7']");
}

/// GeoServer GetFeatureTest.testResultTypeHitsNumReturnedMatched
#[actix_web::test]
async fn gs_get_feature_result_type_hits_num_returned_matched() {
    let xml = assert_gml32(&srv().await.post_wfs(HITS_SEVEN).await);
    xml.assert("/wfs2:FeatureCollection[@numberMatched='7' and @numberReturned='0']");
}

/// GeoServer GetFeatureTest.testWithSRS
#[actix_web::test]
async fn gs_get_feature_with_srs() {
    let body = "<wfs:GetFeature service='WFS' version='2.0.0' xmlns:wfs='http://www.opengis.net/wfs/2.0'><wfs:Query xmlns:cdf='http://www.opengis.net/cite/data' typeNames='cdf:Other' srsName='urn:ogc:def:crs:EPSG:4326'/></wfs:GetFeature>";
    let r = srv().await.post_wfs(body).await;
    r.xml().assert_count("//cdf:Other", 1);
}

/// GeoServer GetFeatureTest.testWithSillyLiteral
#[actix_web::test]
async fn gs_get_feature_with_silly_literal() {
    let body = "<wfs:GetFeature service='WFS' version='2.0.0' xmlns:wfs='http://www.opengis.net/wfs/2.0' xmlns:fes='http://www.opengis.net/fes/2.0'><wfs:Query xmlns:cdf='http://www.opengis.net/cite/data' typeNames='cdf:Other' srsName='urn:ogc:def:crs:EPSG:4326'><fes:Filter><fes:PropertyIsEqualTo><fes:ValueReference>description</fes:ValueReference><fes:Literal><wfs:Native vendorId=\"foo\" safeToIgnore=\"true\"/></fes:Literal></fes:PropertyIsEqualTo></fes:Filter></wfs:Query></wfs:GetFeature>";
    let r = srv().await.post_wfs(body).await;
    r.assert_ok();
    r.xml().assert_count("//cdf:Other", 0);
}

/// GeoServer GetFeatureTest.testUserSuppliedNamespacePrefix
#[actix_web::test]
async fn gs_get_feature_user_supplied_namespace_prefix() {
    let r = srv()
        .await
        .get(&format!("{GF}&typename=myPrefix:Fifteen&namespaces=xmlns(myPrefix,http%3A%2F%2Fwww.opengis.net%2Fcite%2Fdata)"))
        .await;
    assert_fifteen_all(&r);
}

/// GeoServer GetFeatureTest.testUserSuppliedDefaultNamespace
#[actix_web::test]
async fn gs_get_feature_user_supplied_default_namespace() {
    let r = srv()
        .await
        .get(&format!(
            "{GF}&typename=Fifteen&namespace=xmlns(http%3A%2F%2Fwww.opengis.net%2Fcite%2Fdata)"
        ))
        .await;
    assert_fifteen_all(&r);
}

/// GeoServer GetFeatureTest.testGML32OutputFormatAlternate
#[actix_web::test]
async fn gs_get_feature_gml32_output_format_alternate() {
    let r = srv()
        .await
        .get(&format!(
            "{GF}&typename=cdf:Fifteen&outputFormat=application/gml%2Bxml;%20version%3D3.2"
        ))
        .await;
    assert_fifteen_all(&r);
}

/// GeoServer GetFeatureTest.testGMLAttributeMapping (default: description/name attributes as gml:description/gml:name)
#[actix_web::test]
async fn gs_get_feature_gml_attribute_mapping() {
    let xml = srv()
        .await
        .get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typename=sf:PrimitiveGeoFeature")
        .await
        .xml();
    xml.assert("//gml32:name");
    xml.assert("//gml32:description");
    xml.assert_not("//sf:name");
    xml.assert_not("//sf:description");
}

const CREATE_OTHER_QUERY: &str = "<wfs:CreateStoredQuery service='WFS' version='2.0.0' xmlns:wfs='http://www.opengis.net/wfs/2.0' xmlns:fes='http://www.opengis.net/fes/2.0' xmlns:gml='http://www.opengis.net/gml/3.2' xmlns:myns='http://www.someserver.com/myns' xmlns:sf='http://cite.opengeospatial.org/gmlsf' xmlns:cdf='http://www.opengis.net/cite/data'><wfs:StoredQueryDefinition id='myStoredQuery'><wfs:Parameter name='integers' type='xs:integer'/><wfs:QueryExpressionText returnFeatureTypes='cdf:Other' language='urn:ogc:def:queryLanguage:OGC-WFS::WFS_QueryExpression' isPrivate='false'><wfs:Query typeNames=\"cdf:Other\"><fes:Filter><fes:PropertyIsEqualTo><fes:ValueReference>cdf:integers</fes:ValueReference>${integers}</fes:PropertyIsEqualTo></fes:Filter></wfs:Query></wfs:QueryExpressionText></wfs:StoredQueryDefinition></wfs:CreateStoredQuery>";

/// GeoServer GetFeatureTest.testStoredQuery
#[actix_web::test]
async fn gs_get_feature_stored_query() {
    let s = srv().await;
    let r = s.post_wfs(CREATE_OTHER_QUERY).await;
    r.xml().assert("/wfs2:CreateStoredQueryResponse");
    let body = "<wfs:GetFeature service='WFS' version='2.0.0' xmlns:wfs='http://www.opengis.net/wfs/2.0' xmlns:fes='http://www.opengis.net/fes/2.0'><wfs:StoredQuery id='myStoredQuery'><wfs:Parameter name='integers'><fes:Literal>7</fes:Literal></wfs:Parameter></wfs:StoredQuery></wfs:GetFeature>";
    let xml = assert_gml32(&s.post_wfs(body).await);
    xml.assert_count("//cdf:Other", 1);
    xml.assert("//cdf:Other/cdf:integers[text()='7']");
}

/// GeoServer GetFeatureTest.testDefaultStoredQueryGet
#[actix_web::test]
async fn gs_get_feature_default_stored_query_get() {
    let r = srv()
        .await
        .get("/wfs?request=GetFeature&version=2.0.0&storedQueryId=urn:ogc:def:query:OGC-WFS::GetFeatureById&ID=PrimitiveGeoFeature.f001")
        .await;
    let xml = r.xml();
    xml.assert_not("//wfs2:FeatureCollection");
    xml.assert_count("/sf:PrimitiveGeoFeature", 1);
    xml.assert("/sf:PrimitiveGeoFeature[@gml32:id='PrimitiveGeoFeature.f001']");
}

/// GeoServer GetFeatureTest.testUnknownStoredQuery
#[actix_web::test]
async fn gs_get_feature_unknown_stored_query() {
    let r = srv()
        .await
        .get("/wfs?request=GetFeature&version=2.0.0&storedQueryId=foobar")
        .await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert_ex(&r, "InvalidParameterValue", Some("STOREDQUERY_ID"));
}

/// GeoServer GetFeatureTest.testDefaultStoredQueryPost
#[actix_web::test]
async fn gs_get_feature_default_stored_query_post() {
    let s = srv().await;
    let body = r#"<?xml version="1.0" encoding="UTF-8"?><wfs:GetFeature xmlns:wfs="http://www.opengis.net/wfs/2.0" service="WFS" startIndex="0" version="2.0.0"><wfs:StoredQuery id="urn:ogc:def:query:OGC-WFS::GetFeatureById"><wfs:Parameter name="id">PrimitiveGeoFeature.f001</wfs:Parameter></wfs:StoredQuery></wfs:GetFeature>"#;
    let xml = s.post_wfs(body).await.xml();
    xml.assert_count("//sf:PrimitiveGeoFeature", 1);
    xml.assert("//sf:PrimitiveGeoFeature[@gml32:id='PrimitiveGeoFeature.f001']");
    let xml = s
        .get("/wfs?request=GetFeature&version=2.0.0&storedQuery_Id=urn:ogc:def:query:OGC-WFS::GetFeatureById&ID=PrimitiveGeoFeature.f001")
        .await
        .xml();
    xml.assert("/sf:PrimitiveGeoFeature[@gml32:id='PrimitiveGeoFeature.f001']");
}

/// GeoServer GetFeatureTest.testStoredQueryBBOX
#[actix_web::test]
async fn gs_get_feature_stored_query_bbox() {
    let s = srv().await;
    let create = "<wfs:CreateStoredQuery service='WFS' version='2.0.0' xmlns:wfs='http://www.opengis.net/wfs/2.0' xmlns:fes='http://www.opengis.net/fes/2.0' xmlns:gml='http://www.opengis.net/gml/3.2' xmlns:sf='http://cite.opengeospatial.org/gmlsf'><wfs:StoredQueryDefinition id='myStoredBBOXQuery'><wfs:Parameter name='BBOX' type='gml:Envelope'/><wfs:QueryExpressionText returnFeatureTypes='sf:PrimitiveGeoFeature' language='urn:ogc:def:queryLanguage:OGC-WFS::WFS_QueryExpression' isPrivate='false'><wfs:Query typeNames='sf:PrimitiveGeoFeature'><fes:Filter><fes:BBOX><fes:ValueReference>pointProperty</fes:ValueReference> ${BBOX}</fes:BBOX></fes:Filter></wfs:Query></wfs:QueryExpressionText></wfs:StoredQueryDefinition></wfs:CreateStoredQuery>";
    s.post_wfs(create)
        .await
        .xml()
        .assert("/wfs2:CreateStoredQueryResponse");
    let body = "<wfs:GetFeature service='WFS' version='2.0.0' xmlns:wfs='http://www.opengis.net/wfs/2.0' xmlns:gml='http://www.opengis.net/gml/3.2'><wfs:StoredQuery id='myStoredBBOXQuery'><wfs:Parameter name='BBOX'><gml:Envelope srsName='EPSG:4326'><gml:lowerCorner>57.0 -4.5</gml:lowerCorner><gml:upperCorner>62.0 1.0</gml:upperCorner></gml:Envelope></wfs:Parameter></wfs:StoredQuery></wfs:GetFeature>";
    let xml = s.post_wfs(body).await.xml();
    xml.assert_count("//sf:PrimitiveGeoFeature", 1);
    xml.assert("//sf:PrimitiveGeoFeature/gml32:name = 'name-f002'");
}

/// GeoServer GetFeatureTest.testTemporalFilter
#[actix_web::test]
async fn gs_get_feature_temporal_filter() {
    let s = srv().await;
    // f008 dateTimeProperty `2006-06-27 22:08:00-07` is the instant 2006-06-28T05:08:00Z;
    // GeoServer drops the time of day (2006-06-27T00:00Z), so its Before bound is adjusted
    for (op, t) in [
        ("After", "2006-06-25T18:00:00-06:00"),
        ("Before", "2006-06-28T06:00:00Z"),
    ] {
        let body = format!("<wfs:GetFeature service='WFS' version='2.0.0' xmlns:sf='http://cite.opengeospatial.org/gmlsf' xmlns:fes='http://www.opengis.net/fes/2.0' xmlns:wfs='http://www.opengis.net/wfs/2.0'><wfs:Query typeNames='sf:PrimitiveGeoFeature'><fes:Filter><fes:{op}><fes:ValueReference>dateTimeProperty</fes:ValueReference><fes:Literal>{t}</fes:Literal></fes:{op}></fes:Filter></wfs:Query></wfs:GetFeature>");
        let r = s.post_wfs(&body).await;
        let xml = r.xml();
        xml.assert_count("//sf:PrimitiveGeoFeature", 1);
        xml.assert("//sf:PrimitiveGeoFeature/@gml32:id = 'PrimitiveGeoFeature.f008'");
    }
}

/// GeoServer GetFeatureTest.testGetFeatureInvalidPropertyName
#[actix_web::test]
async fn gs_get_feature_get_feature_invalid_property_name() {
    let r = srv().await.get("/wfs?version=2.0.0&service=wfs&request=GetFeature&typename=sf:PrimitiveGeoFeature&propertyName=foo").await;
    let xml = r.xml();
    xml.assert("/ows11:ExceptionReport");
    xml.assert("//ows11:Exception[@exceptionCode='InvalidParameterValue']");
}

/// GeoServer GetFeatureTest.testGetFeatureWithMultiplePropertyName
#[actix_web::test]
async fn gs_get_feature_get_feature_with_multiple_property_name() {
    let s = srv().await;
    for _ in 0..2 {
        let xml = s
            .get("/wfs?version=2.0.0&service=wfs&request=GetFeature&typename=cdf:Fifteen,cdf:Seven")
            .await
            .xml();
        xml.assert("/wfs2:FeatureCollection");
        xml.assert_count("/wfs2:FeatureCollection/wfs2:member", 2);
        xml.assert_count(
            "/wfs2:FeatureCollection/wfs2:member/wfs2:FeatureCollection",
            2,
        );
        xml.assert_count(
            "/wfs2:FeatureCollection/wfs2:member[1]/wfs2:FeatureCollection//cdf:Fifteen",
            15,
        );
        xml.assert_count(
            "/wfs2:FeatureCollection/wfs2:member[2]/wfs2:FeatureCollection//cdf:Seven",
            7,
        );
    }
}

fn soap(ns: &str, body: &str) -> String {
    format!("<soap:Envelope xmlns:soap='{ns}'><soap:Header/><soap:Body>{body}</soap:Body></soap:Envelope>")
}

/// GeoServer GetFeatureTest.testSOAP11
#[actix_web::test]
async fn gs_get_feature_soap11() {
    // GeoServer answers SOAP 1.1 with application/soap+xml; SOAP 1.1 itself uses text/xml (not asserted)
    let ns = "http://schemas.xmlsoap.org/soap/envelope/";
    let r = srv()
        .await
        .post_with_type("/wfs", &soap(ns, POST_OTHER), "application/soap+xml")
        .await;
    let doc = roxmltree::Document::parse(&r.body).unwrap();
    assert_eq!(
        doc.root_element().tag_name().namespace(),
        Some(ns),
        "{}",
        r.body
    );
}

/// GeoServer GetFeatureTest.testSOAP12
#[actix_web::test]
async fn gs_get_feature_soap12() {
    let ns = "http://www.w3.org/2003/05/soap-envelope";
    let r = srv()
        .await
        .post_with_type("/wfs", &soap(ns, POST_OTHER), "application/soap+xml")
        .await;
    assert!(
        r.content_type.starts_with("application/soap+xml"),
        "{}",
        r.content_type
    );
    let doc = roxmltree::Document::parse(&r.body).unwrap();
    assert_eq!(
        doc.root_element().tag_name().namespace(),
        Some(ns),
        "{}",
        r.body
    );
}

/// GeoServer GetFeatureTest.testBogusSrsName
#[actix_web::test]
async fn gs_get_feature_bogus_srs_name() {
    let s = srv().await;
    let body = "<wfs:GetFeature service='WFS' version='2.0.0' xmlns:wfs='http://www.opengis.net/wfs/2.0'><wfs:Query xmlns:cdf='http://www.opengis.net/cite/data' typeNames='cdf:Other' srsName='EPSG:XYZ'/></wfs:GetFeature>";
    assert_ex(
        &s.post_wfs(body).await,
        "InvalidParameterValue",
        Some("srsName"),
    );
    let r = s
        .get(
            "/wfs?service=WFS&version=2.0.0&request=getFeature&typeName=cdf:Other&srsName=EPSG:XYZ",
        )
        .await;
    assert_ex(&r, "InvalidParameterValue", Some("srsName"));
}

/// GeoServer GetFeatureTest.testQueryHandleInExceptionReport
#[actix_web::test]
async fn gs_get_feature_query_handle_in_exception_report() {
    let body = "<wfs:GetFeature service='WFS' version='2.0.0' xmlns:wfs='http://www.opengis.net/wfs/2.0' xmlns:fes='http://www.opengis.net/fes/2.0'><wfs:Query xmlns:cdf='http://www.opengis.net/cite/data' typeNames='cdf:Other' srsName='EPSG:XYZ' handle='myHandle'><fes:Filter><fes:PropertyIsEqualTo><fes:ValueReference>foobar</fes:ValueReference><fes:Literal>1</fes:Literal></fes:PropertyIsEqualTo></fes:Filter></wfs:Query></wfs:GetFeature>";
    let xml = srv().await.post_wfs(body).await.xml();
    xml.assert("//ows11:Exception/@locator = 'myHandle'");
}

/// GeoServer GetFeatureTest.testBogusTypeNames
#[actix_web::test]
async fn gs_get_feature_bogus_type_names() {
    let s = srv().await;
    let body = "<wfs:GetFeature service='WFS' version='2.0.0' xmlns:wfs='http://www.opengis.net/wfs/2.0'><wfs:Query typeNames='foobbar'/></wfs:GetFeature>";
    assert_ex(
        &s.post_wfs(body).await,
        "InvalidParameterValue",
        Some("typeName"),
    );
    let r = s
        .get("/wfs?service=WFS&version=2.0.0&request=getFeature&typeNames=foobar")
        .await;
    assert_ex(&r, "InvalidParameterValue", Some("typeName"));
}

/// GeoServer GetFeatureTest.testInvalidRequest
#[actix_web::test]
async fn gs_get_feature_invalid_request() {
    let body = "<wfs:GetFeature service='WFS' version='2.0.0' xmlns:wfs='http://www.opengis.net/wfs/2.0' xmlns:fes='http://www.opengis.net/fes/2.0'><wfs:Query typeNames='cdf:Other'><fes:Filter><fes:PropertyIsEqualTo></fes:foo></fes:Filter></wfs:Query></wfs:GetFeature>";
    assert_ex(
        &srv().await.post_wfs(body).await,
        "OperationParsingFailed",
        None,
    );
}

/// GeoServer GetFeatureTest.testWfs11AndGML32
#[actix_web::test]
async fn gs_get_feature_wfs11_and_gml32() {
    let r = srv()
        .await
        .get("/wfs?request=GetFeature&typeName=cdf:Fifteen&version=1.1.0&service=wfs&featureid=Fifteen.2&outputFormat=gml32")
        .await;
    let xml = assert_gml32(&r);
    xml.assert_count("//wfs2:member/cdf:Fifteen", 1);
    xml.assert("//wfs2:member/cdf:Fifteen/@gml32:id = 'Fifteen.2'");
}

/// GeoServer GetFeatureTest.testGml32MimeType (default content types)
#[actix_web::test]
async fn gs_get_feature_gml32_mime_type() {
    let s = srv().await;
    let r = s.get("/wfs?request=GetFeature&typeName=cdf:Fifteen&version=2.0&service=wfs&featureid=Fifteen.2&outputFormat=gml32").await;
    assert_eq!(r.content_type, "application/gml+xml; version=3.2");
    let r = s.post_wfs(POST_OTHER).await;
    assert_eq!(r.content_type, "application/gml+xml; version=3.2");
}

fn like_buildings(match_case: Option<&str>) -> String {
    let mc = match_case
        .map(|m| format!(" matchCase=\"{m}\""))
        .unwrap_or_default();
    format!(
        r#"<wfs:GetFeature service="WFS" version="2.0.0" xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:cite="http://www.opengis.net/cite"><wfs:Query typeNames="cite:Buildings"><fes:Filter><fes:PropertyIsLike wildCard="*" singleChar="%" escapeChar="!"{mc}><fes:ValueReference>cite:ADDRESS</fes:ValueReference><fes:Literal>* MAIN STREET</fes:Literal></fes:PropertyIsLike></fes:Filter></wfs:Query></wfs:GetFeature>"#
    )
}

/// GeoServer GetFeatureTest.testPropertyIsLikeWithoutMatchCase
#[actix_web::test]
async fn gs_get_feature_property_is_like_without_match_case() {
    let xml = srv().await.post_wfs(&like_buildings(None)).await.xml();
    xml.assert("/wfs2:FeatureCollection");
    xml.assert_count(&format!("//{}", cite("Buildings")), 0);
}

/// GeoServer GetFeatureTest.testPropertyIsLikeMatchCaseTrue
#[actix_web::test]
async fn gs_get_feature_property_is_like_match_case_true() {
    let xml = srv()
        .await
        .post_wfs(&like_buildings(Some("true")))
        .await
        .xml();
    xml.assert_count(&format!("//{}", cite("Buildings")), 0);
}

/// GeoServer GetFeatureTest.testPropertyIsLikeMatchCaseFalse
#[actix_web::test]
async fn gs_get_feature_property_is_like_match_case_false() {
    let xml = srv()
        .await
        .post_wfs(&like_buildings(Some("false")))
        .await
        .xml();
    xml.assert_count(&format!("//{}", cite("Buildings")), 2);
}

// ------------------------------------------------------------------ GetFeatureJoinTest
// Ported on cite:Forests (Green Forest, FID 109) and cite:Lakes (Blue Lake, FID 101); the forest
// polygon contains the lake, so spatial joins yield one tuple and FID equality joins none.

fn join_post(type_names: &str, aliases: Option<&str>, filter: &str, extra: &str) -> String {
    let aliases = aliases
        .map(|a| format!(" aliases='{a}'"))
        .unwrap_or_default();
    format!("<wfs:GetFeature xmlns:wfs='http://www.opengis.net/wfs/2.0' xmlns:fes='http://www.opengis.net/fes/2.0' xmlns:cite='http://www.opengis.net/cite' version='2.0.0'{extra}><wfs:Query typeNames='{type_names}'{aliases}><fes:Filter>{filter}</fes:Filter></wfs:Query></wfs:GetFeature>")
}

const INTERSECTS_AB: &str = "<fes:Intersects><fes:ValueReference>a/the_geom</fes:ValueReference><fes:ValueReference>b/the_geom</fes:ValueReference></fes:Intersects>";

/// One tuple of Green Forest and Blue Lake
#[track_caller]
fn assert_forest_lake_tuple(r: &Response) {
    let xml = r.xml();
    xml.assert_count("//wfs2:Tuple", 1);
    xml.assert(&format!(
        "//wfs2:Tuple[1]/wfs2:member/{}/{} = 'Green Forest'",
        cite("Forests"),
        cite("NAME")
    ));
    xml.assert(&format!(
        "//wfs2:Tuple[1]/wfs2:member/{}/{} = 'Blue Lake'",
        cite("Lakes"),
        cite("NAME")
    ));
}

/// GeoServer GetFeatureJoinTest.testSpatialJoinPOST
#[actix_web::test]
async fn gs_get_feature_join_spatial_join_post() {
    let r = srv()
        .await
        .post_wfs(&join_post(
            "cite:Forests cite:Lakes",
            Some("a b"),
            INTERSECTS_AB,
            "",
        ))
        .await;
    assert_forest_lake_tuple(&r);
}

/// GeoServer GetFeatureJoinTest.testSpatialJoinNoAliasesCustomPrefixes
#[actix_web::test]
async fn gs_get_feature_join_spatial_join_no_aliases_custom_prefixes() {
    let body = "<wfs:GetFeature xmlns:wfs='http://www.opengis.net/wfs/2.0' xmlns:fes='http://www.opengis.net/fes/2.0' xmlns:ns123='http://www.opengis.net/cite' version='2.0.0'><wfs:Query typeNames='ns123:Forests ns123:Lakes'><fes:Filter><fes:Intersects><fes:ValueReference>ns123:Forests/the_geom</fes:ValueReference><fes:ValueReference>ns123:Lakes/the_geom</fes:ValueReference></fes:Intersects></fes:Filter></wfs:Query></wfs:GetFeature>";
    assert_forest_lake_tuple(&srv().await.post_wfs(body).await);
}

/// GeoServer GetFeatureJoinTest.testSpatialJoinGET
#[actix_web::test]
async fn gs_get_feature_join_spatial_join_get() {
    let filter = "<Filter><Intersects><ValueReference>a/the_geom</ValueReference><ValueReference>b/the_geom</ValueReference></Intersects></Filter>";
    let r = srv()
        .await
        .get(&format!(
            "/wfs?service=WFS&version=2.0.0&request=getFeature&typenames=cite:Forests,cite:Lakes&aliases=a,b&filter={}",
            enc(filter)
        ))
        .await;
    assert_forest_lake_tuple(&r);
}

/// GeoServer GetFeatureJoinTest.testSpatialJoinPOSTWithPrimaryFilter
#[actix_web::test]
async fn gs_get_feature_join_spatial_join_post_with_primary_filter() {
    let s = srv().await;
    let f = |fid: &str| {
        format!("<fes:And>{INTERSECTS_AB}<PropertyIsEqualTo><ValueReference>a/FID</ValueReference><Literal>{fid}</Literal></PropertyIsEqualTo></fes:And>")
    };
    assert_forest_lake_tuple(
        &s.post_wfs(&join_post(
            "cite:Forests cite:Lakes",
            Some("a b"),
            &f("109"),
            "",
        ))
        .await,
    );
    let xml = s
        .post_wfs(&join_post(
            "cite:Forests cite:Lakes",
            Some("a b"),
            &f("110"),
            "",
        ))
        .await
        .xml();
    xml.assert_count("//wfs2:Tuple", 0);
}

/// GeoServer GetFeatureJoinTest.testSpatialJoinPOSTWithSecondaryFilter
#[actix_web::test]
async fn gs_get_feature_join_spatial_join_post_with_secondary_filter() {
    let f = format!("<fes:And>{INTERSECTS_AB}<PropertyIsEqualTo><ValueReference>b/FID</ValueReference><Literal>101</Literal></PropertyIsEqualTo></fes:And>");
    assert_forest_lake_tuple(
        &srv()
            .await
            .post_wfs(&join_post("cite:Forests cite:Lakes", Some("a b"), &f, ""))
            .await,
    );
}

/// GeoServer GetFeatureJoinTest.testSpatialJoinWithBothFilters
#[actix_web::test]
async fn gs_get_feature_join_spatial_join_with_both_filters() {
    let f = "<fes:And><fes:Intersects><fes:ValueReference>a/the_geom</fes:ValueReference><fes:ValueReference>b/the_geom</fes:ValueReference></fes:Intersects><fes:And><fes:PropertyIsEqualTo><fes:ValueReference>a/NAME</fes:ValueReference><fes:Literal>Green Forest</fes:Literal></fes:PropertyIsEqualTo><fes:PropertyIsGreaterThan><fes:ValueReference>b/FID</fes:ValueReference><fes:Literal>100</fes:Literal></fes:PropertyIsGreaterThan></fes:And></fes:And>";
    assert_forest_lake_tuple(
        &srv()
            .await
            .post_wfs(&join_post("cite:Forests cite:Lakes", Some("a b"), f, ""))
            .await,
    );
}

const FID_EQUAL: &str = "<PropertyIsEqualTo><ValueReference>a/FID</ValueReference><ValueReference>b/FID</ValueReference></PropertyIsEqualTo>";

/// GeoServer GetFeatureJoinTest.testStandardJoin (FIDs differ: no tuples)
#[actix_web::test]
async fn gs_get_feature_join_standard_join() {
    let s = srv().await;
    let xml = s
        .post_wfs(&join_post(
            "cite:Forests cite:Lakes",
            Some("a b"),
            FID_EQUAL,
            "",
        ))
        .await
        .xml();
    xml.assert("/wfs2:FeatureCollection");
    xml.assert_count("//wfs2:Tuple", 0);
    let ne = "<PropertyIsNotEqualTo><ValueReference>a/FID</ValueReference><ValueReference>b/FID</ValueReference></PropertyIsNotEqualTo>";
    assert_forest_lake_tuple(
        &s.post_wfs(&join_post("cite:Forests cite:Lakes", Some("a b"), ne, ""))
            .await,
    );
}

/// GeoServer GetFeatureJoinTest.testJoinAliasConflictProperty
#[actix_web::test]
async fn gs_get_feature_join_join_alias_conflict_property() {
    let f = "<PropertyIsNotEqualTo><ValueReference>a/FID</ValueReference><ValueReference>NAME/FID</ValueReference></PropertyIsNotEqualTo>";
    assert_forest_lake_tuple(
        &srv()
            .await
            .post_wfs(&join_post("cite:Forests cite:Lakes", Some("a NAME"), f, ""))
            .await,
    );
}

/// GeoServer GetFeatureJoinTest.testStandardJoin2
#[actix_web::test]
async fn gs_get_feature_join_standard_join2() {
    let f = "<PropertyIsNotEqualTo><ValueReference>c/FID</ValueReference><ValueReference>d/FID</ValueReference></PropertyIsNotEqualTo>";
    assert_forest_lake_tuple(
        &srv()
            .await
            .post_wfs(&join_post("cite:Forests cite:Lakes", Some("c d"), f, ""))
            .await,
    );
}

/// GeoServer GetFeatureJoinTest.testStandardJoinNoAliases
#[actix_web::test]
async fn gs_get_feature_join_standard_join_no_aliases() {
    let f = "<PropertyIsNotEqualTo><ValueReference>cite:Forests/FID</ValueReference><ValueReference>cite:Lakes/FID</ValueReference></PropertyIsNotEqualTo>";
    assert_forest_lake_tuple(
        &srv()
            .await
            .post_wfs(&join_post("cite:Forests cite:Lakes", None, f, ""))
            .await,
    );
}

/// GeoServer GetFeatureJoinTest.testStandardJoinLocalFilterNot
#[actix_web::test]
async fn gs_get_feature_join_standard_join_local_filter_not() {
    let f = "<And><PropertyIsNotEqualTo><ValueReference>c/FID</ValueReference><ValueReference>d/FID</ValueReference></PropertyIsNotEqualTo><Not><PropertyIsEqualTo><ValueReference>d/NAME</ValueReference><Literal>foo</Literal></PropertyIsEqualTo></Not></And>";
    assert_forest_lake_tuple(
        &srv()
            .await
            .post_wfs(&join_post("cite:Forests cite:Lakes", Some("c d"), f, ""))
            .await,
    );
}

/// GeoServer GetFeatureJoinTest.testStandardJoinLocalFilterOr
#[actix_web::test]
async fn gs_get_feature_join_standard_join_local_filter_or() {
    let f = "<And><PropertyIsNotEqualTo><ValueReference>c/FID</ValueReference><ValueReference>d/FID</ValueReference></PropertyIsNotEqualTo><Or><PropertyIsEqualTo><ValueReference>d/NAME</ValueReference><Literal>foo</Literal></PropertyIsEqualTo><PropertyIsEqualTo><ValueReference>d/NAME</ValueReference><Literal>Blue Lake</Literal></PropertyIsEqualTo></Or></And>";
    assert_forest_lake_tuple(
        &srv()
            .await
            .post_wfs(&join_post("cite:Forests cite:Lakes", Some("c d"), f, ""))
            .await,
    );
}

/// GeoServer GetFeatureJoinTest.testOredJoinCondition
#[actix_web::test]
async fn gs_get_feature_join_ored_join_condition() {
    let f = "<Or><PropertyIsEqualTo><ValueReference>c/FID</ValueReference><ValueReference>d/FID</ValueReference></PropertyIsEqualTo><And><PropertyIsEqualTo><ValueReference>c/NAME</ValueReference><Literal>Green Forest</Literal></PropertyIsEqualTo><PropertyIsEqualTo><ValueReference>d/NAME</ValueReference><Literal>Blue Lake</Literal></PropertyIsEqualTo></And></Or>";
    let xml = srv()
        .await
        .post_wfs(&join_post("cite:Forests cite:Lakes", Some("c d"), f, ""))
        .await
        .xml();
    xml.assert_count("//wfs2:Tuple", 1);
    xml.assert(&format!(
        "//wfs2:Tuple[wfs2:member/{}/{}='Green Forest' and wfs2:member/{}/{}='Blue Lake']",
        cite("Forests"),
        cite("NAME"),
        cite("Lakes"),
        cite("NAME")
    ));
}

/// GeoServer GetFeatureJoinTest.testSpatialJoinPOST_CSV / testStandardJoinCSV
#[actix_web::test]
async fn gs_get_feature_join_spatial_join_post_csv() {
    let r = srv()
        .await
        .post_wfs(&join_post(
            "cite:Forests cite:Lakes",
            Some("a b"),
            INTERSECTS_AB,
            " outputFormat='csv'",
        ))
        .await;
    r.assert_ok();
    assert!(r.content_type.starts_with("text/csv"), "{}", r.content_type);
    assert!(
        r.content_type.to_lowercase().contains("utf-8"),
        "{}",
        r.content_type
    );
    let cd = r.header("content-disposition").unwrap_or_default();
    assert!(cd.contains("filename=Forests_Lakes.csv"), "{cd}");
    for line in r.body.lines().filter(|l| !l.is_empty()) {
        // fields separated by commas outside quotes (WKT values are quoted)
        let mut in_quotes = false;
        let fields = 1 + line
            .chars()
            .filter(|c| {
                if *c == '"' {
                    in_quotes = !in_quotes;
                }
                *c == ',' && !in_quotes
            })
            .count();
        assert_eq!(fields, 7, "{line}");
    }
}

/// GeoServer GetFeatureJoinTest.testSelfJoinNoAliases
#[actix_web::test]
async fn gs_get_feature_join_self_join_no_aliases() {
    let body = r#"<wfs:GetFeature xmlns:wfs="http://www.opengis.net/wfs/2.0" count="10" service="WFS" startIndex="0" version="2.0.0"><wfs:Query xmlns:ns76="http://cite.opengeospatial.org/gmlsf" typeNames="ns76:PrimitiveGeoFeature ns76:PrimitiveGeoFeature"><Filter xmlns="http://www.opengis.net/fes/2.0"><PropertyIsEqualTo><ValueReference>ns76:PrimitiveGeoFeature/ns76:booleanProperty</ValueReference><ValueReference>ns76:PrimitiveGeoFeature/ns76:booleanProperty</ValueReference></PropertyIsEqualTo></Filter></wfs:Query></wfs:GetFeature>"#;
    let r = srv().await.post_wfs(body).await;
    let xml = r.xml();
    xml.assert_count("//wfs2:Tuple", 10);
    let desc = |i: usize, m: usize| {
        xml.string(&format!(
            "(//wfs2:Tuple)[{i}]/wfs2:member[{m}]//*[local-name()='description']"
        ))
    };
    let mut pairs: Vec<(String, String)> = (1..=10).map(|i| (desc(i, 1), desc(i, 2))).collect();
    pairs.sort();
    let mut expected: Vec<(String, String)> = Vec::new();
    for a in ["f001", "f003", "f008"] {
        for b in ["f001", "f003", "f008"] {
            expected.push((format!("description-{a}"), format!("description-{b}")));
        }
    }
    expected.push(("description-f002".into(), "description-f002".into()));
    expected.sort();
    assert_eq!(pairs, expected, "{}", r.body);
}

// ------------------------------------------------------------------ GetFeaturePagingTest (cdf:Fifteen)

async fn fifteen_count(s: &TestServer, query: &str) -> usize {
    let r = s
        .get(&format!(
            "/wfs?request=GetFeature&version=2.0.0&service=wfs&{query}"
        ))
        .await;
    r.assert_ok();
    r.xml().count("//cdf:Fifteen")
}

/// GeoServer GetFeaturePagingTest.testSingleType
#[actix_web::test]
async fn gs_get_feature_paging_single_type() {
    let s = srv().await;
    for (q, n) in [
        ("startIndex=10", 5),
        ("startIndex=16", 0),
        ("startIndex=0", 15),
        ("startIndex=1&count=1", 1),
        ("startIndex=16&count=1", 0),
    ] {
        assert_eq!(
            fifteen_count(&s, &format!("typename=cdf:Fifteen&{q}")).await,
            n,
            "{q}"
        );
    }
}

fn simple_xml(t: &str, si: Option<u32>, c: Option<u32>) -> String {
    let si = si.map(|v| format!(" startIndex='{v}'")).unwrap_or_default();
    let c = c.map(|v| format!(" count='{v}'")).unwrap_or_default();
    format!("<GetFeature version='2.0.0'{si}{c}> <Query typeNames='{t}'> </Query></GetFeature>")
}

/// GeoServer GetFeaturePagingTest.testStartIndexSimplePOST (namespace-less POST body)
#[actix_web::test]
async fn gs_get_feature_paging_start_index_simple_post() {
    let s = srv().await;
    for (si, c, n) in [
        (Some(10), None, 5),
        (Some(16), None, 0),
        (Some(0), None, 15),
        (Some(1), Some(1), 1),
        (Some(16), Some(1), 0),
    ] {
        let r = s.post_wfs(&simple_xml("cdf:Fifteen", si, c)).await;
        r.assert_ok();
        r.xml().assert_count("//cdf:Fifteen", n);
    }
}

/// GeoServer GetFeaturePagingTest.testStartIndexMultipleTypes
#[actix_web::test]
async fn gs_get_feature_paging_start_index_multiple_types() {
    let s = srv().await;
    for (q, n15, n7) in [
        ("startIndex=10", 5, 7),
        ("startIndex=16", 0, 6),
        ("startIndex=10&count=5", 5, 0),
        ("startIndex=10&count=6", 5, 1),
        ("startIndex=25", 0, 0),
    ] {
        let xml = s.get(&format!("/wfs?request=GetFeature&version=2.0.0&service=wfs&typename=cdf:Fifteen,cdf:Seven&{q}")).await.xml();
        xml.assert_count("//cdf:Fifteen", n15);
        xml.assert_count("//cdf:Seven", n7);
    }
}

/// GeoServer GetFeaturePagingTest.testStartIndexMultipleTypesPOST
#[actix_web::test]
async fn gs_get_feature_paging_start_index_multiple_types_post() {
    let s = srv().await;
    for (si, c, n15, n7) in [
        (10, None, 5, 7),
        (16, None, 0, 6),
        (10, Some(5), 5, 0),
        (10, Some(6), 5, 1),
        (25, None, 0, 0),
    ] {
        let c = c.map(|v| format!(" count='{v}'")).unwrap_or_default();
        let body = format!("<GetFeature version=\"2.0.0\" startIndex='{si}'{c}> <Query typeNames='cdf:Fifteen'> </Query> <Query typeNames='cdf:Seven'> </Query></GetFeature>");
        let xml = s.post_wfs(&body).await.xml();
        xml.assert_count("//cdf:Fifteen", n15);
        xml.assert_count("//cdf:Seven", n7);
    }
}

/// GeoServer GetFeaturePagingTest.testWithFilter
#[actix_web::test]
async fn gs_get_feature_paging_with_filter() {
    let s = srv().await;
    assert_eq!(
        fifteen_count(&s, "typename=cdf:Fifteen&startIndex=10").await,
        5
    );
    assert_eq!(
        fifteen_count(&s, "typename=cdf:Fifteen&startIndex=10&count=4").await,
        4
    );
    let xml = s.post_wfs("<GetFeature version='2.0.0' startIndex='10' count='100'><Query typeNames = 'cdf:Fifteen'/></GetFeature>").await.xml();
    xml.assert_count("//cdf:Fifteen", 5);
    let body = "<GetFeature version='2.0.0' xmlns:gml='http://www.opengis.net/gml/2.0' xmlns:fes='http://www.opengis.net/fes/2.0' startIndex='1' count='100'><Query typeNames='cdf:Fifteen'><fes:Filter><fes:ResourceId rid='Fifteen.3'/><fes:ResourceId rid='Fifteen.4'/><fes:ResourceId rid='Fifteen.5'/></fes:Filter></Query></GetFeature>";
    let xml = s.post_wfs(body).await.xml();
    xml.assert_count("//cdf:Fifteen", 2);
    xml.assert_count("//cdf:Fifteen[@gml32:id='Fifteen.3']", 0);
    xml.assert_count("//cdf:Fifteen[@gml32:id='Fifteen.4']", 1);
    xml.assert_count("//cdf:Fifteen[@gml32:id='Fifteen.5']", 1);
}

/// Check next/previous link: None = absent; Some((si, count)) with count None = no count key
#[track_caller]
fn assert_link(xml: &Xml, att: &str, expected: Option<(&str, Option<&str>)>) {
    let href = xml.string(&format!("/wfs2:FeatureCollection/@{att}"));
    match expected {
        None => assert!(href.is_empty(), "unexpected {att}: {href}"),
        Some((si, c)) => {
            assert!(!href.is_empty(), "missing {att}");
            let p = link_params(&href);
            assert_eq!(param(&p, "startindex"), Some(si), "{att}: {href}");
            assert_eq!(param(&p, "count"), c, "{att}: {href}");
        }
    }
}

/// GeoServer GetFeaturePagingTest.testNextPreviousGET
#[actix_web::test]
async fn gs_get_feature_paging_next_previous_get() {
    let s = srv().await;
    let get = |q: &str| {
        let s = &s;
        let q = q.to_string();
        async move {
            s.get(&format!(
                "/wfs?request=GetFeature&version=2.0.0&service=wfs&typename=cdf:Fifteen&{q}"
            ))
            .await
            .xml()
        }
    };
    // without startIndex GeoServer omits `next` (GEOS-5085); a next link is legitimate, not asserted
    let xml = get("count=5").await;
    assert_link(&xml, "previous", None);
    let xml = get("startIndex=0&count=5").await;
    assert_link(&xml, "previous", None);
    assert_link(&xml, "next", Some(("5", Some("5"))));
    let xml = get("startIndex=5&count=7").await;
    assert_link(&xml, "previous", Some(("0", Some("5"))));
    assert_link(&xml, "next", Some(("12", Some("7"))));
    let xml = get("startIndex=12&count=7").await;
    assert_link(&xml, "previous", Some(("5", Some("7"))));
    assert_link(&xml, "next", None);
    let xml = get("startIndex=15").await;
    assert_link(&xml, "previous", Some(("0", Some("15"))));
    assert_link(&xml, "next", None);
}

/// GeoServer GetFeaturePagingTest.testNextPreviousPOST
#[actix_web::test]
async fn gs_get_feature_paging_next_previous_post() {
    let s = srv().await;
    let xml = s
        .post_wfs(&simple_xml("cdf:Fifteen", None, Some(5)))
        .await
        .xml();
    assert_link(&xml, "previous", None);
    let xml = s
        .post_wfs(&simple_xml("cdf:Fifteen", Some(0), Some(5)))
        .await
        .xml();
    assert_link(&xml, "previous", None);
    assert_link(&xml, "next", Some(("5", Some("5"))));
    let xml = s
        .post_wfs(&simple_xml("cdf:Fifteen", Some(5), Some(7)))
        .await
        .xml();
    assert_link(&xml, "previous", Some(("0", Some("5"))));
    assert_link(&xml, "next", Some(("12", Some("7"))));
    let xml = s
        .post_wfs(&simple_xml("cdf:Fifteen", Some(15), None))
        .await
        .xml();
    assert_link(&xml, "previous", Some(("0", Some("15"))));
    assert_link(&xml, "next", None);
}

/// Path and query of a link, relative to the server
fn local_path(href: &str) -> String {
    let i = href.find("/wfs").expect("wfs path in link");
    href[i..].to_string()
}

/// GeoServer GetFeaturePagingTest.testNextPreviousLinksPOST
#[actix_web::test]
async fn gs_get_feature_paging_next_previous_links_post() {
    let s = srv().await;
    let body = "<GetFeature version='2.0.0' xmlns:gml='http://www.opengis.net/gml/2.0' xmlns:fes='http://www.opengis.net/fes/2.0' startIndex='0' count='2'><Query typeNames='cdf:Fifteen'><fes:Filter><fes:ResourceId rid='Fifteen.5'/><fes:ResourceId rid='Fifteen.6'/><fes:ResourceId rid='Fifteen.7'/><fes:ResourceId rid='Fifteen.8'/><fes:ResourceId rid='Fifteen.9'/></fes:Filter></Query></GetFeature>";
    let xml = s.post_wfs(body).await.xml();
    xml.assert_count("//cdf:Fifteen", 2);
    xml.assert("//cdf:Fifteen[@gml32:id='Fifteen.5']");
    xml.assert("//cdf:Fifteen[@gml32:id='Fifteen.6']");
    assert_link(&xml, "previous", None);
    let next = xml.string("/wfs2:FeatureCollection/@next");
    let p = link_params(&next);
    assert_eq!(param(&p, "startindex"), Some("2"));
    assert_eq!(param(&p, "count"), Some("2"));
    assert!(
        param(&p, "typenames").unwrap_or("").contains("cdf:Fifteen"),
        "{next}"
    );
    assert!(param(&p, "filter").is_some(), "{next}");
    let xml = s.get(&local_path(&next)).await.xml();
    xml.assert_count("//cdf:Fifteen", 2);
    xml.assert("//cdf:Fifteen[@gml32:id='Fifteen.7']");
    xml.assert("//cdf:Fifteen[@gml32:id='Fifteen.8']");
    assert_link(&xml, "previous", Some(("0", Some("2"))));
    assert_link(&xml, "next", Some(("4", Some("2"))));
    let next = xml.string("/wfs2:FeatureCollection/@next");
    let xml = s.get(&local_path(&next)).await.xml();
    xml.assert_count("//cdf:Fifteen", 1);
    xml.assert("//cdf:Fifteen[@gml32:id='Fifteen.9']");
    assert_link(&xml, "previous", Some(("2", Some("2"))));
    assert_link(&xml, "next", None);
}

/// GeoServer GetFeaturePagingTest.testNextPreviousHitsGET
#[actix_web::test]
async fn gs_get_feature_paging_next_previous_hits_get() {
    let s = srv().await;
    for (q, next) in [
        ("count=5", None),
        ("startIndex=0&count=5", Some(("0", Some("5")))),
        ("startIndex=5&count=7", Some(("0", Some("7")))),
        ("startIndex=12&count=7", Some(("0", Some("7")))),
        ("startIndex=15", Some(("0", None))),
    ] {
        let xml = s.get(&format!("/wfs?request=GetFeature&version=2.0.0&service=wfs&typename=cdf:Fifteen&resulttype=hits&{q}")).await.xml();
        assert_link(&xml, "previous", None);
        if next.is_some() {
            assert_link(&xml, "next", next);
        }
    }
}

/// GeoServer GetFeaturePagingTest.testCountZero
#[actix_web::test]
async fn gs_get_feature_paging_count_zero() {
    let xml = srv()
        .await
        .get("/wfs?request=GetFeature&version=2.0.0&service=wfs&typename=cdf:Fifteen&count=0")
        .await
        .xml();
    xml.assert("/wfs2:FeatureCollection[@numberMatched='15' and @numberReturned='0']");
}

// ------------------------------------------------------------------ StoredQueryTest

const CREATE_WITHIN: &str = "<wfs:CreateStoredQuery service='WFS' version='2.0.0' xmlns:wfs='http://www.opengis.net/wfs/2.0' xmlns:fes='http://www.opengis.net/fes/2.0' xmlns:gml='http://www.opengis.net/gml/3.2' xmlns:myns='http://www.someserver.com/myns' xmlns:sf='http://cite.opengeospatial.org/gmlsf'><wfs:StoredQueryDefinition id='myStoredQuery'><wfs:Parameter name='AreaOfInterest' type='gml:Polygon'/><wfs:QueryExpressionText returnFeatureTypes='RFT' language='urn:ogc:def:queryLanguage:OGC-WFS::WFS_QueryExpression' isPrivate='false'><wfs:Query typeNames='QTYPE'><fes:Filter><fes:Within><fes:ValueReference>pointProperty</fes:ValueReference> ${AreaOfInterest}</fes:Within></fes:Filter></wfs:Query></wfs:QueryExpressionText></wfs:StoredQueryDefinition></wfs:CreateStoredQuery>";

fn create_within(rft: &str, qtype: &str) -> String {
    CREATE_WITHIN.replace("RFT", rft).replace("QTYPE", qtype)
}

/// Number of built-in stored queries (GeoServer: 1; bbox may offer more)
async fn builtin_count() -> usize {
    srv()
        .await
        .get("/wfs?service=WFS&version=2.0.0&request=ListStoredQueries")
        .await
        .xml()
        .count("//wfs2:StoredQuery")
}

async fn list_count(s: &TestServer) -> usize {
    s.get("/wfs?request=ListStoredQueries")
        .await
        .xml()
        .count("//wfs2:StoredQuery")
}

/// GeoServer StoredQueryTest.testListStoredQueries
#[actix_web::test]
async fn gs_stored_query_list_stored_queries() {
    let xml = srv()
        .await
        .get("/wfs?request=ListStoredQueries&service=wfs&version=2.0.0")
        .await
        .xml();
    xml.assert("//wfs2:StoredQuery[@id='urn:ogc:def:query:OGC-WFS::GetFeatureById']");
    xml.assert_valid(WFS2_XSD);
}

/// GeoServer StoredQueryTest.testListStoredQueries2
#[actix_web::test]
async fn gs_stored_query_list_stored_queries2() {
    let s = srv().await;
    s.post_wfs(&create_within(
        "sf:PrimitiveGeoFeature",
        "sf:PrimitiveGeoFeature",
    ))
    .await
    .assert_ok();
    let xml = s
        .get("/wfs?request=ListStoredQueries&service=wfs&version=2.0.0")
        .await
        .xml();
    xml.assert_count("//wfs2:StoredQuery", builtin_count().await + 1);
    xml.assert("//wfs2:StoredQuery[@id='urn:ogc:def:query:OGC-WFS::GetFeatureById']");
    xml.assert("//wfs2:StoredQuery[@id='myStoredQuery']");
}

/// GeoServer StoredQueryTest.testCreateUnknownLanguage
#[actix_web::test]
async fn gs_stored_query_create_unknown_language() {
    let body = r#"<CreateStoredQuery xmlns="http://www.opengis.net/wfs/2.0" service="WFS" version="2.0.0"><StoredQueryDefinition xmlns:xsd="http://www.w3.org/2001/XMLSchema" id="urn:example:wfs2-query:InvalidLang"><Title>GetFeatureByTypeName</Title><Abstract>Returns feature representations by type name.</Abstract><Parameter name="typeName" type="xsd:QName"><Abstract>Qualified name of feature type (required).</Abstract></Parameter><QueryExpressionText isPrivate="false" language="http://qry.example.org" returnFeatureTypes=""><Query typeNames="${typeName}"/></QueryExpressionText></StoredQueryDefinition></CreateStoredQuery>"#;
    let r = srv().await.post_wfs(body).await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert_ex(&r, "InvalidParameterValue", Some("language"));
}

/// GeoServer StoredQueryTest.testCreateStoredQuery
#[actix_web::test]
async fn gs_stored_query_create_stored_query() {
    let s = srv().await;
    let xml = s.post_wfs("<wfs:ListStoredQueries service='WFS' version='2.0.0' xmlns:wfs='http://www.opengis.net/wfs/2.0'/>").await.xml();
    xml.assert("/wfs2:ListStoredQueriesResponse");
    xml.assert_count("//wfs2:StoredQuery", builtin_count().await);
    let xml = s
        .post_wfs(&create_within(
            "sf:PrimitiveGeoFeature",
            "sf:PrimitiveGeoFeature",
        ))
        .await
        .xml();
    xml.assert("/wfs2:CreateStoredQueryResponse[@status='OK']");
    let xml = s.get("/wfs?request=ListStoredQueries").await.xml();
    xml.assert_count("//wfs2:StoredQuery", builtin_count().await + 1);
    xml.assert("//wfs2:StoredQuery[@id='myStoredQuery']");
    xml.assert("//wfs2:ReturnFeatureType[text()='sf:PrimitiveGeoFeature']");
}

/// GeoServer StoredQueryTest.testDuplicateStoredQuery
#[actix_web::test]
async fn gs_stored_query_duplicate_stored_query() {
    let s = srv().await;
    let create = create_within("sf:PrimitiveGeoFeature", "sf:PrimitiveGeoFeature");
    s.post_wfs(&create)
        .await
        .xml()
        .assert("/wfs2:CreateStoredQueryResponse[@status='OK']");
    let r = s.post_wfs(&create).await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert_ex(&r, "DuplicateStoredQueryIdValue", Some("myStoredQuery"));
}

/// GeoServer StoredQueryTest.testCreateStoredQueryMismatchingTypes
#[actix_web::test]
async fn gs_stored_query_create_stored_query_mismatching_types() {
    let s = srv().await;
    let xml = s
        .post_wfs(&create_within(
            "sf:PrimitiveGeoFeature",
            "sf:AggregateGeoFeature",
        ))
        .await
        .xml();
    xml.assert("/ows11:ExceptionReport");
    assert_eq!(list_count(&s).await, builtin_count().await);
}

/// GeoServer StoredQueryTest.testDescribeStoredQueries
#[actix_web::test]
async fn gs_stored_query_describe_stored_queries() {
    let s = srv().await;
    let r = s
        .get("/wfs?request=DescribeStoredQueries&storedQueryId=myStoredQuery")
        .await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert_ex(&r, "InvalidParameterValue", Some("STOREDQUERY_ID"));
    s.post_wfs(&create_within(
        "sf:PrimitiveGeoFeature",
        "sf:PrimitiveGeoFeature",
    ))
    .await
    .assert_ok();
    let xml = s.post_wfs("<wfs:DescribeStoredQueries xmlns:wfs='http://www.opengis.net/wfs/2.0' service='WFS'><wfs:StoredQueryId>myStoredQuery</wfs:StoredQueryId></wfs:DescribeStoredQueries>").await.xml();
    xml.assert("/wfs2:DescribeStoredQueriesResponse");
    xml.assert("//wfs2:StoredQueryDescription[@id='myStoredQuery']");
}

/// GeoServer StoredQueryTest.testDescribeStoredQueries2
#[actix_web::test]
async fn gs_stored_query_describe_stored_queries2() {
    let s = srv().await;
    s.get("/wfs?request=DescribeStoredQueries&storedQuery_Id=myStoredQuery")
        .await
        .xml()
        .assert("/ows11:ExceptionReport");
    s.post_wfs(&create_within(
        "sf:PrimitiveGeoFeature",
        "sf:PrimitiveGeoFeature",
    ))
    .await
    .assert_ok();
    let xml = s
        .get("/wfs?request=DescribeStoredQueries&storedQuery_Id=myStoredQuery")
        .await
        .xml();
    xml.assert("/wfs2:DescribeStoredQueriesResponse");
    xml.assert("//wfs2:StoredQueryDescription[@id='myStoredQuery']");
}

/// GeoServer StoredQueryTest.testDescribeDefaultStoredQuery
#[actix_web::test]
async fn gs_stored_query_describe_default_stored_query() {
    let xml = srv()
        .await
        .get("/wfs?request=DescribeStoredQueries&storedQueryId=urn:ogc:def:query:OGC-WFS::GetFeatureById")
        .await
        .xml();
    xml.assert("/wfs2:DescribeStoredQueriesResponse");
    xml.assert("//wfs2:StoredQueryDescription[@id='urn:ogc:def:query:OGC-WFS::GetFeatureById']");
    xml.assert("//wfs2:Parameter[@name='ID']");
    xml.assert("//wfs2:QueryExpressionText[@isPrivate='true']");
    xml.assert_not("//wfs2:QueryExpressionText/*");
}

/// GeoServer StoredQueryTest.testDropStoredQuery
#[actix_web::test]
async fn gs_stored_query_drop_stored_query() {
    let s = srv().await;
    s.get("/wfs?request=DropStoredQuery&id=myStoredQuery")
        .await
        .xml()
        .assert("/ows11:ExceptionReport");
    s.post_wfs(&create_within(
        "sf:PrimitiveGeoFeature",
        "sf:PrimitiveGeoFeature",
    ))
    .await
    .assert_ok();
    let xml = s.post_wfs("<wfs:DropStoredQuery xmlns:wfs='http://www.opengis.net/wfs/2.0' service='WFS' id='myStoredQuery'/>").await.xml();
    xml.assert("/wfs2:DropStoredQueryResponse[@status='OK']");
    s.get("/wfs?request=DropStoredQuery&id=myStoredQuery")
        .await
        .xml()
        .assert("/ows11:ExceptionReport");
}

/// GeoServer StoredQueryTest.testDropStoredQuery2
#[actix_web::test]
async fn gs_stored_query_drop_stored_query2() {
    let s = srv().await;
    let drop = "/wfs?request=DropStoredQuery&storedQuery_id=myStoredQuery";
    s.get(drop).await.xml().assert("/ows11:ExceptionReport");
    s.post_wfs(&create_within(
        "sf:PrimitiveGeoFeature",
        "sf:PrimitiveGeoFeature",
    ))
    .await
    .assert_ok();
    s.get(drop)
        .await
        .xml()
        .assert("/wfs2:DropStoredQueryResponse[@status='OK']");
    s.get(drop).await.xml().assert("/ows11:ExceptionReport");
}

const SOAP12: &str = "http://www.w3.org/2003/05/soap-envelope";

async fn soap_post(s: &TestServer, body: &str) -> Response {
    s.post_with_type("/wfs", &soap(SOAP12, body), "application/soap+xml")
        .await
}

#[track_caller]
fn assert_soap(r: &Response, element: &str) {
    assert!(
        r.content_type.starts_with("application/soap+xml"),
        "{}",
        r.content_type
    );
    let doc = roxmltree::Document::parse(&r.body).unwrap();
    let root = doc.root_element();
    assert_eq!(
        (root.tag_name().namespace(), root.tag_name().name()),
        (Some(SOAP12), "Envelope"),
        "{}",
        r.body
    );
    let n = doc
        .descendants()
        .filter(|n| {
            n.tag_name().name() == element
                && n.tag_name().namespace() == Some("http://www.opengis.net/wfs/2.0")
        })
        .count();
    assert_eq!(n, 1, "{}", r.body);
}

/// GeoServer StoredQueryTest.testCreateStoredQuerySOAP
#[actix_web::test]
async fn gs_stored_query_create_stored_query_soap() {
    let s = srv().await;
    let r = soap_post(
        &s,
        &create_within("sf:PrimitiveGeoFeature", "sf:PrimitiveGeoFeature"),
    )
    .await;
    assert_soap(&r, "CreateStoredQueryResponse");
}

/// GeoServer StoredQueryTest.testDescribeStoredQueriesSOAP
#[actix_web::test]
async fn gs_stored_query_describe_stored_queries_soap() {
    let s = srv().await;
    s.post_wfs(&create_within(
        "sf:PrimitiveGeoFeature",
        "sf:PrimitiveGeoFeature",
    ))
    .await
    .assert_ok();
    let r = soap_post(&s, "<wfs:DescribeStoredQueries xmlns:wfs='http://www.opengis.net/wfs/2.0' service='WFS' version='2.0.0'><wfs:StoredQueryId>myStoredQuery</wfs:StoredQueryId></wfs:DescribeStoredQueries>").await;
    assert_soap(&r, "DescribeStoredQueriesResponse");
}

/// GeoServer StoredQueryTest.testListStoredQueriesSOAP
#[actix_web::test]
async fn gs_stored_query_list_stored_queries_soap() {
    let s = srv().await;
    let r = soap_post(&s, "<wfs:ListStoredQueries service='WFS' version='2.0.0' xmlns:wfs='http://www.opengis.net/wfs/2.0'/>").await;
    assert_soap(&r, "ListStoredQueriesResponse");
}

/// GeoServer StoredQueryTest.testDropStoredQuerySOAP
#[actix_web::test]
async fn gs_stored_query_drop_stored_query_soap() {
    let s = srv().await;
    s.post_wfs(&create_within(
        "sf:PrimitiveGeoFeature",
        "sf:PrimitiveGeoFeature",
    ))
    .await
    .assert_ok();
    let r = soap_post(&s, "<wfs:DropStoredQuery service='WFS' version='2.0.0' xmlns:wfs='http://www.opengis.net/wfs/2.0' id='myStoredQuery'/>").await;
    assert_soap(&r, "DropStoredQueryResponse");
}

/// GeoServer StoredQueryTest.testDropUnknownStoredQuery
#[actix_web::test]
async fn gs_stored_query_drop_unknown_stored_query() {
    let r = srv()
        .await
        .get("/wfs?request=DropStoredQuery&storedQuery_Id=myStoredQuery")
        .await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert_ex(&r, "InvalidParameterValue", Some("id"));
}

/// GeoServer StoredQueryTest.testCreateParametrizedOnTypename
#[actix_web::test]
async fn gs_stored_query_create_parametrized_on_typename() {
    let s = srv().await;
    let body = r#"<CreateStoredQuery xmlns="http://www.opengis.net/wfs/2.0" service="WFS" version="2.0.0"><StoredQueryDefinition xmlns:xsd="http://www.w3.org/2001/XMLSchema" id="urn:example:wfs2-query:GetFeatureByTypeName"><Title>GetFeatureByTypeName</Title><Abstract>Returns feature representations by type name.</Abstract><Parameter name="typeName" type="xsd:QName"><Abstract>Qualified name of feature type (required).</Abstract></Parameter><QueryExpressionText isPrivate="false" language="urn:ogc:def:queryLanguage:OGC-WFS::WFSQueryExpression" returnFeatureTypes=""><Query typeNames="${typeName}"/></QueryExpressionText></StoredQueryDefinition></CreateStoredQuery>"#;
    s.post_wfs(body)
        .await
        .xml()
        .assert("/wfs2:CreateStoredQueryResponse[@status='OK']");
    let xml = s.get("/wfs?request=ListStoredQueries").await.xml();
    xml.assert_count("//wfs2:StoredQuery", builtin_count().await + 1);
    xml.assert("//wfs2:StoredQuery[@id='urn:example:wfs2-query:GetFeatureByTypeName']");
    let r = s
        .get("/wfs?service=WFS&version=2.0.0&request=GetFeature&storedQuery_id=urn:example:wfs2-query:GetFeatureByTypeName&typename=tns:Fifteen")
        .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let xml = r.xml();
    xml.assert("/wfs2:FeatureCollection");
    xml.assert_count("//cdf:Fifteen", 15);
}

/// GeoServer StoredQueryTest.testCreateWithLocalNamespaceDeclaration
#[actix_web::test]
async fn gs_stored_query_create_with_local_namespace_declaration() {
    let s = srv().await;
    let body = r#"<wfs:CreateStoredQuery xmlns:wfs="http://www.opengis.net/wfs/2.0" service="WFS" version="2.0.0"><wfs:StoredQueryDefinition xmlns:xsd="http://www.w3.org/2001/XMLSchema" id="urn:example:wfs2-query:GetFeatureByName"><wfs:Title>GetFeatureByName</wfs:Title><wfs:Parameter name="name" type="xsd:string"/><wfs:QueryExpressionText xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:ns42="http://cite.opengeospatial.org/gmlsf" isPrivate="false" language="urn:ogc:def:queryLanguage:OGC-WFS::WFSQueryExpression" returnFeatureTypes="ns42:GenericEntity"><wfs:Query typeNames="ns42:GenericEntity"><fes:Filter><fes:PropertyIsLike escapeChar="\" singleChar="?" wildCard="*"><fes:ValueReference>gml:name</fes:ValueReference><fes:Literal>*${name}*</fes:Literal></fes:PropertyIsLike></fes:Filter></wfs:Query></wfs:QueryExpressionText></wfs:StoredQueryDefinition></wfs:CreateStoredQuery>"#;
    s.post_wfs(body)
        .await
        .xml()
        .assert("/wfs2:CreateStoredQueryResponse[@status='OK']");
    let xml = s.get("/wfs?request=ListStoredQueries").await.xml();
    xml.assert_count("//wfs2:StoredQuery", builtin_count().await + 1);
    xml.assert("//wfs2:StoredQuery[@id='urn:example:wfs2-query:GetFeatureByName']");
}

/// GeoServer StoredQueryTest.testCreateStoredQueryWithEmptyReturnFeatureTypes
#[actix_web::test]
async fn gs_stored_query_create_stored_query_with_empty_return_feature_types() {
    let s = srv().await;
    assert_eq!(list_count(&s).await, builtin_count().await);
    s.post_wfs(&create_within("", "sf:PrimitiveGeoFeature"))
        .await
        .xml()
        .assert("/wfs2:CreateStoredQueryResponse[@status='OK']");
    let xml = s.get("/wfs?request=ListStoredQueries").await.xml();
    xml.assert_count("//wfs2:StoredQuery", builtin_count().await + 1);
    xml.assert("//wfs2:StoredQuery[@id='myStoredQuery']");
    xml.assert("//wfs2:ReturnFeatureType[text()='sf:PrimitiveGeoFeature']");
    let xml = s.post_wfs("<wfs:DescribeStoredQueries xmlns:wfs='http://www.opengis.net/wfs/2.0' service='WFS'><wfs:StoredQueryId>myStoredQuery</wfs:StoredQueryId></wfs:DescribeStoredQueries>").await.xml();
    xml.assert("//wfs2:StoredQueryDescription[@id='myStoredQuery']");
    xml.assert("//wfs2:QueryExpressionText/@returnFeatureTypes");
    xml.assert("//wfs2:QueryExpressionText[@returnFeatureTypes='']");
    let body = "<wfs:GetFeature service='WFS' version='2.0.0' xmlns:gml='http://www.opengis.net/gml/3.2' xmlns:wfs='http://www.opengis.net/wfs/2.0' xmlns:fes='http://www.opengis.net/fes/2.0'><wfs:StoredQuery id='myStoredQuery'><wfs:Parameter name='AreaOfInterest'><gml:Envelope srsName='EPSG:4326'><gml:lowerCorner>57.0 -4.5</gml:lowerCorner><gml:upperCorner>62.0 1.0</gml:upperCorner></gml:Envelope></wfs:Parameter></wfs:StoredQuery></wfs:GetFeature>";
    let xml = assert_gml32(&s.post_wfs(body).await);
    xml.assert_count("//sf:PrimitiveGeoFeature", 1);
    xml.assert("//sf:PrimitiveGeoFeature/gml32:name = 'name-f002'");
}

// ------------------------------------------------------------------ GetPropertyValueTest

#[track_caller]
fn assert_point_values(r: &Response) {
    let xml = r.xml();
    xml.assert("/wfs2:ValueCollection");
    xml.assert_count("//wfs2:member", 3);
    // GeoServer wraps the value in its property element (wfs:member/sf:pointProperty/gml:Point);
    // bbox emits the value itself (as accepted by ets-wfs20): accept both
    xml.assert_count("//wfs2:member//gml32:Point", 3);
}

/// GeoServer GetPropertyValueTest.testPOST
#[actix_web::test]
async fn gs_get_property_value_post() {
    let body = "<wfs:GetPropertyValue service='WFS' version='2.0.0' xmlns:sf='http://cite.opengeospatial.org/gmlsf' xmlns:fes='http://www.opengis.net/fes/2.0' xmlns:wfs='http://www.opengis.net/wfs/2.0' valueReference='pointProperty'><wfs:Query typeNames='sf:PrimitiveGeoFeature'/></wfs:GetPropertyValue>";
    assert_point_values(&srv().await.post_wfs(body).await);
}

/// GeoServer GetPropertyValueTest.testGET
#[actix_web::test]
async fn gs_get_property_value_get() {
    let r = srv()
        .await
        .get("/wfs?service=WFS&version=2.0.0&request=GetPropertyValue&typeNames=sf:PrimitiveGeoFeature&valueReference=pointProperty")
        .await;
    assert_point_values(&r);
}

/// GeoServer GetPropertyValueTest.testGETAlternateNamespace
#[actix_web::test]
async fn gs_get_property_value_get_alternate_namespace() {
    let r = srv()
        .await
        .get("/wfs?service=WFS&version=2.0.0&request=GetPropertyValue&typeNames=abcd:PrimitiveGeoFeature&valueReference=pointProperty&namespaces=xmlns(abcd,http://cite.opengeospatial.org/gmlsf)")
        .await;
    assert_point_values(&r);
}

/// GeoServer GetPropertyValueTest.testEmptyValueReference
#[actix_web::test]
async fn gs_get_property_value_empty_value_reference() {
    let r = srv()
        .await
        .get("/wfs?service=WFS&version=2.0.0&request=GetPropertyValue&typeNames=sf:PrimitiveGeoFeature&valueReference=")
        .await;
    assert_ex(&r, "InvalidParameterValue", Some("valueReference"));
}

/// GeoServer GetPropertyValueTest.testGmlId
#[actix_web::test]
async fn gs_get_property_value_gml_id() {
    let xml = srv()
        .await
        .get("/wfs?service=WFS&version=2.0.0&request=GetPropertyValue&typeNames=sf:PrimitiveGeoFeature&valueReference=@gml:id")
        .await
        .xml();
    xml.assert("/wfs2:ValueCollection");
    xml.assert_count("//wfs2:member", 5);
    // GeoServer encodes ids as gml:identifier elements; the attribute value as text is accepted too
    xml.assert_count("//wfs2:member[gml32:identifier or starts-with(normalize-space(.), 'PrimitiveGeoFeature.')]", 5);
}

// ------------------------------------------------------------------ ExtendedOperatorTest

/// GeoServer ExtendedOperatorTest.testInvokeExtendedOperator
#[actix_web::test]
async fn gs_extended_operator_invoke_extended_operator() {
    let body = "<wfs:GetFeature service='WFS' version='2.0.0' xmlns:wfs='http://www.opengis.net/wfs/2.0' xmlns:fes='http://www.opengis.net/fes/2.0' xmlns:foo='http://foo.org'><wfs:Query typeNames='sf:PrimitiveGeoFeature'><fes:Filter><foo:strMatches><fes:ValueReference>name</fes:ValueReference><fes:Literal>name-f002</fes:Literal></foo:strMatches></fes:Filter></wfs:Query></wfs:GetFeature>";
    let r = srv().await.post_wfs(body).await;
    let xml = r.xml();
    xml.assert_count("//sf:PrimitiveGeoFeature", 1);
    xml.assert("//sf:PrimitiveGeoFeature/gml32:name[text()='name-f002']");
}

/// CQL_FILTER axis order (observed on GeoServer 3.0.1): x/y in WFS 1.0, latitude/longitude for
/// EPSG:4326 in WFS 1.1 and 2.0, like filter encoding literals
#[actix_web::test]
async fn gs_cql_bbox_axis_order() {
    let s = srv().await;
    let count = |v: &str, bbox: &str| {
        let s = &s;
        let path = format!(
            "/wfs?service=WFS&version={v}&request=GetFeature&typeName=sf:PrimitiveGeoFeature&cql_filter={}",
            enc(&format!("BBOX(pointProperty,{bbox})"))
        );
        async move {
            s.get(&path)
                .await
                .xml()
                .count("//*[local-name()='PrimitiveGeoFeature']")
        }
    };
    assert_eq!(count("1.0.0", "30,0,60,30").await, 2);
    assert_eq!(count("1.0.0", "0,30,30,60").await, 0);
    for v in ["1.1.0", "2.0.0"] {
        assert_eq!(count(v, "0,30,30,60").await, 2, "{v}");
        assert_eq!(count(v, "30,0,60,30").await, 0, "{v}");
    }
}
