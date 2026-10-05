//! GeoServer WFS 1.0.0 / shared 1.x behaviour (GeoServer `wfs1_x` root package tests),
//! re-implemented against the GeoServer test catalog (see tests/common/geoserver.rs).
//!
//! Not ported (reason):
//! - TransactionTest, LockFeatureTest, GetFeatureWithLockTest, ReprojectionWriteTest,
//!   GeometrylessWriteTest, RetypingTransactionTest, TransactionCallbackWFS11Test,
//!   TransactionListenerTest: write operations (read-only server).
//! - GetFeatureTest testGetNullGeometies, ReprojectionTest testReprojectNullGeometries:
//!   cite:NullGeometries is not in the default catalog.
//! - GetFeatureTest testGetIAULayer, GetCapabilitiesTest testIauFeatureTypes: IAU planetary CRS.
//! - GetFeatureTest testWorkspaceQualified/testLayerQualified, GetCapabilitiesTest
//!   testWorkspaceQualified/testLayerQualified/testNonAdvertisedLayer/testWFSDisabledLayer,
//!   DescribeFeatureTest testWorkspaceQualified, WfsIsolatedWorkspacesTest: virtual services.
//! - GetFeatureTest testRequestDisabledResource/testRequestDisabledStore/testCQLFilter,
//!   GetCapabilitiesTest testSkipMisconfiguredLayers/testNamespaceFilter/testOutputFormatAllowed,
//!   DescribeFeatureTest testSkipMisconfiguredLayers, WFSDisabledTest: catalog configuration.
//! - GetCapabilitiesTest testCachingHeaders/testCachingHeadersDisabled: GeoServer caching callback
//!   (bbox ETag/Cache-Control covered by tests/wfs_cache.rs).
//! - MaxFeaturesTest testLocalMax/testLocalMaxBigger/testCombinedLocalMaxes/
//!   testCombinedLocalMaxesBigger: per-layer maxFeatures config (request caps ported).
//! - GetFeatureHitsIgnoreMaxFeaturesTest testHitsIgnoreMaxFeaturesDisabled: GeoServer flag.
//! - GetFeatureBboxTest testFeatureBoudingOff: featureBounding config toggle.
//! - GetFeatureMissingTypesTest: requires deleting backing data of a configured type.
//! - SrsNameTest testSrsNameSyntax11: configurable srsName style.
//! - ReprojectionTest testGetFeatureGetAutoCRS/testGetFeatureAutoCRSBBox: AUTO:42001 CRS;
//!   testGetFeatureReprojectedFeatureType: declared-SRS reprojection policy, extra data.
//! - AliasTest: published-name aliasing.
//! - GetFeatureCurvesTest: curve layers not in the default catalog.
//! - NumDecimalsTest testGlobal/testPerFeatureType: numDecimals config (defaults ported).
//! - GMLOutputFormatTest coordinate formatting and invalid name/namespace tests: numDecimals
//!   config and extra data.
//! - WFSServiceExceptionTest: EXCEPTIONS=application/json vendor exception formats.
//! - SQLViewTest (viewparams), SecuredGetFeatureTest, ResourceAccessManagerWFSTest (security),
//!   StoredQueryProviderTest, GetFeatureCallbackTest, WFSCascadedStoredQueryConfigurationParserTest
//!   (internal Java units / extension points).

mod common;
use common::*;

const CITE: &str = "http://www.opengis.net/cite";

/// XPath step for an element in the cite namespace (prefix not registered in the harness)
fn cite(local: &str) -> String {
    format!("*[local-name()='{local}' and namespace-uri()='{CITE}']")
}

/// Server on the GeoServer catalog with additional `[wfs]` settings
async fn server_with(wfs_extra: &str) -> TestServer {
    static DIRS: tokio::sync::OnceCell<Vec<std::path::PathBuf>> =
        tokio::sync::OnceCell::const_new();
    let dirs = DIRS
        .get_or_init(|| async { geoserver::build(&tmp_dir().join("gs10-custom")).await })
        .await;
    let mut cfg = format!("[webserver]\npublic_server_url = \"{BASE_URL}\"\n");
    for d in dirs {
        cfg.push_str(&format!(
            "\n[[collections.directory]]\ndir = \"{}\"\n",
            d.display()
        ));
    }
    cfg.push_str(&format!(
        "\n[wfs]\nnamespace_prefix = \"gs\"\nnamespace_uri = \"http://geoserver.org\"\nother_crs = [4326, 3857, 32615, 4269]\n{wfs_extra}\n"
    ));
    for ns in geoserver::NAMESPACES {
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

const FIFTEEN: &str = "/wfs?request=GetFeature&typename=cdf:Fifteen&version=1.0.0&service=wfs";

#[track_caller]
fn assert_fifteen_all(xml: &Xml) {
    xml.assert("/wfs:FeatureCollection");
    xml.assert_count("//gml:featureMember", 15);
}

// ---------------------------------------------------------------- GetFeatureTest

/// GeoServer GetFeatureTest.testGet
#[actix_web::test]
async fn gs_get_feature_test_get() {
    let srv = geoserver::server().await;
    assert_fifteen_all(&srv.get(FIFTEEN).await.xml());
}

/// GeoServer GetFeatureTest.testPostForm
#[actix_web::test]
async fn gs_get_feature_test_post_form() {
    let srv = geoserver::server().await;
    let r = srv
        .post_with_type(
            "/wfs",
            "request=GetFeature&typename=cdf:Fifteen&version=1.0.0&service=wfs",
            "application/x-www-form-urlencoded; charset=UTF-8",
        )
        .await;
    assert_fifteen_all(&r.xml());
}

/// GeoServer GetFeatureTest.testCiteCompliant (form POST in strict mode)
#[actix_web::test]
async fn gs_get_feature_test_cite_compliant() {
    let srv = geoserver::server().await;
    let r = srv
        .post_with_type(
            "/wfs",
            "request=GetFeature&typename=cdf:Fifteen&version=1.0.0&service=wfs",
            "application/x-www-form-urlencoded",
        )
        .await;
    assert_fifteen_all(&r.xml());
}

/// GeoServer GetFeatureTest.testGetPropertyNameEmpty
#[actix_web::test]
async fn gs_get_feature_test_get_property_name_empty() {
    let srv = geoserver::server().await;
    assert_fifteen_all(&srv.get(&format!("{FIFTEEN}&propertyname=")).await.xml());
}

/// GeoServer GetFeatureTest.testGetFilterEmpty
#[actix_web::test]
async fn gs_get_feature_test_get_filter_empty() {
    let srv = geoserver::server().await;
    assert_fifteen_all(&srv.get(&format!("{FIFTEEN}&filter=")).await.xml());
}

/// GeoServer GetFeatureTest.testGetCqlFilterEmpty
#[actix_web::test]
async fn gs_get_feature_test_get_cql_filter_empty() {
    let srv = geoserver::server().await;
    assert_fifteen_all(&srv.get(&format!("{FIFTEEN}&cql_filter=")).await.xml());
}

/// GeoServer GetFeatureTest.testGetPropertyNameStar
#[actix_web::test]
async fn gs_get_feature_test_get_property_name_star() {
    let srv = geoserver::server().await;
    assert_fifteen_all(&srv.get(&format!("{FIFTEEN}&propertyname=*")).await.xml());
}

/// GeoServer GetFeatureTest.testGetMissingParams
#[actix_web::test]
async fn gs_get_feature_test_get_missing_params() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?request=GetFeature&typeNameWrongParam=cdf:Fifteen&version=1.0.0&service=wfs")
        .await
        .xml();
    xml.assert_count("//ogc:ServiceException", 1);
    assert_eq!(
        xml.string("//ogc:ServiceException/@code"),
        "MissingParameterValue"
    );
}

/// GeoServer GetFeatureTest.testAlienNamespace
#[actix_web::test]
async fn gs_get_feature_test_alien_namespace() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?request=GetFeature&typename=youdontknowme:Fifteen&version=1.0.0&service=wfs")
        .await
        .xml();
    xml.assert("/*[local-name()='ServiceExceptionReport']");
}

