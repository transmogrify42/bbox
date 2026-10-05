//! Ported GeoServer WFS 2.0 tests: GetCapabilities, DescribeFeatureType, srsName handling,
//! exceptions and miscellaneous behaviour (GeoServer `wfs2_x` module, read-only parts).
//!
//! Independent re-implementations of the behaviour asserted by GeoServer's tests, run against
//! the GeoServer test catalog (cite, cdf, cgf, sf) loaded by `common::geoserver`.
//! Requests to GeoServer's generic `ows` endpoint are sent to `/wfs`.
//!
//! Not ported:
//! - TransactionTest, TransactionCurveTest, GetFeatureWithLockTest, LockFeatureTest,
//!   TransactionCallbackWFS20Test: write / locking operations (read-only server). Read-relevant
//!   parts of testInsertWithNoSRS and testUpdateOnSimpleXPath are ported as query-only tests.
//! - GetCapabilitiesTest: testOperationsMetadataWithDisabledStoredQueryManagement (config
//!   switch), testLayerQualified (virtual services), testMetadataLinks /
//!   testMetadataLinksTransormToProxyBaseURL (per-layer metadata links),
//!   testOtherSRSSingleTypeOverride (per-layer SRS override), testDisableLocking
//!   (TRANSACTIONAL service level), testInternationalContent*, testAcceptLanguages*,
//!   testNullLocale (i18n configuration), testIauFeatureTypes (iau:MarsPoi not loaded),
//!   testOutputFormats (GeoServer's registered format list; replaced by a check of the formats
//!   bbox advertises).
//! - DescribeFeatureTypeTest: testUserSuppliedTypeNameNamespaceWithVirtualService (virtual
//!   service), strict CITE mode halves of testCiteCompliance (config switch),
//!   testGMLAttributeMapping (overrideGMLAttributes config), testCustomizeFeatureType,
//!   testFeatureTypeWithAttributeRestrictions(+JsonOutputFormat) (attribute customization),
//!   describePostGISTable (PostGIS column comments), the Content-Disposition and forced MIME
//!   parts of testGet / testPost.
//! - GMLOutputFormatTest: testGML32CoordinatesFormatting (per-layer numDecimals),
//!   testGML32InvalidElementName / InvalidNamespaceUri / InvalidNamespacePrefix (extra layers
//!   with invalid names not in the default catalog).
//! - GetFeatureCachingTest (re-runs the WFS 1.x GetFeature tests with an internal cache),
//!   WFSURIHandlerTest (internal schema URI handler), WfsRemoteStoreTest testAddRemoteWfsLayer*
//!   (cascaded remote store; the request style of GeoServer's WFS client is ported instead).

mod common;
use common::*;

const W: &str = "/wfs?service=WFS&version=2.0.0&request=";
const WFS20_XSD: &str = "http://schemas.opengis.net/wfs/2.0/wfs.xsd";
const GML32_NS: &str = "http://www.opengis.net/gml/3.2";

async fn server() -> TestServer {
    geoserver::server().await
}

/// All feature types of the catalog (prefixed names)
fn type_names() -> Vec<String> {
    geoserver::NAMESPACES
        .iter()
        .flat_map(|ns| ns.types.iter().map(move |t| format!("{}:{t}", ns.prefix)))
        .collect()
}