/// GeoServer GetFeatureTest.testGetWithFeatureId
#[actix_web::test]
async fn gs_get_feature_test_get_with_feature_id() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?request=GetFeature&typeName=cdf:Fifteen&version=1.0.0&service=wfs&featureid=Fifteen.2")
        .await
        .xml();
    xml.assert_count("//gml:featureMember", 1);
    assert_eq!(
        xml.string("//wfs:FeatureCollection/gml:featureMember/cdf:Fifteen/@fid"),
        "Fifteen.2"
    );
    let xml = srv
        .get("/wfs?request=GetFeature&typeName=cite:NamedPlaces&version=1.0.0&service=wfs&featureId=NamedPlaces.1107531895891")
        .await
        .xml();
    xml.assert_count("//gml:featureMember", 1);
    assert_eq!(
        xml.string(&format!(
            "//wfs:FeatureCollection/gml:featureMember/{}/@fid",
            cite("NamedPlaces")
        )),
        "NamedPlaces.1107531895891"
    );
}

/// GeoServer GetFeatureTest.testMultiLayer
#[actix_web::test]
async fn gs_get_feature_test_multi_layer() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?request=GetFeature&typename=cite:BasicPolygons,cite:Bridges&version=1.0.0&service=wfs")
        .await
        .xml();
    let loc = xml.string("/wfs:FeatureCollection/@xsi:schemaLocation");
    let parts: Vec<&str> = loc.split_whitespace().collect();
    let pos = parts
        .iter()
        .position(|p| *p == CITE)
        .unwrap_or_else(|| panic!("no schema location for {CITE}: {loc}"));
    let url = parts
        .get(pos + 1)
        .expect("schema location URL")
        .replace("%3A", ":")
        .replace("%2C", ",");
    let lower = url.to_lowercase();
    assert!(lower.contains("request=describefeaturetype"), "{url}");
    assert!(lower.contains("version=1.0.0"), "{url}");
    assert!(url.contains("cite:BasicPolygons,cite:Bridges"), "{url}");
}

const POST_OTHER: &str = r#"<wfs:GetFeature service="WFS" version="1.0.0" xmlns:cdf="http://www.opengis.net/cite/data" xmlns:ogc="http://www.opengis.net/ogc" xmlns:wfs="http://www.opengis.net/wfs"><wfs:Query typeName="cdf:Other"><ogc:PropertyName>cdf:string2</ogc:PropertyName></wfs:Query></wfs:GetFeature>"#;

/// GeoServer GetFeatureTest.testPost
#[actix_web::test]
async fn gs_get_feature_test_post() {
    let srv = geoserver::server().await;
    let xml = srv.post_wfs(POST_OTHER).await.xml();
    xml.assert("/wfs:FeatureCollection");
    assert!(xml.count("//gml:featureMember") > 0);
}

/// GeoServer GetFeatureTest.testPostWithFilter (arithmetic in filter)
#[actix_web::test]
async fn gs_get_feature_test_post_with_filter() {
    let srv = geoserver::server().await;
    let body = r#"<wfs:GetFeature service="WFS" version="1.0.0" outputFormat="GML2" xmlns:cdf="http://www.opengis.net/cite/data" xmlns:ogc="http://www.opengis.net/ogc" xmlns:wfs="http://www.opengis.net/wfs"><wfs:Query typeName="cdf:Other"><ogc:PropertyName>cdf:string2</ogc:PropertyName><ogc:Filter><ogc:PropertyIsEqualTo><ogc:PropertyName>cdf:integers</ogc:PropertyName><ogc:Add><ogc:Literal>4</ogc:Literal><ogc:Literal>3</ogc:Literal></ogc:Add></ogc:PropertyIsEqualTo></ogc:Filter></wfs:Query></wfs:GetFeature>"#;
    let xml = srv.post_wfs(body).await.xml();
    xml.assert("/wfs:FeatureCollection");
    assert!(xml.count("//gml:featureMember") > 0);
}

/// GeoServer GetFeatureTest.testLax (no wfs/ogc namespaces, unqualified type name)
#[actix_web::test]
async fn gs_get_feature_test_lax() {
    let srv = geoserver::server().await;
    let body = r#"<GetFeature version='1.1.0' xmlns:gml="http://www.opengis.net/gml"><Query typeName="Buildings"><PropertyName>ADDRESS</PropertyName><Filter><PropertyIsEqualTo><PropertyName>ADDRESS</PropertyName><Literal>123 Main Street</Literal></PropertyIsEqualTo></Filter></Query></GetFeature>"#;
    let xml = srv.post_wfs(body).await.xml();
    xml.assert("/wfs:FeatureCollection");
    xml.assert_count(&format!("//{}", cite("Buildings")), 1);
}

/// GeoServer GetFeatureTest.testMixed (KVP in URL and XML body)
#[actix_web::test]
async fn gs_get_feature_test_mixed() {
    let srv = geoserver::server().await;
    let xml = srv.post("/wfs?request=GetFeature", POST_OTHER).await.xml();
    xml.assert("/wfs:FeatureCollection");
    assert!(xml.count("//gml:featureMember") > 0);
}

/// GeoServer GetFeatureTest.testLikeMatchCase
#[actix_web::test]
async fn gs_get_feature_test_like_match_case() {
    let srv = geoserver::server().await;
    for (match_case, expected) in [("false", 2), ("true", 0)] {
        let body = format!(
            r#"<GetFeature version='1.1.0' xmlns:gml="http://www.opengis.net/gml"><Query typeName="Buildings"><Filter><PropertyIsLike wildCard="*" singleChar="." escapeChar="\" matchCase="{match_case}"><PropertyName>ADDRESS</PropertyName><Literal>* MAIN STREET</Literal></PropertyIsLike></Filter></Query></GetFeature>"#
        );
        let xml = srv.post_wfs(&body).await.xml();
        xml.assert("/wfs:FeatureCollection");
        xml.assert_count(&format!("//{}", cite("Buildings")), expected);
    }
}

/// GeoServer GetFeatureTest.testStrictComplianceBBoxValidator (bbox with CRS element)
#[actix_web::test]
async fn gs_get_feature_test_strict_compliance_bbox_validator() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?service=WFS&version=1.1.0&request=GetFeature&typeName=cite:Forests&bbox=1818131,6142575,1818198,6142642,EPSG:3857&srsName=EPSG:4326")
        .await
        .xml();
    xml.assert_count("//wfs:FeatureCollection", 1);
}

// ---------------------------------------------------------------- GetCapabilitiesTest

/// GeoServer GetCapabilitiesTest.testGet (lowercase request value)
#[actix_web::test]
async fn gs_get_capabilities_test_get() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?service=WFS&version=1.0.0&request=getCapabilities")
        .await
        .xml();
    xml.assert("/wfs:WFS_Capabilities");
    assert!(xml.count("//wfs:FeatureType") > 0);
}

/// GeoServer GetCapabilitiesTest.testPost
#[actix_web::test]
async fn gs_get_capabilities_test_post() {
    let srv = geoserver::server().await;
    let body = r#"<GetCapabilities service="WFS" version="1.0.0" xmlns="http://www.opengis.net/wfs" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:schemaLocation="http://www.opengis.net/wfs http://schemas.opengis.net/wfs/1.0.0/WFS-basic.xsd"/>"#;
    srv.post_wfs(body)
        .await
        .xml()
        .assert("/wfs:WFS_Capabilities");
}

/// GeoServer GetCapabilitiesTest.testOutputFormats (GeoServer format names in 1.0 ResultFormat)
#[actix_web::test]
async fn gs_get_capabilities_test_output_formats() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?service=WFS&version=1.0.0&request=GetCapabilities")
        .await
        .xml();
    let formats = xml.child_names("(//wfs:ResultFormat)[1]");
    // accepted difference: the WFS 1.0 capabilities schema allows only wfs:GML2 in ResultFormat;
    // GeoServer adds GML3, SHAPE-ZIP, JSON, CSV (schema invalid). The formats are supported.
    assert!(
        formats.iter().any(|n| n.ends_with("GML2")),
        "missing GML2 in {formats:?}"
    );
    for f in ["GML3", "SHAPE-ZIP", "application/json", "csv"] {
        let r = srv.get(&format!("/wfs?service=WFS&version=1.0.0&request=GetFeature&typeName=cdf:Fifteen&maxFeatures=1&outputFormat={f}")).await;
        assert_eq!(r.status, 200, "{f}: {}", r.body);
    }
}

/// GeoServer GetCapabilitiesTest.testSupportedSpatialOperators
#[actix_web::test]
async fn gs_get_capabilities_test_supported_spatial_operators() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?service=WFS&version=1.0.0&request=GetCapabilities")
        .await
        .xml();
    let mut ops: Vec<String> = xml
        .child_names("//ogc:Spatial_Operators")
        .into_iter()
        .map(|n| n.rsplit(':').next().unwrap_or(&n).to_string())
        .collect();
    ops.sort();
    let mut expected = vec![
        "Disjoint",
        "Equals",
        "DWithin",
        "Beyond",
        "Intersect",
        "Touches",
        "Crosses",
        "Within",
        "Contains",
        "Overlaps",
        "BBOX",
    ];
    expected.sort();
    assert_eq!(ops, expected);
}

/// GeoServer GetCapabilitiesTest.testTypeNameCount
#[actix_web::test]
async fn gs_get_capabilities_test_type_name_count() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?service=WFS&version=1.0.0&request=GetCapabilities")
        .await
        .xml();
    let expected: usize = geoserver::NAMESPACES.iter().map(|n| n.types.len()).sum();
    xml.assert_count(
        "/wfs:WFS_Capabilities/wfs:FeatureTypeList/wfs:FeatureType",
        expected,
    );
}

/// GeoServer GetCapabilitiesTest.testTypeNames
#[actix_web::test]
async fn gs_get_capabilities_test_type_names() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?service=WFS&version=1.0.0&request=GetCapabilities")
        .await
        .xml();
    for ns in geoserver::NAMESPACES {
        for t in ns.types {
            xml.assert(&format!(
                "/wfs:WFS_Capabilities/wfs:FeatureTypeList/wfs:FeatureType/wfs:Name[text()='{}:{t}']",
                ns.prefix
            ));
        }
    }
}

// ---------------------------------------------------------------- DescribeFeatureTest

/// GeoServer DescribeFeatureTest.testGet (all types)
#[actix_web::test]
async fn gs_describe_feature_test_get() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?service=WFS&request=DescribeFeatureType&version=1.0.0")
        .await
        .xml();
    xml.assert("/xs:schema");
}