/// OWS 1.1 exception report of WFS 2.0 with code and (optional) locator
#[track_caller]
fn check_ows11_exception(r: &Response, code: &str, locator: Option<&str>) {
    let xml = r.xml();
    xml.assert("/ows11:ExceptionReport[@version='2.0.0']");
    xml.assert_count("//ows11:Exception", 1);
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

/// Schema returned by DescribeFeatureType contains the given types and imports GML 3.2
#[track_caller]
fn assert_schema(xml: &Xml, types: &[&str]) {
    xml.assert("/*[local-name()='schema' and namespace-uri()='http://www.w3.org/2001/XMLSchema']");
    xml.assert(&format!("//xs:import[@namespace='{GML32_NS}']"));
    for t in types {
        xml.assert_count(&format!("//xs:complexType[@name='{t}Type']"), 1);
        xml.assert_count(&format!("/xs:schema/xs:element[@name='{t}']"), 1);
    }
}

fn soap12(body: &str) -> String {
    format!(
        r#"<soap:Envelope xmlns:soap="http://www.w3.org/2003/05/soap-envelope"><soap:Header/><soap:Body>{body}</soap:Body></soap:Envelope>"#
    )
}

fn base64_decode(s: &str) -> Vec<u8> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
        let v = T.iter().position(|t| *t == c).expect("base64 character") as u32;
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

// ---------------------------------------------------------------- GetCapabilitiesTest

/// GetCapabilitiesTest.testGet
#[actix_web::test]
async fn gs_get_capabilities_test_get() {
    let srv = server().await;
    let r = srv
        .get("/wfs?service=WFS&request=getCapabilities&version=2.0.0")
        .await;
    r.assert_ok();
    let xml = r.xml();
    xml.assert("/wfs2:WFS_Capabilities[@version='2.0.0']");
    assert!(xml.count("//wfs2:FeatureType") > 0);
    for c in ["KVPEncoding", "XMLEncoding", "SOAPEncoding"] {
        assert_eq!(
            xml.string(&format!(
                "//ows11:OperationsMetadata/ows11:Constraint[@name='{c}']/ows11:DefaultValue"
            )),
            "TRUE",
            "{c}"
        );
    }
}

/// GetCapabilitiesTest.testPost
#[actix_web::test]
async fn gs_get_capabilities_test_post() {
    let srv = server().await;
    let r = srv
        .post_wfs(r#"<GetCapabilities service="WFS" xmlns="http://www.opengis.net/wfs/2.0" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:schemaLocation="http://www.opengis.net/wfs/2.0 http://schemas.opengis.net/wfs/2.0/wfs.xsd"/>"#)
        .await;
    r.assert_ok();
    // without a version the highest 2.0 version is negotiated (GeoServer: 2.0.0, bbox: 2.0.2)
    r.xml()
        .assert("/wfs2:WFS_Capabilities[starts-with(@version,'2.0.')]");
}

/// GetCapabilitiesTest.testNamespaceFilter (GeoServer vendor parameter `namespace`)
#[actix_web::test]
async fn gs_get_capabilities_test_namespace_filter() {
    let srv = server().await;
    let xml = srv
        .get("/wfs?service=WFS&version=2.0.0&request=getCapabilities&namespace=sf")
        .await
        .xml();
    xml.assert("/*[local-name()='WFS_Capabilities']");
    let names = xml.strings("//wfs2:FeatureType/wfs2:Name");
    assert!(!names.is_empty());
    assert!(names.iter().all(|n| n.starts_with("sf:")), "{names:?}");
    let xml = srv
        .get("/wfs?service=WFS&request=getCapabilities&namespace=NotThere")
        .await
        .xml();
    xml.assert_count("//wfs2:FeatureType", 0);
}

/// GetCapabilitiesTest.testPostNoSchemaLocation
#[actix_web::test]
async fn gs_get_capabilities_test_post_no_schema_location() {
    let srv = server().await;
    let r = srv
        .post_wfs(r#"<GetCapabilities service="WFS" version='2.0.0' xmlns="http://www.opengis.net/wfs/2.0" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"/>"#)
        .await;
    r.assert_ok();
    r.xml().assert("/wfs2:WFS_Capabilities[@version='2.0.0']");
}

/// GetCapabilitiesTest.testOutputFormats (bbox format list instead of GeoServer's)
#[actix_web::test]
async fn gs_get_capabilities_test_output_formats() {
    let srv = server().await;
    let xml = srv.get(&format!("{W}GetCapabilities")).await.xml();
    let formats = xml.strings("//ows11:Operation[@name='GetFeature']/ows11:Parameter[@name='outputFormat']/ows11:AllowedValues/ows11:Value");
    for f in ["application/gml+xml; version=3.2", "application/json"] {
        assert!(formats.iter().any(|v| v == f), "{f} not in {formats:?}");
    }
    // every advertised format works
    for f in &formats {
        let r = srv
            .kvp(&[
                ("service", "WFS"),
                ("version", "2.0.0"),
                ("request", "GetFeature"),
                ("typeNames", "sf:PrimitiveGeoFeature"),
                ("outputFormat", f),
            ])
            .await;
        assert_eq!(r.status, 200, "outputFormat {f}: {}", r.body);
    }
}

/// GetCapabilitiesTest.testResolveParameter
#[actix_web::test]
async fn gs_get_capabilities_test_resolve_parameter() {
    let srv = server().await;
    let xml = srv.get(&format!("{W}GetCapabilities")).await.xml();
    for op in ["GetFeature", "GetPropertyValue"] {
        for v in ["none", "local"] {
            // the parameter may be declared on the operation or for all operations
            let on_op = format!("//ows11:Operation[@name='{op}']/ows11:Parameter[@name='resolve']/ows11:AllowedValues[ows11:Value='{v}']");
            let global = format!("//ows11:OperationsMetadata/ows11:Parameter[@name='resolve']/ows11:AllowedValues[ows11:Value='{v}']");
            assert!(
                xml.count(&on_op) + xml.count(&global) > 0,
                "{op} resolve {v}"
            );
        }
    }
}

/// GetCapabilitiesTest.testSupportedSpatialOperators
#[actix_web::test]
async fn gs_get_capabilities_test_supported_spatial_operators() {
    let srv = server().await;
    let xml = srv.get(&format!("{W}GetCapabilities")).await.xml();
    let mut ops =
        xml.strings("//fes:Spatial_Capabilities/fes:SpatialOperators/fes:SpatialOperator/@name");
    ops.sort();
    let mut expected = vec![
        "BBOX",
        "Beyond",
        "Contains",
        "Crosses",
        "DWithin",
        "Disjoint",
        "Equals",
        "Intersects",
        "Overlaps",
        "Touches",
        "Within",
    ];
    expected.sort();
    assert_eq!(ops, expected);
}

/// GetCapabilitiesTest.testBasicWFSFesConstraints
#[actix_web::test]
async fn gs_get_capabilities_test_basic_wfs_fes_constraints() {
    let srv = server().await;
    let xml = srv.get(&format!("{W}GetCapabilities")).await.xml();
    for c in [
        "ImplementsAdHocQuery",
        "ImplementsResourceId",
        "ImplementsMinStandardFilter",
        "ImplementsStandardFilter",
        "ImplementsMinSpatialFilter",
        "ImplementsSpatialFilter",
        "ImplementsSorting",
        "ImplementsMinimumXPath",
    ] {
        assert_eq!(
            xml.string(&format!("//fes:Constraint[@name='{c}']/ows11:DefaultValue")),
            "TRUE",
            "{c}"
        );
    }
}

/// GetCapabilitiesTest.testFunctionArgCount
#[actix_web::test]
async fn gs_get_capabilities_test_function_arg_count() {
    let srv = server().await;
    let xml = srv.get(&format!("{W}GetCapabilities")).await.xml();
    xml.assert_count("//fes:Function[@name='abs']/fes:Arguments/fes:Argument", 1);
}

/// GetCapabilitiesTest.testTypeNameCount
#[actix_web::test]
async fn gs_get_capabilities_test_type_name_count() {
    let srv = server().await;
    let xml = srv.get(&format!("{W}GetCapabilities")).await.xml();
    xml.assert_count(
        "/wfs2:WFS_Capabilities/wfs2:FeatureTypeList/wfs2:FeatureType",
        type_names().len(),
    );
    assert_eq!(type_names().len(), 29);
}

/// GetCapabilitiesTest.testTypeNames
#[actix_web::test]
async fn gs_get_capabilities_test_type_names() {
    let srv = server().await;
    let xml = srv.get(&format!("{W}GetCapabilities")).await.xml();
    for n in type_names() {
        xml.assert(&format!(
            "/wfs2:WFS_Capabilities/wfs2:FeatureTypeList/wfs2:FeatureType/wfs2:Name[text()='{n}']"
        ));
    }
}

/// GetCapabilitiesTest.testOperationsMetadata (read-only operation set)
#[actix_web::test]
async fn gs_get_capabilities_test_operations_metadata() {
    let srv = server().await;
    let xml = srv.get(&format!("{W}GetCapabilities")).await.xml();
    for op in [
        "GetCapabilities",
        "DescribeFeatureType",
        "GetFeature",
        "GetPropertyValue",
        "ListStoredQueries",
        "DescribeStoredQueries",
        "CreateStoredQuery",
        "DropStoredQuery",
    ] {
        xml.assert(&format!("//ows11:Operation[@name='{op}']"));
    }
}

/// GetCapabilitiesTest.testValidCapabilitiesDocument
#[actix_web::test]
async fn gs_get_capabilities_test_valid_capabilities_document() {
    let srv = server().await;
    let xml = srv.get(&format!("{W}GetCapabilities")).await.xml();
    xml.assert_valid(WFS20_XSD);
}

/// GetCapabilitiesTest.testSOAP
#[actix_web::test]
async fn gs_get_capabilities_test_soap() {
    let srv = server().await;
    let body = soap12(
        r#"<wfs:GetCapabilities service="WFS" xmlns:wfs="http://www.opengis.net/wfs/2.0"/>"#,
    );
    let r = srv
        .post_with_type("/wfs", &body, "application/soap+xml")
        .await;
    r.assert_ok();
    assert!(
        r.content_type.starts_with("application/soap+xml"),
        "{}",
        r.content_type
    );
    let xml = r.xml();
    xml.assert(
        "/*[local-name()='Envelope' and namespace-uri()='http://www.w3.org/2003/05/soap-envelope']",
    );
    xml.assert_count("//wfs2:WFS_Capabilities", 1);
}

/// GetCapabilitiesTest.testAcceptVersions11WithVersion
#[actix_web::test]
async fn gs_get_capabilities_test_accept_versions11_with_version() {
    let srv = server().await;
    let xml = srv
        .get("/wfs?service=WFS&request=GetCapabilities&version=2.0.0&acceptversions=1.1.0,1.0.0")
        .await
        .xml();
    xml.assert("/wfs:WFS_Capabilities[@version='1.1.0']");
}

/// GetCapabilitiesTest.testAcceptFormats (OWS 1.1 default format is text/xml; GeoServer
/// returns application/xml by default)
#[actix_web::test]
async fn gs_get_capabilities_test_accept_formats() {
    let srv = server().await;
    let r = srv
        .get("/wfs?service=WFS&request=GetCapabilities&version=2.0.0")
        .await;
    assert!(
        r.content_type.starts_with("text/xml") || r.content_type.starts_with("application/xml"),
        "{}",
        r.content_type
    );
    let r = srv
        .get("/wfs?service=WFS&request=GetCapabilities&version=2.0.0&acceptformats=text/xml")
        .await;
    assert!(r.content_type.starts_with("text/xml"), "{}", r.content_type);
    let r = srv
        .get("/wfs?service=WFS&request=GetCapabilities&version=2.0.0&acceptformats=application/xml")
        .await;
    r.assert_ok();
    assert!(
        r.content_type.starts_with("application/xml"),
        "{}",
        r.content_type
    );
}

/// GetCapabilitiesTest.testGetPropertyValueFormat
#[actix_web::test]
async fn gs_get_capabilities_test_get_property_value_format() {
    let srv = server().await;
    let xml = srv.get(&format!("{W}GetCapabilities")).await.xml();
    assert_eq!(
        xml.string("//ows11:Operation[@name='GetPropertyValue']/ows11:Parameter[@name='outputFormat']/ows11:AllowedValues/ows11:Value[1]"),
        "application/gml+xml; version=3.2"
    );
}

/// GetCapabilitiesTest.testCreateStoredQuery
#[actix_web::test]
async fn gs_get_capabilities_test_create_stored_query() {
    let srv = server().await;
    let xml = srv.get(&format!("{W}GetCapabilities")).await.xml();
    assert_eq!(
        xml.string("//ows11:Operation[@name='CreateStoredQuery']/ows11:Parameter[@name='language']/ows11:AllowedValues/ows11:Value[1]"),
        "urn:ogc:def:queryLanguage:OGC-WFS::WFSQueryExpression"
    );
}

/// GetCapabilitiesTest.testOtherCRS (service CRS list from the bbox configuration:
/// 4326, 3857, 32615, 4269; the native CRS is never repeated)
#[actix_web::test]
async fn gs_get_capabilities_test_other_crs() {
    let srv = server().await;
    let xml = srv.get(&format!("{W}GetCapabilities")).await.xml();
    let configured = [4326, 3857, 32615, 4269];
    let mut checked = 0;
    for n in type_names() {
        let ft = format!("//wfs2:FeatureType[wfs2:Name='{n}']");
        let default = xml.string(&format!("{ft}/wfs2:DefaultCRS"));
        if default.is_empty() {
            continue;
        }
        let others = xml.strings(&format!("{ft}/wfs2:OtherCRS"));
        assert!(!others.contains(&default), "{n}: {others:?}");
        for o in &others {
            assert!(o.starts_with("urn:ogc:def:crs:EPSG::"), "{o}");
        }
        let native: u16 = default.rsplit(':').next().unwrap().parse().unwrap();
        let expected = configured.len() - configured.contains(&native) as usize;
        assert_eq!(others.len(), expected, "{n}: {others:?}");
        checked += 1;
    }
    assert!(checked >= 28);
}

/// GetCapabilitiesTest.testGetSections
#[actix_web::test]
async fn gs_get_capabilities_test_get_sections() {
    let srv = server().await;
    let parts = [
        "//ows11:ServiceIdentification",
        "//ows11:ServiceProvider",
        "//ows11:OperationsMetadata",
        "//wfs2:FeatureTypeList",
        "//fes:Filter_Capabilities",
    ];
    for (sections, expected) in [
        ("", [1, 1, 1, 1, 1]),
        ("All", [1, 1, 1, 1, 1]),
        ("ServiceIdentification", [1, 0, 0, 0, 0]),
        ("ServiceProvider", [0, 1, 0, 0, 0]),
        ("OperationsMetadata", [0, 0, 1, 0, 0]),
        ("FeatureTypeList", [0, 0, 0, 1, 0]),
        ("Filter_Capabilities", [0, 0, 0, 0, 1]),
        ("ServiceIdentification,Filter_Capabilities", [1, 0, 0, 0, 1]),
        (
            "ServiceIdentification,Filter_Capabilities,All",
            [1, 1, 1, 1, 1],
        ),
    ] {
        let xml = srv
            .get(&format!("{W}GetCapabilities&sections={sections}"))
            .await
            .xml();
        let counts: Vec<usize> = parts.iter().map(|p| xml.count(p)).collect();
        assert_eq!(counts, expected, "sections={sections}");
    }
    let r = srv
        .get(&format!("{W}GetCapabilities&sections=FooBar"))
        .await;
    check_ows11_exception(&r, "InvalidParameterValue", Some("sections"));
}

/// GetCapabilitiesTest.testDisableTransaction (BASIC service level = read-only profile)
#[actix_web::test]
async fn gs_get_capabilities_test_disable_transaction() {
    let srv = server().await;
    let xml = srv.get(&format!("{W}GetCapabilities")).await.xml();
    assert_eq!(
        xml.string("//ows11:Constraint[@name='ImplementsTransactionalWFS']/ows11:DefaultValue"),
        "FALSE"
    );
    assert_eq!(
        xml.string("//ows11:Constraint[@name='ImplementsLockingWFS']/ows11:DefaultValue"),
        "FALSE"
    );
    for op in ["Transaction", "LockFeature", "GetFeatureWithLock"] {
        xml.assert_not(&format!("//ows11:Operation[@name='{op}']"));
    }
}

/// GetCapabilitiesTest.testCiteCompliant
#[actix_web::test]
async fn gs_get_capabilities_test_cite_compliant() {
    let srv = geoserver::server_with("cite_compliant = true").await;
    // VERSION is not required for GetCapabilities
    let r = srv.get("/wfs?service=WFS&request=GetCapabilities").await;
    r.assert_ok();
    r.xml().assert("/*[local-name()='WFS_Capabilities']");
    // SERVICE is required
    let r = srv.get("/wfs?request=GetCapabilities&version=2.0.0").await;
    assert_eq!(r.status, 400, "{}", r.body);
    check_ows11_exception(&r, "MissingParameterValue", Some("service"));
    r.xml().assert_count("//ows11:ExceptionText", 1);
}

// ---------------------------------------------------------------- DescribeFeatureTypeTest

/// DescribeFeatureTypeTest.testGet
#[actix_web::test]
async fn gs_describe_feature_type_test_get() {
    let srv = server().await;
    let r = srv
        .get(&format!(
            "{W}DescribeFeatureType&typeName=sf:PrimitiveGeoFeature"
        ))
        .await;
    r.assert_ok();
    assert_eq!(r.content_type, "application/gml+xml; version=3.2");
    assert_schema(&r.xml(), &["PrimitiveGeoFeature"]);
}

/// DescribeFeatureTypeTest.testGetPluralKey
#[actix_web::test]
async fn gs_describe_feature_type_test_get_plural_key() {
    let srv = server().await;
    let r = srv
        .get(&format!(
            "{W}DescribeFeatureType&typeNames=sf:PrimitiveGeoFeature"
        ))
        .await;
    r.assert_ok();
    assert_eq!(r.content_type, "application/gml+xml; version=3.2");
    assert_schema(&r.xml(), &["PrimitiveGeoFeature"]);
}

/// DescribeFeatureTypeTest.testConcurrentGet
#[actix_web::test]
async fn gs_describe_feature_type_test_concurrent_get() {
    let srv = server().await;
    let path = format!("{W}DescribeFeatureType&typeName=sf:PrimitiveGeoFeature");
    let futs = (0..50).map(|_| srv.get(&path));
    for r in futures::future::join_all(futs).await {
        r.assert_ok();
        assert_schema(&r.xml(), &["PrimitiveGeoFeature"]);
    }
}

const DFT_POST: &str = "<wfs:DescribeFeatureType service='WFS' version='2.0.0' xmlns:wfs='http://www.opengis.net/wfs/2.0' xmlns:sf='http://cite.opengeospatial.org/gmlsf'><wfs:TypeName>sf:PrimitiveGeoFeature</wfs:TypeName></wfs:DescribeFeatureType>";

/// DescribeFeatureTypeTest.testPost
#[actix_web::test]
async fn gs_describe_feature_type_test_post() {
    let srv = server().await;
    let r = srv.post_wfs(DFT_POST).await;
    r.assert_ok();
    assert_eq!(r.content_type, "application/gml+xml; version=3.2");
    assert_schema(&r.xml(), &["PrimitiveGeoFeature"]);
}

/// DescribeFeatureTypeTest.testConcurrentPost
#[actix_web::test]
async fn gs_describe_feature_type_test_concurrent_post() {
    let srv = server().await;
    let futs = (0..50).map(|_| srv.post_wfs(DFT_POST));
    for r in futures::future::join_all(futs).await {
        r.assert_ok();
        assert_schema(&r.xml(), &["PrimitiveGeoFeature"]);
    }
}

/// DescribeFeatureTypeTest.testDateMappings
#[actix_web::test]
async fn gs_describe_feature_type_test_date_mappings() {
    let srv = server().await;
    let xml = srv.post_wfs(DFT_POST).await.xml();
    let local = |name: &str| {
        let t = xml.string(&format!("//xs:element[@name='{name}']/@type"));
        t.rsplit(':').next().unwrap_or("").to_string()
    };
    assert_eq!(local("dateProperty"), "date");
    assert_eq!(local("dateTimeProperty"), "dateTime");
}

/// DescribeFeatureTypeTest.testNoNamespaceDeclaration
#[actix_web::test]
async fn gs_describe_feature_type_test_no_namespace_declaration() {
    let srv = server().await;
    let r = srv
        .post_wfs("<wfs:DescribeFeatureType service='WFS' version='2.0.0' xmlns:wfs='http://www.opengis.net/wfs/2.0'><wfs:TypeName>sf:PrimitiveGeoFeature</wfs:TypeName></wfs:DescribeFeatureType>")
        .await;
    r.assert_ok();
    assert_schema(&r.xml(), &["PrimitiveGeoFeature"]);
}

/// DescribeFeatureTypeTest.testMultipleTypesImport
#[actix_web::test]
async fn gs_describe_feature_type_test_multiple_types_import() {
    let srv = server().await;
    let r = srv
        .post_wfs("<wfs:DescribeFeatureType service='WFS' version='2.0.0' xmlns:wfs='http://www.opengis.net/wfs/2.0' xmlns:sf='http://cite.opengeospatial.org/gmlsf'><wfs:TypeName>sf:PrimitiveGeoFeature</wfs:TypeName><wfs:TypeName>sf:GenericEntity</wfs:TypeName></wfs:DescribeFeatureType>")
        .await;
    r.assert_ok();
    let xml = r.xml();
    assert_schema(&xml, &["PrimitiveGeoFeature", "GenericEntity"]);
    // imports precede all type definitions
    xml.assert_count(
        "/xs:schema/xs:complexType[1]/following-sibling::xs:import",
        0,
    );
}

/// DescribeFeatureTypeTest.testUserSuppliedTypeNameNamespace
#[actix_web::test]
async fn gs_describe_feature_type_test_user_supplied_type_name_namespace() {
    let srv = server().await;
    let r = srv
        .get(&format!("{W}DescribeFeatureType&typeName=myPrefix:Polygons&namespaces=xmlns(myPrefix,http%3A%2F%2Fwww.opengis.net%2Fcite%2Fgeometry)"))
        .await;
    r.assert_ok();
    assert_schema(&r.xml(), &["Polygons"]);
}

/// DescribeFeatureTypeTest.testUserSuppliedTypeNameDefaultNamespace
#[actix_web::test]
async fn gs_describe_feature_type_test_user_supplied_type_name_default_namespace() {
    let srv = server().await;
    let r = srv
        .get(&format!("{W}DescribeFeatureType&typeName=Polygons&namespace=xmlns(http%3A%2F%2Fwww.opengis.net%2Fcite%2Fgeometry)"))
        .await;
    r.assert_ok();
    assert_schema(&r.xml(), &["Polygons"]);
}

/// DescribeFeatureTypeTest.testMissingNameNamespacePrefix (unqualified name resolved by local name)
#[actix_web::test]
async fn gs_describe_feature_type_test_missing_name_namespace_prefix() {
    let srv = server().await;
    let r = srv
        .get(&format!("{W}DescribeFeatureType&typeName=Polygons"))
        .await;
    r.assert_ok();
    assert_schema(&r.xml(), &["Polygons"]);
}

/// DescribeFeatureTypeTest.testCiteCompliance (non-strict mode)
#[actix_web::test]
async fn gs_describe_feature_type_test_cite_compliance() {
    let srv = server().await;
    let r = srv
        .get(&format!("{W}DescribeFeatureType&typeName=Streams"))
        .await;
    r.assert_ok();
    assert_schema(&r.xml(), &["Streams"]);
}

/// DescribeFeatureTypeTest.testPrefixedGetStrictCite
#[actix_web::test]
async fn gs_describe_feature_type_test_prefixed_get_strict_cite() {
    let srv = server().await;
    let r = srv
        .get(&format!("{W}DescribeFeatureType&typeName=cgf:Polygons"))
        .await;
    r.assert_ok();
    assert_schema(&r.xml(), &["Polygons"]);
}

/// DescribeFeatureTypeTest.testGML32OutputFormat
#[actix_web::test]
async fn gs_describe_feature_type_test_gml32_output_format() {
    let srv = server().await;
    let r = srv
        .get(&format!(
            "{W}DescribeFeatureType&outputFormat=text/xml;+subtype%3Dgml/3.2&typename=cgf:Polygons"
        ))
        .await;
    r.assert_ok();
    assert_schema(&r.xml(), &["Polygons"]);
}

/// DescribeFeatureTypeTest.testSOAP
#[actix_web::test]
async fn gs_describe_feature_type_test_soap() {
    let srv = server().await;
    let r = srv
        .post_with_type("/wfs", &soap12(DFT_POST), "application/soap+xml")
        .await;
    r.assert_ok();
    assert!(
        r.content_type.starts_with("application/soap+xml"),
        "{}",
        r.content_type
    );
    let xml = r.xml();
    xml.assert("/*[local-name()='Envelope']");
    assert_eq!(
        xml.string("//*[local-name()='Body']/@type"),
        "xsd:base64",
        "{}",
        r.body
    );
    xml.assert_count("//wfs2:DescribeFeatureTypeResponse", 1);
    let schema = String::from_utf8(base64_decode(
        &xml.string("//wfs2:DescribeFeatureTypeResponse"),
    ))
    .unwrap();
    Xml::new(schema).assert("/xs:schema");
}

/// DescribeFeatureTypeTest.testNoWfsSchemaImport
#[actix_web::test]
async fn gs_describe_feature_type_test_no_wfs_schema_import() {
    let srv = server().await;
    let r = srv
        .get(&format!(
            "{W}DescribeFeatureType&typeNames=sf:PrimitiveGeoFeature"
        ))
        .await;
    r.assert_ok();
    assert_eq!(r.content_type, "application/gml+xml; version=3.2");
    let xml = r.xml();
    assert_schema(&xml, &["PrimitiveGeoFeature"]);
    xml.assert_count(
        "//xs:import[@namespace='http://www.opengis.net/wfs/2.0']",
        0,
    );
}

// ---------------------------------------------------------------- SrsNameRequestTest

const LON_LAT_BBOX: &str = "34.939,-10.521,34.941,-10.519";
const LAT_LON_BBOX: &str = "-10.521,34.939,-10.519,34.941";
const EPSG_XML: &str = "http://www.opengis.net/gml/srs/epsg.xml#4326";
const URN: &str = "urn:ogc:def:crs:EPSG::4326";

/// GetFeature on sf:PrimitiveGeoFeature with srsName / bbox; checks feature f015
async fn srs_request(srs: &str, bbox: Option<String>, returned: usize, out_srs: &str, pos: &str) {
    let srv = server().await;
    let mut path = format!("/wfs?service=WFS&version=2.0.0&request=GetFeature&typenames=sf:PrimitiveGeoFeature&srsname={}", srs.replace('#', "%23"));
    if let Some(b) = bbox {
        path.push_str(&format!("&bbox={}", b.replace('#', "%23")));
    }
    let r = srv.get(&path).await;
    r.assert_ok();
    let xml = r.xml();
    assert_eq!(
        xml.string("//wfs2:FeatureCollection/@numberReturned"),
        returned.to_string(),
        "{path}"
    );
    if returned > 0 {
        let f = "//sf:PrimitiveGeoFeature[@gml32:id='PrimitiveGeoFeature.f015']";
        xml.assert(f);
        // GeoServer normalizes the srsName (e.g. EPSG:4326 -> http://www.opengis.net/gml/srs/epsg.xml#4326,
        // urn:x-ogc / http URI -> urn:ogc); echoing the requested identifier is equally valid
        let got = xml.string(&format!("{f}/sf:pointProperty/gml32:Point/@srsName"));
        assert!(got == out_srs || got == srs, "{path}: srsName {got}");
        assert_eq!(
            xml.string(&format!("{f}/sf:pointProperty/gml32:Point/gml32:pos")),
            pos,
            "{path}"
        );
    }
}

macro_rules! srs_test {
    ($name:ident, $doc:literal, $srs:expr, $bbox:expr, $n:expr, $out:expr, $pos:expr) => {
        #[doc = $doc]
        #[actix_web::test]
        async fn $name() {
            srs_request($srs, $bbox, $n, $out, $pos).await;
        }
    };
}

fn with_crs(bbox: &str, crs: &str) -> Option<String> {
    Some(format!("{bbox},{crs}"))
}

srs_test!(
    gs_srs_name_request_test_epsg_code,
    "SrsNameRequestTest.testEpsgCode",
    "EPSG:4326",
    None,
    5,
    EPSG_XML,
    "34.94 -10.52"
);
srs_test!(
    gs_srs_name_request_test_epsg_code_native_bbox,
    "SrsNameRequestTest.testEpsgCodeNativeBbox",
    "EPSG:4326",
    Some(LAT_LON_BBOX.into()),
    1,
    EPSG_XML,
    "34.94 -10.52"
);
srs_test!(
    gs_srs_name_request_test_epsg_code_native_bbox_wrong_axis_order,
    "SrsNameRequestTest.testEpsgCodeNativeBboxWrongAxisOrder",
    "EPSG:4326",
    Some(LON_LAT_BBOX.into()),
    0,
    "",
    ""
);
srs_test!(
    gs_srs_name_request_test_epsg_code_bbox,
    "SrsNameRequestTest.testEpsgCodeBbox",
    "EPSG:4326",
    with_crs(LON_LAT_BBOX, "EPSG:4326"),
    1,
    EPSG_XML,
    "34.94 -10.52"
);
srs_test!(
    gs_srs_name_request_test_epsg_code_bbox_wrong_axis_order,
    "SrsNameRequestTest.testEpsgCodeBboxWrongAxisOrder",
    "EPSG:4326",
    with_crs(LAT_LON_BBOX, "EPSG:4326"),
    0,
    "",
    ""
);
srs_test!(
    gs_srs_name_request_test_http_url,
    "SrsNameRequestTest.testHttpUrl",
    EPSG_XML,
    None,
    5,
    EPSG_XML,
    "34.94 -10.52"
);
srs_test!(
    gs_srs_name_request_test_http_url_native_bbox,
    "SrsNameRequestTest.testHttpUrlNativeBbox",
    EPSG_XML,
    Some(LAT_LON_BBOX.into()),
    1,
    EPSG_XML,
    "34.94 -10.52"
);
srs_test!(
    gs_srs_name_request_test_http_url_native_bbox_wrong_axis_order,
    "SrsNameRequestTest.testHttpUrlNativeBboxWrongAxisOrder",
    EPSG_XML,
    Some(LON_LAT_BBOX.into()),
    0,
    "",
    ""
);
srs_test!(
    gs_srs_name_request_test_http_url_bbox,
    "SrsNameRequestTest.testHttpUrlBbox",
    EPSG_XML,
    with_crs(LON_LAT_BBOX, EPSG_XML),
    1,
    EPSG_XML,
    "34.94 -10.52"
);
srs_test!(
    gs_srs_name_request_test_http_url_bbox_wrong_axis_order,
    "SrsNameRequestTest.testHttpUrlBboxWrongAxisOrder",
    EPSG_XML,
    with_crs(LAT_LON_BBOX, EPSG_XML),
    0,
    "",
    ""
);
srs_test!(
    gs_srs_name_request_test_urn_experimental,
    "SrsNameRequestTest.testUrnExperimental",
    "urn:x-ogc:def:crs:EPSG::4326",
    None,
    5,
    URN,
    "-10.52 34.94"
);
srs_test!(
    gs_srs_name_request_test_urn_experimental_native_bbox,
    "SrsNameRequestTest.testUrnExperimentalNativeBbox",
    "urn:x-ogc:def:crs:EPSG::4326",
    Some(LAT_LON_BBOX.into()),
    1,
    URN,
    "-10.52 34.94"
);
srs_test!(
    gs_srs_name_request_test_urn_experimental_native_bbox_wrong_axis_order,
    "SrsNameRequestTest.testUrnExperimentalNativeBboxWrongAxisOrder",
    "urn:x-ogc:def:crs:EPSG::4326",
    Some(LON_LAT_BBOX.into()),
    0,
    "",
    ""
);
srs_test!(
    gs_srs_name_request_test_urn_experimental_bbox,
    "SrsNameRequestTest.testUrnExperimentalBbox",
    "urn:x-ogc:def:crs:EPSG::4326",
    with_crs(LAT_LON_BBOX, "urn:x-ogc:def:crs:EPSG::4326"),
    1,
    URN,
    "-10.52 34.94"
);
srs_test!(
    gs_srs_name_request_test_urn_experimental_bbox_wrong_axis_order,
    "SrsNameRequestTest.testUrnExperimentalBboxWrongAxisOrder",
    "urn:x-ogc:def:crs:EPSG::4326",
    with_crs(LON_LAT_BBOX, "urn:x-ogc:def:crs:EPSG::4326"),
    0,
    "",
    ""
);
srs_test!(
    gs_srs_name_request_test_urn,
    "SrsNameRequestTest.testUrn",
    URN,
    None,
    5,
    URN,
    "-10.52 34.94"
);
srs_test!(
    gs_srs_name_request_test_urn_native_bbox,
    "SrsNameRequestTest.testUrnNativeBbox",
    URN,
    Some(LAT_LON_BBOX.into()),
    1,
    URN,
    "-10.52 34.94"
);
srs_test!(
    gs_srs_name_request_test_urn_native_bbox_wrong_axis_order,
    "SrsNameRequestTest.testUrnNativeBboxWrongAxisOrder",
    URN,
    Some(LON_LAT_BBOX.into()),
    0,
    "",
    ""
);
srs_test!(
    gs_srs_name_request_test_urn_bbox,
    "SrsNameRequestTest.testUrnBbox",
    URN,
    with_crs(LAT_LON_BBOX, URN),
    1,
    URN,
    "-10.52 34.94"
);
srs_test!(
    gs_srs_name_request_test_urn_bbox_wrong_axis_order,
    "SrsNameRequestTest.testUrnBboxWrongAxisOrder",
    URN,
    with_crs(LON_LAT_BBOX, URN),
    0,
    "",
    ""
);
srs_test!(
    gs_srs_name_request_test_http_uri,
    "SrsNameRequestTest.testHttpUri",
    "http://www.opengis.net/def/crs/EPSG/0/4326",
    None,
    5,
    URN,
    "-10.52 34.94"
);
srs_test!(
    gs_srs_name_request_test_http_uri_native_bbox,
    "SrsNameRequestTest.testHttpUriNativeBbox",
    "http://www.opengis.net/def/crs/EPSG/0/4326",
    Some(LAT_LON_BBOX.into()),
    1,
    URN,
    "-10.52 34.94"
);
srs_test!(
    gs_srs_name_request_test_http_uri_native_bbox_wrong_axis_order,
    "SrsNameRequestTest.testHttpUriNativeBboxWrongAxisOrder",
    "http://www.opengis.net/def/crs/EPSG/0/4326",
    Some(LON_LAT_BBOX.into()),
    0,
    "",
    ""
);
srs_test!(
    gs_srs_name_request_test_http_uri_bbox,
    "SrsNameRequestTest.testHttpUriBbox",
    "http://www.opengis.net/def/crs/EPSG/0/4326",
    with_crs(LAT_LON_BBOX, "http://www.opengis.net/def/crs/EPSG/0/4326"),
    1,
    URN,
    "-10.52 34.94"
);
srs_test!(
    gs_srs_name_request_test_http_uri_bbox_wrong_axis_order,
    "SrsNameRequestTest.testHttpUriBboxWrongAxisOrder",
    "http://www.opengis.net/def/crs/EPSG/0/4326",
    with_crs(LON_LAT_BBOX, "http://www.opengis.net/def/crs/EPSG/0/4326"),
    0,
    "",
    ""
);

// ---------------------------------------------------------------- SrsNameTest

/// SrsNameTest.testSrsNameSyntax (default WFS 2.0 style)
#[actix_web::test]
async fn gs_srs_name_test_srs_name_syntax() {
    let srv = server().await;
    let r = srv
        .get("/wfs?request=getfeature&service=wfs&version=2.0.0&typenames=cgf:Points")
        .await;
    r.assert_ok();
    let xml = r.xml();
    xml.assert("/wfs2:FeatureCollection");
    let srs = "urn:ogc:def:crs:EPSG::32615";
    // GeoServer's test catalog enables feature bounding (gml:boundedBy per feature); bbox does not
    // emit boundedBy in WFS 2.0, so only an Envelope that is present is checked
    xml.assert_count(&format!("//gml32:Envelope[@srsName!='{srs}']"), 0);
    xml.assert(&format!("//gml32:Point[@srsName='{srs}']"));
}

// ---------------------------------------------------------------- GMLOutputFormatTest

/// GMLOutputFormatTest.testGML32 (outputFormat alias `gml32`)
#[actix_web::test]
async fn gs_gml_output_format_test_gml32() {
    let srv = server().await;
    // with SERVICE: outputFormat alias only
    let r = srv.get("/wfs?service=WFS&request=getfeature&version=2.0.0&outputFormat=gml32&typename=cite:BasicPolygons").await;
    r.assert_ok();
    // GeoServer (non-CITE mode) also tolerates the missing SERVICE parameter
    let r = srv
        .get("/wfs?request=getfeature&version=2.0.0&outputFormat=gml32&typename=cite:BasicPolygons")
        .await;
    r.assert_ok();
    r.xml().assert(
        "/*[local-name()='FeatureCollection' and namespace-uri()='http://www.opengis.net/wfs/2.0']",
    );
}

// ---------------------------------------------------------------- WFSServiceExceptionTest

const UNKNOWN_DFT: &str = "/wfs/?service=wfs&version=2.0.0&request=DescribeFeatureType&typeName=foobar&format_options=callback:myMethod&EXCEPTIONS=";

fn check_json_exception(json: &serde_json::Value) {
    assert_eq!(json["version"], "2.0.0", "{json}");
    let e = &json["exceptions"][0];
    assert!(!e["code"].is_null(), "{json}");
    assert!(!e["locator"].is_null(), "{json}");
    assert!(
        e["text"].as_str().unwrap_or("").contains("foobar"),
        "{json}"
    );
}

/// WFSServiceExceptionTest.testJsonpException20 (vendor EXCEPTIONS=text/javascript)
#[actix_web::test]
async fn gs_wfs_service_exception_test_jsonp_exception20() {
    let srv = server().await;
    let r = srv
        .get(&format!(
            "{}text/javascript",
            UNKNOWN_DFT.replace("/wfs/?", "/wfs?")
        ))
        .await;
    assert!(
        r.content_type.starts_with("text/javascript"),
        "{} {}",
        r.content_type,
        r.body
    );
    let body = r.body.trim();
    let inner = body
        .strip_prefix("myMethod(")
        .and_then(|b| b.strip_suffix(')'))
        .unwrap_or_else(|| panic!("{body}"));
    check_json_exception(&serde_json::from_str(inner).unwrap());
}

/// WFSServiceExceptionTest.testJsonException20 (vendor EXCEPTIONS=application/json)
#[actix_web::test]
async fn gs_wfs_service_exception_test_json_exception20() {
    let srv = server().await;
    let r = srv
        .get(&format!(
            "{}application/json",
            UNKNOWN_DFT.replace("/wfs/?", "/wfs?")
        ))
        .await;
    assert!(
        r.content_type.starts_with("application/json"),
        "{} {}",
        r.content_type,
        r.body
    );
    check_json_exception(&serde_json::from_str(&r.body).unwrap());
}

// ---------------------------------------------------------------- ExternalEntitiesTest

/// ExternalEntitiesTest.testWfs2_0 (requests with a DOCTYPE are rejected)
#[actix_web::test]
async fn gs_external_entities_test_wfs2_0() {
    let srv = server().await;
    let body = r#"<?xml version="1.0" ?>
<!DOCTYPE wfs:GetFeature [
<!ELEMENT wfs:GetFeature (wfs:Query*)>
<!ATTLIST wfs:GetFeature service CDATA #FIXED "WFS" version CDATA #FIXED "2.0.0" outputFormat CDATA #FIXED "application/gml+xml; version=3.2" xmlns:wfs CDATA #FIXED "http://www.opengis.net/wfs" xmlns:ogc CDATA #FIXED "http://www.opengis.net/ogc" xmlns:fes CDATA #FIXED "http://www.opengis.net/fes/2.0">
<!ELEMENT wfs:Query (wfs:PropertyName*,ogc:Filter?)>
<!ATTLIST wfs:Query typeName CDATA #FIXED "cdf:Fifteen">
<!ELEMENT wfs:PropertyName (#PCDATA) >
<!ELEMENT ogc:Filter (fes:ResourceId*)>
<!ELEMENT fes:ResourceId EMPTY>
<!ATTLIST fes:ResourceId rid CDATA #FIXED "states.3">
<!ENTITY passwd  SYSTEM "FILE:///thisfiledoesnotexist?.XSD">
]>
<wfs:GetFeature service="WFS" version="2.0.0" outputFormat="application/gml+xml; version=3.2" xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:fes="http://www.opengis.net/fes/2.0">
  <wfs:Query typeName="cdf:Fifteen"><wfs:PropertyName>&passwd;</wfs:PropertyName>
    <fes:Filter><fes:ResourceId rid="states.3"/></fes:Filter></wfs:Query>
</wfs:GetFeature>"#;
    let r = srv.post_wfs(body).await;
    let xml = r.xml();
    xml.assert("/ows11:ExceptionReport[@version='2.0.0']");
    let text = xml.string("//ows11:ExceptionText");
    let upper = text.to_uppercase();
    assert!(
        upper.contains("DOCTYPE") || upper.contains("DTD"),
        "{}",
        r.body
    );
}

// ---------------------------------------------------------------- Filter_2_0_0_KvpParserTest

/// PropertyIsLike KVP filter on sf:PrimitiveGeoFeature names (name-f001..f003, f008)
async fn like_count(literal: &str, match_case: Option<&str>) -> String {
    let srv = server().await;
    let mc = match_case
        .map(|m| format!(" matchCase=\"{m}\""))
        .unwrap_or_default();
    let filter = format!(
        r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:sf="http://cite.opengeospatial.org/gmlsf"><fes:PropertyIsLike wildCard="*" singleChar="%" escapeChar="!"{mc}><fes:ValueReference>sf:name</fes:ValueReference><fes:Literal>{literal}</fes:Literal></fes:PropertyIsLike></fes:Filter>"#
    );
    let r = srv
        .kvp(&[
            ("service", "WFS"),
            ("version", "2.0.0"),
            ("request", "GetFeature"),
            ("typeNames", "sf:PrimitiveGeoFeature"),
            ("resultType", "hits"),
            ("filter", &filter),
        ])
        .await;
    r.assert_ok();
    r.xml().string("/wfs2:FeatureCollection/@numberMatched")
}

/// Filter_2_0_0_KvpParserTest.testPropertyIsLikeAsciiLiteral (matchCase defaults to true;
/// `%` single char wildcard survives URL decoding)
#[actix_web::test]
async fn gs_filter_2_0_0_kvp_parser_test_property_is_like_ascii_literal() {
    assert_eq!(like_count("name-f00*", None).await, "4");
    assert_eq!(like_count("name-f00%", None).await, "4");
    assert_eq!(like_count("NAME-F00*", None).await, "0");
}

/// Filter_2_0_0_KvpParserTest.testPropertyIsLikeNonAsciiLiteral
#[actix_web::test]
async fn gs_filter_2_0_0_kvp_parser_test_property_is_like_non_ascii_literal() {
    assert_eq!(like_count("ü*", None).await, "0");
}

/// Filter_2_0_0_KvpParserTest.testPropertyIsLikeMatchCaseTrue
#[actix_web::test]
async fn gs_filter_2_0_0_kvp_parser_test_property_is_like_match_case_true() {
    assert_eq!(like_count("NAME-F00*", Some("true")).await, "0");
}

/// Filter_2_0_0_KvpParserTest.testPropertyIsLikeMatchCaseFalse
#[actix_web::test]
async fn gs_filter_2_0_0_kvp_parser_test_property_is_like_match_case_false() {
    assert_eq!(like_count("NAME-F00*", Some("false")).await, "4");
}

// ---------------------------------------------------------------- read parts of write tests

/// TransactionTest.testInsertWithNoSRS (query part: stray wfs:ValueReference inside wfs:Query)
#[actix_web::test]
async fn gs_transaction_test_insert_with_no_srs_query() {
    let srv = server().await;
    let r = srv
        .post_wfs(r#"<wfs:GetFeature service='WFS' version='2.0.0' xmlns:cgf='http://www.opengis.net/cite/geometry' xmlns:fes='http://www.opengis.net/fes/2.0' xmlns:wfs='http://www.opengis.net/wfs/2.0'><wfs:Query typeNames="cgf:Points"><wfs:ValueReference>cite:id</wfs:ValueReference></wfs:Query></wfs:GetFeature>"#)
        .await;
    r.assert_ok();
    let xml = r.xml();
    let n = xml.count("//cgf:Points");
    assert!(n > 0, "{}", r.body);
    // filter on cgf:id, no srsName: native 32615 order
    let r = srv
        .post_wfs(r#"<wfs:GetFeature service='WFS' version='2.0.0' xmlns:cgf='http://www.opengis.net/cite/geometry' xmlns:fes='http://www.opengis.net/fes/2.0' xmlns:wfs='http://www.opengis.net/wfs/2.0'><wfs:Query typeNames="cgf:Points"><fes:Filter><fes:PropertyIsEqualTo><fes:ValueReference>cgf:id</fes:ValueReference><fes:Literal>t0000</fes:Literal></fes:PropertyIsEqualTo></fes:Filter></wfs:Query></wfs:GetFeature>"#)
        .await;
    r.assert_ok();
    assert_eq!(r.xml().string("(//gml32:pos)[1]"), "500050 500050");
}

/// TransactionTest.testUpdateOnSimpleXPath (read part: RESOURCEID KVP)
#[actix_web::test]
async fn gs_transaction_test_update_on_simple_x_path_read() {
    let srv = server().await;
    let xml = srv
        .get("/wfs?service=wfs&version=2.0.0&request=getfeature&typename=sf:GenericEntity&resourceId=GenericEntity.f004")
        .await
        .xml();
    xml.assert_count("//sf:GenericEntity", 1);
    assert_eq!(xml.string("//sf:featureRef"), "name-f003");
}

/// TransactionTest.testInsert2 (read part: cql_filter with EPSG:4326 lon/lat output)
#[actix_web::test]
async fn gs_transaction_test_insert2_read() {
    let srv = server().await;
    let r = srv
        .get("/wfs?service=WFS&version=2.0.0&request=getfeature&typename=cite:RoadSegments&srsName=EPSG:4326&cql_filter=FID%3D'102'")
        .await;
    r.assert_ok();
    let xml = r.xml();
    xml.assert_count("//*[local-name()='RoadSegments']", 1);
    let pos: Vec<f64> = xml
        .string("(//gml32:posList)[1]")
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect();
    // RoadSegments FID 102 starts near lon 0.0042, lat 0.0006 (lon/lat order)
    assert!(pos.len() >= 4, "{pos:?}");
    let first = std::fs::read_to_string(geoserver::data_dir().join("main/RoadSegments.properties"))
        .unwrap();
    let line = first
        .lines()
        .find(|l| l.contains("|102|"))
        .expect("FID 102");
    let wkt_first: Vec<f64> = line
        .split("((")
        .nth(1)
        .unwrap()
        .split(',')
        .next()
        .unwrap()
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect();
    assert!(
        (pos[0] - wkt_first[0]).abs() < 1e-6 && (pos[1] - wkt_first[1]).abs() < 1e-6,
        "{pos:?} vs {wkt_first:?}"
    );
}

// ---------------------------------------------------------------- WfsRemoteStoreTest

/// WfsRemoteStoreTest.testAddRemoteWfsLayer (request style of GeoServer's WFS 2.0 client)
#[actix_web::test]
async fn gs_wfs_remote_store_test_add_remote_wfs_layer() {
    let srv = server().await;
    let r = srv
        .get("/wfs?REQUEST=DescribeFeatureType&VERSION=2.0.0&TYPENAMES=ns9%3APrimitiveGeoFeature&NAMESPACES=xmlns%28ns9%2Chttp%3A%2F%2Fcite.opengeospatial.org%2Fgmlsf%29&SERVICE=WFS")
        .await;
    r.assert_ok();
    assert_schema(&r.xml(), &["PrimitiveGeoFeature"]);
    let r = srv
        .get("/wfs?SERVICE=WFS&REQUEST=GetFeature&VERSION=2.0.0&TYPENAMES=ns9%3APrimitiveGeoFeature&NAMESPACES=xmlns%28ns9%2Chttp%3A%2F%2Fcite.opengeospatial.org%2Fgmlsf%29&RESULTTYPE=RESULTS&OUTPUTFORMAT=application%2Fgml%2Bxml%3B+version%3D3.2&COUNT=1000000&PROPERTYNAME=ns9%3ApointProperty%2Cns9%3AintProperty")
        .await;
    r.assert_ok();
    assert!(
        r.body.contains(r#"numberMatched="5" numberReturned="5""#),
        "{}",
        &r.body[..400.min(r.body.len())]
    );
}