/// GeoServer DescribeFeatureTest.testPost
#[actix_web::test]
async fn gs_describe_feature_test_post() {
    let srv = geoserver::server().await;
    let xml = srv
        .post_wfs(r#"<wfs:DescribeFeatureType service="WFS" version="1.0.0" xmlns:wfs="http://www.opengis.net/wfs" />"#)
        .await
        .xml();
    xml.assert("/xs:schema");
}

/// GeoServer DescribeFeatureTest.testPostDummyFeature
#[actix_web::test]
async fn gs_describe_feature_test_post_dummy_feature() {
    let srv = geoserver::server().await;
    let xml = srv
        .post_wfs(r#"<wfs:DescribeFeatureType service="WFS" version="1.0.0" xmlns:wfs="http://www.opengis.net/wfs"><wfs:TypeName>cgf:DummyFeature</wfs:TypeName></wfs:DescribeFeatureType>"#)
        .await
        .xml();
    xml.assert("/*[local-name()='ServiceExceptionReport']");
}

/// GeoServer DescribeFeatureTest.testWithoutExplicitMapping (prefix resolved by the server)
#[actix_web::test]
async fn gs_describe_feature_test_without_explicit_mapping() {
    let srv = geoserver::server().await;
    let body = r#"<DescribeFeatureType xmlns='http://www.opengis.net/wfs' xmlns:gml='http://www.opengis.net/gml' xmlns:ogc='http://www.opengis.net/ogc' version='1.0.0' service='WFS'><TypeName>cdf:Locks</TypeName></DescribeFeatureType>"#;
    let xml = srv.post_wfs(body).await.xml();
    xml.assert("/xs:schema");
    xml.assert_count("//xs:complexType", 1);
}

/// GeoServer DescribeFeatureTest.testWithoutTypeName (one import per namespace listing all types)
#[actix_web::test]
async fn gs_describe_feature_test_without_type_name() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?request=DescribeFeatureType&version=1.0.0")
        .await
        .xml();
    xml.assert("/xs:schema");
    xml.assert_count("//xs:import", geoserver::NAMESPACES.len());
    for ns in geoserver::NAMESPACES {
        let loc = xml.string(&format!(
            "//xs:import[@namespace='{}']/@schemaLocation",
            ns.uri
        ));
        assert!(!loc.is_empty(), "no import for {}", ns.uri);
        let query = loc.split_once('?').map(|(_, q)| q).unwrap_or("");
        let params: Vec<(String, String)> =
            serde_urlencoded::from_str(&query.replace("&amp;", "&")).unwrap();
        let get = |k: &str| {
            params
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(k))
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert_eq!(get("service").to_lowercase(), "wfs", "{loc}");
        assert_eq!(get("version"), "1.0.0", "{loc}");
        assert_eq!(
            get("request").to_lowercase(),
            "describefeaturetype",
            "{loc}"
        );
        let names: Vec<String> = get("typename").split(',').map(str::to_string).collect();
        assert_eq!(names.len(), ns.types.len(), "{loc}");
    }
}

/// GeoServer DescribeFeatureTest.testMultipleNamespaceNoTargetNamespace
#[actix_web::test]
async fn gs_describe_feature_test_multiple_namespace_no_target_namespace() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?request=DescribeFeatureType&version=1.0.0&typeName=sf:PrimitiveGeoFeature,cgf:Points")
        .await
        .xml();
    xml.assert("/xs:schema");
    xml.assert_not("/xs:schema/@targetNamespace");
}

/// GeoServer DescribeFeatureTest.testMethodNameInjection (request value echoed escaped)
#[actix_web::test]
async fn gs_describe_feature_test_method_name_injection() {
    let srv = geoserver::server().await;
    let r = srv
        .get("/wfs?service=WFS&version=1.0.0&request=DescribeFeatureType%22%3E%3C/ServiceException%3E%3Cfoo%3EHello,%20World%3C/foo%3E%3CServiceException+foo=%22&typeName=sf:archsites")
        .await;
    let xml = r.xml();
    xml.assert("/ogc:ServiceExceptionReport/ogc:ServiceException");
    assert_eq!(
        xml.string("//ogc:ServiceException/@code"),
        "OperationNotSupported"
    );
    assert_eq!(
        xml.string("//ogc:ServiceException/@locator"),
        r#"DescribeFeatureType"></ServiceException><foo>Hello, World</foo><ServiceException foo=""#
    );
    xml.assert_count("//foo", 0);
}

// ---------------------------------------------------------------- GetFeaturePagingTest (startIndex 1.x)

fn type_count(xml: &Xml, prefix: &str, t: &str) -> usize {
    xml.count(&format!("//{prefix}:{t}"))
}

/// GeoServer GetFeaturePagingTest.testSingleType
#[actix_web::test]
async fn gs_get_feature_paging_test_single_type() {
    let srv = geoserver::server().await;
    for (q, expected) in [
        ("startIndex=10", 5),
        ("startIndex=16", 0),
        ("startIndex=0", 15),
        ("startIndex=1&maxFeatures=1", 1),
        ("startIndex=16&maxFeatures=1", 0),
    ] {
        let xml = srv
            .get(&format!(
                "/wfs?request=GetFeature&version=1.0.0&service=wfs&typename=cdf:Fifteen&{q}"
            ))
            .await
            .xml();
        assert_eq!(type_count(&xml, "cdf", "Fifteen"), expected, "{q}");
    }
}

/// GeoServer GetFeaturePagingTest.testStartIndexSimplePOST
#[actix_web::test]
async fn gs_get_feature_paging_test_start_index_simple_post() {
    let srv = geoserver::server().await;
    for (start, max, expected) in [
        ("10", None, 5),
        ("16", None, 0),
        ("0", None, 15),
        ("1", Some("1"), 1),
        ("16", Some("1"), 0),
    ] {
        let max_attr = max
            .map(|m| format!(" maxFeatures='{m}'"))
            .unwrap_or_default();
        let body = format!(
            r#"<GetFeature version='1.0.0' xmlns:gml="http://www.opengis.net/gml" xmlns:cdf="http://www.opengis.net/cite/data" startIndex='{start}'{max_attr}><Query typeName='cdf:Fifteen'></Query></GetFeature>"#
        );
        let xml = srv.post_wfs(&body).await.xml();
        assert_eq!(
            type_count(&xml, "cdf", "Fifteen"),
            expected,
            "{start} {max:?}"
        );
    }
}

const PAGING_MULTI: &[(&str, usize, usize)] = &[
    ("startIndex=10", 5, 7),
    ("startIndex=16", 0, 6),
    ("startIndex=10&maxfeatures=5", 5, 0),
    ("startIndex=10&maxfeatures=6", 5, 1),
    ("startIndex=25", 0, 0),
];

/// GeoServer GetFeaturePagingTest.testStartIndexMultipleTypes
#[actix_web::test]
async fn gs_get_feature_paging_test_start_index_multiple_types() {
    let srv = geoserver::server().await;
    for (q, fifteen, seven) in PAGING_MULTI {
        let xml = srv
            .get(&format!("/wfs?request=GetFeature&version=1.0.0&service=wfs&typename=cdf:Fifteen,cdf:Seven&{q}"))
            .await
            .xml();
        assert_eq!(type_count(&xml, "cdf", "Fifteen"), *fifteen, "{q}");
        assert_eq!(type_count(&xml, "cdf", "Seven"), *seven, "{q}");
    }
}

/// GeoServer GetFeaturePagingTest.testStartIndexMultipleTypesPOST
#[actix_web::test]
async fn gs_get_feature_paging_test_start_index_multiple_types_post() {
    let srv = geoserver::server().await;
    for (q, fifteen, seven) in PAGING_MULTI {
        let attrs: String = q
            .split('&')
            .map(|kv| {
                let (k, v) = kv.split_once('=').unwrap();
                let k = if k == "maxfeatures" { "maxFeatures" } else { k };
                format!(" {k}='{v}'")
            })
            .collect();
        let body = format!(
            r#"<GetFeature version='1.0.0' xmlns:gml="http://www.opengis.net/gml" xmlns:cdf="http://www.opengis.net/cite/data"{attrs}><Query typeName='cdf:Fifteen'/><Query typeName='cdf:Seven'/></GetFeature>"#
        );
        let xml = srv.post_wfs(&body).await.xml();
        assert_eq!(type_count(&xml, "cdf", "Fifteen"), *fifteen, "{q}");
        assert_eq!(type_count(&xml, "cdf", "Seven"), *seven, "{q}");
    }
}

/// GeoServer GetFeaturePagingTest.testWithFilter (WFS 1.1.0)
#[actix_web::test]
async fn gs_get_feature_paging_test_with_filter() {
    let srv = geoserver::server().await;
    let base = "/wfs?request=GetFeature&version=1.1.0&service=wfs&typename=cdf:Fifteen";
    let xml = srv.get(&format!("{base}&startIndex=10")).await.xml();
    assert_eq!(type_count(&xml, "cdf", "Fifteen"), 5);
    let xml = srv
        .get(&format!("{base}&startIndex=10&maxfeatures=4"))
        .await
        .xml();
    assert_eq!(type_count(&xml, "cdf", "Fifteen"), 4);
    let xml = srv
        .post_wfs(r#"<GetFeature version='1.1.0' xmlns:gml="http://www.opengis.net/gml" xmlns:cdf="http://www.opengis.net/cite/data" startIndex='10' maxFeatures='100'><Query typeName='cdf:Fifteen'/></GetFeature>"#)
        .await
        .xml();
    assert_eq!(type_count(&xml, "cdf", "Fifteen"), 5);
    let xml = srv
        .post_wfs(r#"<GetFeature version='1.1.0' xmlns:gml="http://www.opengis.net/gml" xmlns:cdf="http://www.opengis.net/cite/data" xmlns:ogc="http://www.opengis.net/ogc" startIndex='1' maxFeatures='100'><Query typeName='cdf:Fifteen'><ogc:Filter><ogc:FeatureId fid='Fifteen.3'/><ogc:FeatureId fid='Fifteen.4'/><ogc:FeatureId fid='Fifteen.5'/></ogc:Filter></Query></GetFeature>"#)
        .await
        .xml();
    assert_eq!(type_count(&xml, "cdf", "Fifteen"), 2);
    xml.assert_count("//cdf:Fifteen[@gml:id='Fifteen.3']", 0);
    xml.assert_count("//cdf:Fifteen[@gml:id='Fifteen.4']", 1);
    xml.assert_count("//cdf:Fifteen[@gml:id='Fifteen.5']", 1);
}

// ---------------------------------------------------------------- MaxFeaturesTest (server cap 5)

/// GeoServer MaxFeaturesTest.testGlobalMax
#[actix_web::test]
async fn gs_max_features_test_global_max() {
    let srv = server_with("count_default = 5").await;
    srv.get(FIFTEEN)
        .await
        .xml()
        .assert_count("//gml:featureMember", 5);
}

/// GeoServer MaxFeaturesTest.testCombinedLocalMaxesBiggerRequestOverride (request maxFeatures
/// is a global cap across queries; per-layer limits not ported)
#[actix_web::test]
async fn gs_max_features_test_combined_local_maxes_bigger_request_override() {
    let srv = server_with("count_default = 5").await;
    let xml = srv
        .get("/wfs?request=GetFeature&typename=cdf:Fifteen,cite:BasicPolygons&version=1.0.0&service=wfs&srsName=EPSG:4326&maxFeatures=4")
        .await
        .xml();
    xml.assert_count("//gml:featureMember", 4);
    xml.assert_count("//cdf:Fifteen", 4);
    xml.assert_count(&format!("//{}", cite("BasicPolygons")), 0);
}

/// GeoServer MaxFeaturesTest.testMaxFeaturesBreak
#[actix_web::test]
async fn gs_max_features_test_max_features_break() {
    let srv = server_with("count_default = 5").await;
    let xml = srv
        .get("/wfs?request=GetFeature&typename=cdf:Fifteen,cite:BasicPolygons&version=1.0.0&service=wfs&maxFeatures=3")
        .await
        .xml();
    xml.assert_count("//gml:featureMember", 3);
    xml.assert_count("//cdf:Fifteen", 3);
    xml.assert_count(&format!("//{}", cite("BasicPolygons")), 0);
}

// ---------------------------------------------------------------- GetFeatureHitsIgnoreMaxFeaturesTest

/// GeoServer GetFeatureHitsIgnoreMaxFeaturesTest.testHitsIgnoreMaxFeaturesEnabled
#[actix_web::test]
async fn gs_get_feature_hits_ignore_max_features_test_hits_ignore_max_features_enabled() {
    let srv = server_with("count_default = 1").await;
    let xml = srv
        .get("/wfs?request=GetFeature&typename=cdf:Fifteen&version=1.1.0&service=wfs&resultType=hits")
        .await
        .xml();
    assert_eq!(
        xml.string("//wfs:FeatureCollection/@numberOfFeatures"),
        "15"
    );
}

/// GeoServer GetFeatureHitsIgnoreMaxFeaturesTest.testGetFeatureRespectsMaxFeatures
#[actix_web::test]
async fn gs_get_feature_hits_ignore_max_features_test_get_feature_respects_max_features() {
    let srv = server_with("count_default = 1").await;
    let xml = srv
        .get("/wfs?request=GetFeature&typename=cdf:Fifteen&version=1.1.0&service=wfs")
        .await
        .xml();
    xml.assert("/wfs:FeatureCollection");
    xml.assert_count("//cdf:Fifteen", 1);
}

// ---------------------------------------------------------------- GetFeatureBboxTest

/// GeoServer GetFeatureBboxTest.testFeatureBoudingOn (1.0 features carry boundedBy)
#[actix_web::test]
async fn gs_get_feature_bbox_test_feature_bouding_on() {
    // GeoServer test catalog runs with featureBounding enabled
    let srv = geoserver::server_with("feature_bounding = true").await;
    let xml = srv
        .get("/wfs?request=GetFeature&typeName=cite:Buildings&version=1.0.0&service=wfs&propertyName=ADDRESS")
        .await
        .xml();
    xml.assert_count("//wfs:FeatureCollection", 1);
    xml.assert_count("//wfs:FeatureCollection/gml:boundedBy/gml:Box", 1);
    assert!(xml.count(&format!("//{}/gml:boundedBy/gml:Box", cite("Buildings"))) > 0);
}

// ---------------------------------------------------------------- SrsNameTest

/// GeoServer SrsNameTest.testWfs10 (GML2 srsName in XML/URL form)
#[actix_web::test]
async fn gs_srs_name_test_wfs10() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?request=getfeature&service=wfs&version=1.0.0&typename=cgf:Points")
        .await
        .xml();
    xml.assert("/wfs:FeatureCollection");
    let names = xml.strings("//gml:Box/@srsName | //gml:Point/@srsName");
    assert!(!names.is_empty());
    // accepted difference: bbox writes the EPSG:<code> form in GML2 (GeoServer: XML/URL form)
    for n in names {
        assert!(
            n == "http://www.opengis.net/gml/srs/epsg.xml#32615" || n == "EPSG:32615",
            "{n}"
        );
    }
}

/// GeoServer SrsNameTest.testWfs11 (GML3 srsName as urn:x-ogc)
#[actix_web::test]
async fn gs_srs_name_test_wfs11() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?request=getfeature&service=wfs&version=1.1.0&typename=cgf:Points")
        .await
        .xml();
    xml.assert("/wfs:FeatureCollection");
    let names = xml.strings("//gml:Envelope/@srsName | //gml:Point/@srsName");
    assert!(!names.is_empty());
    // accepted difference: bbox writes the OGC URN form urn:ogc:def:crs:EPSG::<code>
    // (GeoServer: the legacy urn:x-ogc form)
    for n in names {
        assert!(
            n == "urn:x-ogc:def:crs:EPSG:32615" || n == "urn:ogc:def:crs:EPSG::32615",
            "{n}"
        );
    }
}

// ---------------------------------------------------------------- ReprojectionTest

/// Coordinates of the first gml:Box
fn first_box(xml: &Xml) -> Vec<(f64, f64)> {
    let text = xml.string("(//gml:Box)[1]/gml:coordinates");
    assert!(!text.is_empty(), "no gml:Box in\n{}", xml.text);
    text.split_whitespace()
        .map(|t| {
            let (x, y) = t.split_once(',').unwrap();
            (x.parse().unwrap(), y.parse().unwrap())
        })
        .collect()
}

fn to_3857(coords: &[(f64, f64)]) -> Vec<(f64, f64)> {
    coords
        .iter()
        .map(|(x, y)| {
            let mut g = geo::Geometry::Point(geo::Point::new(*x, *y));
            bbox_feature_server::wfs::crs::transform(&mut g, 32615, 3857).unwrap();
            let geo::Geometry::Point(p) = g else {
                unreachable!()
            };
            (p.x(), p.y())
        })
        .collect()
}

#[track_caller]
fn assert_reprojected(native: &Xml, reprojected: &Xml) {
    let expected = to_3857(&first_box(native));
    let got = first_box(reprojected);
    assert_eq!(expected.len(), got.len());
    for (e, g) in expected.iter().zip(&got) {
        assert!(
            (e.0 - g.0).abs() < 0.001 && (e.1 - g.1).abs() < 0.001,
            "{expected:?} vs {got:?}"
        );
    }
}

/// GeoServer ReprojectionTest.testGetFeatureGet (unqualified type name, srsName EPSG:900913)
#[actix_web::test]
async fn gs_reprojection_test_get_feature_get() {
    let srv = geoserver::server().await;
    let base = "/wfs?request=getfeature&service=wfs&version=1.0.0&typename=Polygons";
    let native = srv.get(base).await.xml();
    let reprojected = srv.get(&format!("{base}&srsName=EPSG:900913")).await.xml();
    assert_reprojected(&native, &reprojected);
}

/// GeoServer ReprojectionTest.testGetFeaturePost (wfs:PropertyName accepted in 1.0)
#[actix_web::test]
async fn gs_reprojection_test_get_feature_post() {
    let srv = geoserver::server().await;
    let q = |srs: &str| {
        format!(
            r#"<wfs:GetFeature service="WFS" version="1.0.0" xmlns:cgf="http://www.opengis.net/cite/geometry" xmlns:ogc="http://www.opengis.net/ogc" xmlns:wfs="http://www.opengis.net/wfs"><wfs:Query {srs}typeName="cgf:Polygons"><wfs:PropertyName>cgf:polygonProperty</wfs:PropertyName></wfs:Query></wfs:GetFeature>"#
        )
    };
    let native = srv.post_wfs(&q("")).await.xml();
    let reprojected = srv.post_wfs(&q(r#"srsName="EPSG:900913" "#)).await.xml();
    assert_reprojected(&native, &reprojected);
}

/// Native bounds of cgf:Polygons transformed to EPSG:3857 (minx, miny, maxx, maxy)
async fn projected_bounds(srv: &TestServer) -> (f64, f64, f64, f64) {
    let native = srv
        .get("/wfs?request=getfeature&service=wfs&version=1.0.0&typename=cgf:Polygons")
        .await
        .xml();
    let c = to_3857(&first_box(&native));
    let xs = c.iter().map(|p| p.0);
    let ys = c.iter().map(|p| p.1);
    (
        xs.clone().fold(f64::INFINITY, f64::min),
        ys.clone().fold(f64::INFINITY, f64::min),
        xs.fold(f64::NEG_INFINITY, f64::max),
        ys.fold(f64::NEG_INFINITY, f64::max),
    )
}

/// GeoServer ReprojectionTest.testGetFeatureWithProjectedBoxGet (bbox CRS, version=1.0)
#[actix_web::test]
async fn gs_reprojection_test_get_feature_with_projected_box_get() {
    let srv = geoserver::server().await;
    let (x1, y1, x2, y2) = projected_bounds(&srv).await;
    let xml = srv
        .get(&format!("/wfs?request=getfeature&service=wfs&version=1.0&typeName=Polygons&bbox={x1},{y1},{x2},{y2},EPSG:900913"))
        .await
        .xml();
    xml.assert_count("//cgf:Polygons", 1);
}

/// GeoServer ReprojectionTest.testGetFeatureWithProjectedBoxPost
#[actix_web::test]
async fn gs_reprojection_test_get_feature_with_projected_box_post() {
    let srv = geoserver::server().await;
    let (x1, y1, x2, y2) = projected_bounds(&srv).await;
    let body = format!(
        r#"<wfs:GetFeature service="WFS" version="1.0.0" xmlns:cgf="http://www.opengis.net/cite/geometry" xmlns:ogc="http://www.opengis.net/ogc" xmlns:gml="http://www.opengis.net/gml" xmlns:wfs="http://www.opengis.net/wfs"><wfs:Query typeName="cgf:Polygons"><ogc:Filter><ogc:BBOX><ogc:PropertyName>polygonProperty</ogc:PropertyName><gml:Box srsName="EPSG:900913"><gml:coord><gml:X>{x1}</gml:X><gml:Y>{y1}</gml:Y></gml:coord><gml:coord><gml:X>{x2}</gml:X><gml:Y>{y2}</gml:Y></gml:coord></gml:Box></ogc:BBOX></ogc:Filter></wfs:Query></wfs:GetFeature>"#
    );
    srv.post_wfs(&body)
        .await
        .xml()
        .assert_count("//cgf:Polygons", 1);
}

/// GeoServer ReprojectionTest.testGetFeatureWithProjectedBoxIntersectsPost (filter geometry
/// without srsName is in the Query srsName)
#[actix_web::test]
async fn gs_reprojection_test_get_feature_with_projected_box_intersects_post() {
    let srv = geoserver::server().await;
    let (x1, y1, x2, y2) = projected_bounds(&srv).await;
    let body = format!(
        r#"<wfs:GetFeature service="WFS" version="1.0.0" xmlns:cgf="http://www.opengis.net/cite/geometry" xmlns:ogc="http://www.opengis.net/ogc" xmlns:gml="http://www.opengis.net/gml" xmlns:wfs="http://www.opengis.net/wfs"><wfs:Query typeName="cgf:Polygons" srsName="EPSG:900913"><ogc:Filter><ogc:Intersects><ogc:PropertyName>polygonProperty</ogc:PropertyName><gml:Box><gml:coord><gml:X>{x1}</gml:X><gml:Y>{y1}</gml:Y></gml:coord><gml:coord><gml:X>{x2}</gml:X><gml:Y>{y2}</gml:Y></gml:coord></gml:Box></ogc:Intersects></ogc:Filter></wfs:Query></wfs:GetFeature>"#
    );
    srv.post_wfs(&body)
        .await
        .xml()
        .assert_count("//cgf:Polygons", 1);
}

// ---------------------------------------------------------------- GeometrylessTest

/// GeoServer GeometrylessTest.testGetFeature10
#[actix_web::test]
async fn gs_geometryless_test_get_feature10() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?request=GetFeature&typename=cite:Geometryless&version=1.0.0&service=wfs")
        .await
        .xml();
    xml.assert("/wfs:FeatureCollection");
    xml.assert_count("//gml:featureMember", 3);
}

/// GeoServer GeometrylessTest.testGetFeatureReproject10
#[actix_web::test]
async fn gs_geometryless_test_get_feature_reproject10() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?request=GetFeature&typename=cite:Geometryless&version=1.0.0&service=wfs&srsName=EPSG:900913")
        .await
        .xml();
    xml.assert_count("//gml:featureMember", 3);
}

/// GeoServer GeometrylessTest.testGetFeature11 (GML3 featureMembers container)
#[actix_web::test]
async fn gs_geometryless_test_get_feature11() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?request=GetFeature&typename=cite:Geometryless&version=1.1.0&service=wfs")
        .await
        .xml();
    xml.assert("/wfs:FeatureCollection");
    // accepted difference: bbox uses one gml:featureMember per feature (GeoServer: gml:featureMembers)
    assert!(
        xml.count("//gml:featureMembers | //gml:featureMember") > 0,
        "{}",
        xml.text
    );
    xml.assert_count(&format!("//{}", cite("Geometryless")), 3);
}

/// GeoServer GeometrylessTest.testGetFeatureReproject11
#[actix_web::test]
async fn gs_geometryless_test_get_feature_reproject11() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?request=GetFeature&typename=cite:Geometryless&version=1.1.0&service=wfs&srsName=EPSG:900913")
        .await
        .xml();
    // accepted difference: bbox uses one gml:featureMember per feature (GeoServer: gml:featureMembers)
    assert!(
        xml.count("//gml:featureMembers | //gml:featureMember") > 0,
        "{}",
        xml.text
    );
    xml.assert_count(&format!("//{}", cite("Geometryless")), 3);
}

/// GeoServer GeometrylessTest.testGetFeatureReprojectPost
#[actix_web::test]
async fn gs_geometryless_test_get_feature_reproject_post() {
    let srv = geoserver::server().await;
    let body = r#"<wfs:GetFeature service="WFS" version="1.0.0" outputFormat="GML2" xmlns:cite="http://www.opengis.net/cite" xmlns:wfs="http://www.opengis.net/wfs" xmlns:ogc="http://www.opengis.net/ogc" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:schemaLocation="http://www.opengis.net/wfs http://schemas.opengis.net/wfs/1.0.0/WFS-basic.xsd"><wfs:Query typeName="cite:Geometryless" srsName="EPSG:900913"/></wfs:GetFeature>"#;
    let xml = srv.post_wfs(body).await.xml();
    xml.assert_count("//gml:featureMember", 3);
    xml.assert_count(&format!("//{}", cite("Geometryless")), 3);
}

// ---------------------------------------------------------------- NumDecimalsTest (defaults)

/// GeoServer NumDecimalsTest.testDefaults (featureid only; coordinates not padded)
#[actix_web::test]
async fn gs_num_decimals_test_defaults() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?request=getfeature&featureid=PrimitiveGeoFeature.f008&version=1.0.0")
        .await
        .xml();
    xml.assert_count("//gml:featureMember", 1);
    let coords = xml.strings("//sf:PrimitiveGeoFeature//gml:coordinates");
    assert!(!coords.is_empty(), "{}", xml.text);
    for c in coords
        .iter()
        .flat_map(|c| c.split([' ', ',']).map(str::to_string).collect::<Vec<_>>())
    {
        let decimals = c.split_once('.').map(|(_, d)| d.len()).unwrap_or(0);
        assert_eq!(decimals, 3, "{c} in {coords:?}");
    }
}

/// GeoServer NumDecimalsTest.testMultipleFeatureTypes (featureid of several types, no typename)
#[actix_web::test]
async fn gs_num_decimals_test_multiple_feature_types() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?request=getfeature&featureid=PrimitiveGeoFeature.f008,AggregateGeoFeature.f009&version=1.0.0")
        .await
        .xml();
    xml.assert_count("//gml:featureMember", 2);
    xml.assert_count("//sf:PrimitiveGeoFeature", 1);
    xml.assert_count("//sf:AggregateGeoFeature", 1);
    for c in xml.strings("//gml:featureMember//gml:coordinates") {
        for v in c.split([' ', ',']) {
            assert_eq!(
                v.split_once('.').map(|(_, d)| d.len()).unwrap_or(0),
                3,
                "{v}"
            );
        }
    }
}

// ---------------------------------------------------------------- GMLOutputFormatTest

/// GeoServer GMLOutputFormatTest.testGML2 (GML2 requested in 1.0 and 1.1)
#[actix_web::test]
async fn gs_gml_output_format_test_gml2() {
    let srv = geoserver::server().await;
    for version in ["1.0.0", "1.1.0"] {
        for format in ["gml2", "text/xml;%20subtype%3Dgml/2.1.2"] {
            let r = srv
                .get(&format!("/wfs?request=getfeature&version={version}&outputFormat={format}&typename=cite:BasicPolygons"))
                .await;
            let xml = r.xml();
            xml.assert("/*[local-name()='FeatureCollection']");
            assert!(
                xml.count("//gml:outerBoundaryIs") > 0,
                "{version} {format}:\n{}",
                r.body
            );
            xml.assert_count("//gml:exterior", 0);
        }
    }
}

/// GeoServer GMLOutputFormatTest.testGML3 (GML3 requested in 1.0 and 1.1)
#[actix_web::test]
async fn gs_gml_output_format_test_gml3() {
    let srv = geoserver::server().await;
    for version in ["1.0.0", "1.1.0"] {
        for format in ["gml3", "text/xml;%20subtype%3Dgml/3.1.1"] {
            let r = srv
                .get(&format!("/wfs?request=getfeature&version={version}&outputFormat={format}&typename=cite:BasicPolygons"))
                .await;
            let xml = r.xml();
            xml.assert("/*[local-name()='FeatureCollection']");
            assert!(
                xml.count("//gml:exterior") > 0,
                "{version} {format}:\n{}",
                r.body
            );
            xml.assert_count("//gml:outerBoundaryIs", 0);
        }
    }
}

// ---------------------------------------------------------------- ExternalEntitiesTest

/// GeoServer ExternalEntitiesTest.testWfs1_0 (DOCTYPE / external entity rejected)
#[actix_web::test]
async fn gs_external_entities_test_wfs1_0() {
    let srv = geoserver::server().await;
    let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE wfs:GetFeature [<!ENTITY c SYSTEM "FILE:///this/file/does/not/exist?.XSD">]>
<wfs:GetFeature service="WFS" version="1.0.0" outputFormat="GML2" xmlns:cdf="http://www.opengis.net/cite/data" xmlns:wfs="http://www.opengis.net/wfs" xmlns:ogc="http://www.opengis.net/ogc" xmlns:gml="http://www.opengis.net/gml"><wfs:Query typeName="cdf:Fifteen"><ogc:Filter><ogc:BBOX><ogc:PropertyName>the_geom</ogc:PropertyName><gml:Box srsName="http://www.opengis.net/gml/srs/epsg.xml#4326"><gml:coordinates>-75.102613,40.212597 -72.361859,41.512517</gml:coordinates></gml:Box></ogc:BBOX></ogc:Filter><ogc:Literal>&c;</ogc:Literal></wfs:Query></wfs:GetFeature>"#;
    let r = srv.post_wfs(body).await;
    let xml = r.xml();
    xml.assert("/*[local-name()='ServiceExceptionReport']");
    xml.assert_count("//*[local-name()='ServiceException']", 1);
}

/// GeoServer ExternalEntitiesTest.testWfs1_1
#[actix_web::test]
async fn gs_external_entities_test_wfs1_1() {
    let srv = geoserver::server().await;
    let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE wfs:GetFeature [<!ELEMENT wfs:GetFeature (wfs:Query*)><!ATTLIST wfs:GetFeature service CDATA #FIXED "WFS" version CDATA #FIXED "1.1.0" xmlns:wfs CDATA #FIXED "http://www.opengis.net/wfs" xmlns:ogc CDATA #FIXED "http://www.opengis.net/ogc"><!ELEMENT wfs:Query (wfs:PropertyName*,ogc:Filter?)><!ATTLIST wfs:Query typeName CDATA #FIXED "cdf:Fifteen"><!ELEMENT wfs:PropertyName (#PCDATA)><!ELEMENT ogc:Filter (ogc:FeatureId*)><!ELEMENT ogc:FeatureId EMPTY><!ATTLIST ogc:FeatureId fid CDATA #FIXED "states.3"><!ENTITY passwd SYSTEM "FILE:///etc/passwd">]>
<wfs:GetFeature service="WFS" version="1.1.0" xmlns:wfs="http://www.opengis.net/wfs" xmlns:ogc="http://www.opengis.net/ogc"><wfs:Query typeName="cdf:Fifteen"><wfs:PropertyName>&passwd;</wfs:PropertyName><ogc:Filter><ogc:FeatureId fid="states.3"/></ogc:Filter></wfs:Query></wfs:GetFeature>"#;
    let r = srv.post_wfs(body).await;
    assert!(!r.body.contains("root:"), "entity resolved: {}", r.body);
    r.xml().assert("//ows:ExceptionText");
}

/// GeoServer ExternalEntitiesTest.testKvpEntityExpansion
#[actix_web::test]
async fn gs_external_entities_test_kvp_entity_expansion() {
    let srv = geoserver::server().await;
    let filter = r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE foo [<!ELEMENT foo ANY ><!ENTITY xxe SYSTEM "file:///etc/passwd" >]><Filter xmlns="http://www.opengis.net/ogc"><PropertyIsEqualTo><PropertyName>&xxe;</PropertyName><Literal>BAR</Literal></PropertyIsEqualTo></Filter>"#;
    let r = srv
        .kvp(&[
            ("request", "GetFeature"),
            ("SERVICE", "WFS"),
            ("VERSION", "1.0.0"),
            ("TYPENAME", "cdf:Fifteen"),
            ("FILTER", filter),
        ])
        .await;
    assert!(!r.body.contains("root:"), "entity resolved: {}", r.body);
    r.xml().assert("//ogc:ServiceException");
}
