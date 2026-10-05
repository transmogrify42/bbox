//! WFS 1.1.0 behaviour checks derived from the GeoServer WFS 1.1 test suite
//! (wfs1_x module: v1_1, kvp, xml packages), run against the GeoServer default test catalog
//! (tests/common/geoserver.rs; data from tests/fetch-geoserver-data.sh).
//!
//! The tests are independent re-implementations of the asserted behaviour, not copies.
//!
//! Not ported (with reason):
//! - GetCapabilitiesTest: testLayerQualified (virtual services), testMetadataLinks*,
//!   testOtherSRSSingleTypeOverride (catalog config), testIauFeatureTypes (IAU CRS + workspace),
//!   testCiteCompliant (cite-compliant mode has no bbox configuration).
//! - CapabilitiesTransformerTest.testContactInfo (GeoServer test contact config).
//! - WFSXmlTest.testInvalid (internal parser validation, no observable HTTP contract).
//! - DescribeFeatureTypeTest: testGMLAttributeMapping override=true branch, testCustomSchema,
//!   testCustomizeFeatureType (catalog customisation); testCiteCompliance strict branch.
//! - GetFeatureTest: testPostWithBoundsEnabled, testFeatureMembers (GeoServer config switches),
//!   testAfterFeatureTypeAdded (catalog reload), testWithGMLProperties (sf:WithGMLProperties not in
//!   the default catalog; the mapping is covered by gs_get_feature_test_gml_attribute_mapping),
//!   testLayerQualified, testVirtualServicesInvocation (virtual services), testGetIAULayer*
//!   (IAU CRS); trailing-slash `wfs/` path variant (handled by NormalizePath middleware, absent
//!   in the in-process harness).
//! - GetFeatureBboxTest (featureBounding config switch).
//! - BoundingBox3DTest (sf:With3D not in the default catalog).
//! - AliasTest (published name aliases), WfsRemoteStoreTest (cascaded WFS).
//! - WFSReprojectionTest.testGetFeatureWithAutoBoxGet (AUTO:42004 CRS).
//! - Transaction / LockFeature / GetFeatureWithLock / write tests (read-only server).
//! - GetFeatureKvpRequestReaderTest viewParams tests (SQL views).
//! - FeatureTypeInfoSchemaBuilderTest.testUUID (cite:uuid not in catalog), testDocumentation
//!   (internal); XMLParsingTest (internal; wfs:Native covered by testWithSillyLiteral).
//!
//! Adaptations: EPSG:900913 (GeoServer legacy alias) is replaced by EPSG:3857; ne:countries
//! (MultiPolygon) by cite:Buildings; feature member XPaths accept gml:featureMember and
//! gml:featureMembers (GeoServer's choice of featureMembers is a configuration setting).

mod common;
use common::*;

const WFS11_XSD: &str = "http://schemas.opengis.net/wfs/1.1.0/wfs.xsd";
const CITE: &str = "http://www.opengis.net/cite";

/// XPath step for an element in the cite namespace (prefix not registered in the harness)
fn cite(local: &str) -> String {
    format!("*[local-name()='{local}' and namespace-uri()='{CITE}']")
}

fn post_get_feature(query: &str, attrs: &str) -> String {
    format!(
        r#"<wfs:GetFeature service="WFS" version="1.1.0" {attrs} xmlns:cdf="http://www.opengis.net/cite/data" xmlns:sf="http://cite.opengeospatial.org/gmlsf" xmlns:gml="http://www.opengis.net/gml" xmlns:ogc="http://www.opengis.net/ogc" xmlns:wfs="http://www.opengis.net/wfs">{query}</wfs:GetFeature>"#
    )
}

/// FeatureCollection with Fifteen features, each with gml:id
fn assert_fifteen_all(r: &Response) {
    r.assert_ok();
    let xml = r.xml();
    xml.assert("/wfs:FeatureCollection");
    let n = xml.count("//cdf:Fifteen");
    assert!(n > 0, "no cdf:Fifteen:\n{}", r.body);
    assert_eq!(xml.count("//cdf:Fifteen[@gml:id]"), n);
}

fn assert_ows10_exception(r: &Response, code: &str, locator: Option<&str>) -> Xml {
    let xml = r.xml();
    xml.assert("/ows:ExceptionReport[@version='1.0.0']");
    xml.assert_count("//ows:Exception", 1);
    assert_eq!(
        xml.string("//ows:Exception/@exceptionCode"),
        code,
        "{}",
        r.body
    );
    if let Some(l) = locator {
        assert_eq!(
            xml.string("//ows:Exception/@locator").to_lowercase(),
            l.to_lowercase(),
            "{}",
            r.body
        );
    }
    xml
}

// ---------------------------------------------------------------- GetCapabilitiesTest

/// GetCapabilitiesTest.testAcceptVersions11
#[actix_web::test]
async fn gs_get_capabilities_test_accept_versions11() {
    let srv = geoserver::server().await;
    let r = srv
        .get("/wfs?request=GetCapabilities&acceptversions=1.1.0,1.0.0")
        .await;
    r.assert_ok();
    r.xml().assert("/wfs:WFS_Capabilities[@version='1.1.0']");
}

/// GetCapabilitiesTest.testAcceptVersions11WithVersion
#[actix_web::test]
async fn gs_get_capabilities_test_accept_versions11_with_version() {
    let srv = geoserver::server().await;
    let r = srv
        .get("/wfs?request=GetCapabilities&version=2.0.0&acceptversions=1.1.0,1.0.0")
        .await;
    r.assert_ok();
    r.xml().assert("/wfs:WFS_Capabilities[@version='1.1.0']");
}

/// GetCapabilitiesTest.testGet
#[actix_web::test]
async fn gs_get_capabilities_test_get() {
    let srv = geoserver::server().await;
    let r = srv
        .get("/wfs?service=WFS&request=getCapabilities&version=1.1.0")
        .await;
    r.assert_ok();
    let xml = r.xml();
    xml.assert("/wfs:WFS_Capabilities[@version='1.1.0']");
    assert!(xml.count("//wfs:FeatureType") > 0);
    assert!(!r.body.contains("xmlns:xml="));
}

/// GetCapabilitiesTest.testNamespaceFilter (GeoServer `namespace` vendor parameter)
#[actix_web::test]
async fn gs_get_capabilities_test_namespace_filter() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?service=WFS&version=1.1.0&request=getCapabilities&namespace=sf")
        .await
        .xml();
    assert!(xml.count("//wfs:FeatureType/wfs:Name[starts-with(., 'sf:')]") > 0);
    xml.assert_count("//wfs:FeatureType/wfs:Name[not(starts-with(., 'sf:'))]", 0);
    let r = srv
        .get("/wfs?service=WFS&version=1.1.0&request=getCapabilities&namespace=NotThere")
        .await;
    r.assert_ok();
    let xml = r.xml();
    xml.assert("/wfs:WFS_Capabilities");
    xml.assert_count("//wfs:FeatureType", 0);
}

const CAPS_POST: &str = r#"<GetCapabilities service="WFS" version='1.1.0' xmlns="http://www.opengis.net/wfs" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:ows="http://www.opengis.net/ows" xsi:schemaLocation="http://www.opengis.net/wfs  http://schemas.opengis.net/wfs/1.1.0/wfs.xsd"><ows:AcceptVersions><ows:Version>1.1.0</ows:Version></ows:AcceptVersions></GetCapabilities>"#;

/// GetCapabilitiesTest.testPost
#[actix_web::test]
async fn gs_get_capabilities_test_post() {
    let srv = geoserver::server().await;
    let r = srv.post_wfs(CAPS_POST).await;
    r.assert_ok();
    r.xml().assert("/wfs:WFS_Capabilities[@version='1.1.0']");
}

/// GetCapabilitiesTest.testPostNoSchemaLocation
#[actix_web::test]
async fn gs_get_capabilities_test_post_no_schema_location() {
    let srv = geoserver::server().await;
    let body = CAPS_POST.replace(
        r#" xsi:schemaLocation="http://www.opengis.net/wfs  http://schemas.opengis.net/wfs/1.1.0/wfs.xsd""#,
        "",
    );
    let r = srv.post_wfs(&body).await;
    r.assert_ok();
    r.xml().assert("/wfs:WFS_Capabilities[@version='1.1.0']");
}

/// GetCapabilitiesTest.testOutputFormats (adapted: every advertised format is accepted)
#[actix_web::test]
async fn gs_get_capabilities_test_output_formats() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?service=WFS&request=getCapabilities&version=1.1.0")
        .await
        .xml();
    let formats = xml.strings(
        "//ows:Operation[@name='GetFeature']/ows:Parameter[@name='outputFormat']/ows:Value",
    );
    assert!(!formats.is_empty());
    for f in formats {
        let r = srv
            .kvp(&[
                ("service", "WFS"),
                ("version", "1.1.0"),
                ("request", "GetFeature"),
                ("typename", "cdf:Seven"),
                ("outputFormat", &f),
            ])
            .await;
        assert_eq!(r.status, 200, "outputFormat {f}: {}", r.body);
        assert!(
            !r.body.contains("ExceptionReport"),
            "outputFormat {f}: {}",
            r.body
        );
    }
}

/// GetCapabilitiesTest.testSupportedSpatialOperators
#[actix_web::test]
async fn gs_get_capabilities_test_supported_spatial_operators() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?service=WFS&request=getCapabilities&version=1.1.0")
        .await
        .xml();
    let mut ops =
        xml.strings("//ogc:Spatial_Capabilities/ogc:SpatialOperators/ogc:SpatialOperator/@name");
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

/// GetCapabilitiesTest.testFunctionArgCount
#[actix_web::test]
async fn gs_get_capabilities_test_function_arg_count() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?service=WFS&request=getCapabilities&version=1.1.0")
        .await
        .xml();
    assert_eq!(xml.string("//ogc:FunctionName[text()='abs']/@nArgs"), "1");
}

/// GetCapabilitiesTest.testTypeNameCount
#[actix_web::test]
async fn gs_get_capabilities_test_type_name_count() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?service=WFS&request=getCapabilities&version=1.1.0")
        .await
        .xml();
    let n: usize = geoserver::NAMESPACES.iter().map(|ns| ns.types.len()).sum();
    xml.assert_count(
        "/wfs:WFS_Capabilities/wfs:FeatureTypeList/wfs:FeatureType",
        n,
    );
}

/// GetCapabilitiesTest.testTypeNames
#[actix_web::test]
async fn gs_get_capabilities_test_type_names() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?service=WFS&request=getCapabilities&version=1.1.0")
        .await
        .xml();
    for ns in geoserver::NAMESPACES {
        for t in ns.types {
            xml.assert(&format!("/wfs:WFS_Capabilities/wfs:FeatureTypeList/wfs:FeatureType/wfs:Name[text()='{}:{t}']", ns.prefix));
        }
    }
}

/// GetCapabilitiesTest.testOtherSRS (adapted: URN encoding, native SRS not repeated)
#[actix_web::test]
async fn gs_get_capabilities_test_other_srs() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?service=WFS&request=getCapabilities&version=1.1.0")
        .await
        .xml();
    for name in xml.strings("//wfs:FeatureType/wfs:Name") {
        let ft = format!("//wfs:FeatureType[wfs:Name='{name}']");
        let default = xml.string(&format!("{ft}/wfs:DefaultSRS"));
        assert!(default.starts_with("urn:"), "{name}: {default}");
        let others = xml.strings(&format!("{ft}/wfs:OtherSRS"));
        assert!(!others.is_empty(), "{name}");
        for o in &others {
            assert!(o.starts_with("urn:"), "{name}: {o}");
            assert_ne!(o, &default, "{name}: native SRS repeated");
        }
    }
}

/// GetCapabilitiesTest.testGetSections
#[actix_web::test]
async fn gs_get_capabilities_test_get_sections() {
    let srv = geoserver::server().await;
    let cases: &[(&str, [usize; 5])] = &[
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
    ];
    let paths = [
        "//ows:ServiceIdentification",
        "//ows:ServiceProvider",
        "//ows:OperationsMetadata",
        "//wfs:FeatureTypeList",
        "//ogc:Filter_Capabilities",
    ];
    for (sections, expected) in cases {
        let r = srv
            .get(&format!(
                "/wfs?service=WFS&version=1.1.0&request=GetCapabilities&sections={sections}"
            ))
            .await;
        r.assert_ok();
        let xml = r.xml();
        for (i, (p, n)) in paths.iter().zip(expected).enumerate() {
            // deviation: the WFS 1.1 schema declares ogc:Filter_Capabilities mandatory, so bbox
            // always includes it (GeoServer omits it when not requested)
            let n = if i == 4 { 1 } else { *n };
            assert_eq!(xml.count(p), n, "sections={sections} {p}");
        }
    }
    let r = srv
        .get("/wfs?service=WFS&version=1.1.0&request=GetCapabilities&sections=FooBar")
        .await;
    assert_ows10_exception(&r, "InvalidParameterValue", Some("sections"));
}

// ---------------------------------------------------------------- CapabilitiesTransformerTest

/// CapabilitiesTransformerTest.test (schema valid capabilities)
#[actix_web::test]
async fn gs_capabilities_transformer_test_test() {
    let srv = geoserver::server().await;
    let r = srv
        .get("/wfs?service=WFS&request=GetCapabilities&version=1.1.0")
        .await;
    r.assert_ok();
    r.xml().assert_valid(WFS11_XSD);
}

/// CapabilitiesTransformerTest.testDefaultOutputFormat
#[actix_web::test]
async fn gs_capabilities_transformer_test_default_output_format() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?service=WFS&request=GetCapabilities&version=1.1.0")
        .await
        .xml();
    for op in ["DescribeFeatureType", "GetFeature"] {
        assert_eq!(
            xml.string(&format!(
                "//ows:Operation[@name='{op}']/ows:Parameter[@name='outputFormat']/ows:Value[1]"
            )),
            "text/xml; subtype=gml/3.1.1",
            "{op}"
        );
    }
}

// ---------------------------------------------------------------- VersionNegotiationTest

async fn negotiated(srv: &TestServer, accept: &str) -> String {
    let r = srv
        .get(&format!(
            "/wfs?service=WFS&request=GetCapabilities&acceptversions={accept}"
        ))
        .await;
    r.assert_ok();
    r.xml().string("/*/@version")
}

/// VersionNegotiationTest.test0
#[actix_web::test]
#[ignore = "GeoServer picks the highest listed version; OWS 1.1 (7.3.2) selects the first supported version in client preference order, as bbox does"]
async fn gs_version_negotiation_test_test0() {
    let srv = geoserver::server().await;
    assert_eq!(negotiated(&srv, "1.0.0,1.1.0").await, "1.1.0");
}

/// VersionNegotiationTest.test1
#[actix_web::test]
async fn gs_version_negotiation_test_test1() {
    let srv = geoserver::server().await;
    assert_eq!(negotiated(&srv, "1.0.0").await, "1.0.0");
}

/// VersionNegotiationTest.test2
#[actix_web::test]
async fn gs_version_negotiation_test_test2() {
    let srv = geoserver::server().await;
    assert_eq!(negotiated(&srv, "1.1.0").await, "1.1.0");
}

/// VersionNegotiationTest.test5 (lower than any supported version: lowest supported)
#[actix_web::test]
#[ignore = "GeoServer falls back to a nearby version; OWS requires VersionNegotiationFailed when no listed version is supported, as bbox does"]
async fn gs_version_negotiation_test_test5() {
    let srv = geoserver::server().await;
    assert_eq!(negotiated(&srv, "0.0.0").await, "1.0.0");
}

/// VersionNegotiationTest.test6 (unknown 1.1.1: highest supported not above it)
#[actix_web::test]
#[ignore = "GeoServer falls back to a nearby version; OWS requires VersionNegotiationFailed when no listed version is supported, as bbox does"]
async fn gs_version_negotiation_test_test6() {
    let srv = geoserver::server().await;
    assert_eq!(negotiated(&srv, "1.1.1").await, "1.1.0");
}

/// VersionNegotiationTest.test7 (1.0.5: highest supported not above it)
#[actix_web::test]
#[ignore = "GeoServer falls back to a nearby version; OWS requires VersionNegotiationFailed when no listed version is supported, as bbox does"]
async fn gs_version_negotiation_test_test7() {
    let srv = geoserver::server().await;
    assert_eq!(negotiated(&srv, "1.0.5").await, "1.0.0");
}

// ---------------------------------------------------------------- WFSXmlTest

/// WFSXmlTest.testValid (unprefixed typeName, versioned URN srsName)
#[actix_web::test]
async fn gs_wfs_xml_test_valid() {
    let srv = geoserver::server().await;
    let r = srv
        .post_wfs(r#"<wfs:GetFeature xmlns:wfs="http://www.opengis.net/wfs" service="WFS" version="1.1.0"><wfs:Query typeName="PrimitiveGeoFeature" srsName="urn:x-ogc:def:crs:EPSG:6.11.2:4326"/></wfs:GetFeature>"#)
        .await;
    r.assert_ok();
    let xml = r.xml();
    xml.assert("/wfs:FeatureCollection");
    xml.assert_count("//sf:PrimitiveGeoFeature", 5);
}

// ---------------------------------------------------------------- DescribeFeatureTypeTest

fn dft_post(type_names: &[&str], ns_decl: bool) -> String {
    let names: String = type_names
        .iter()
        .map(|t| format!("<wfs:TypeName>{t}</wfs:TypeName>"))
        .collect();
    let decl = if ns_decl {
        r#" xmlns:sf="http://cite.opengeospatial.org/gmlsf""#
    } else {
        ""
    };
    format!(
        r#"<wfs:DescribeFeatureType service="WFS" version="1.1.0" xmlns:wfs="http://www.opengis.net/wfs"{decl}>{names}</wfs:DescribeFeatureType>"#
    )
}

fn type_local(t: &str) -> &str {
    t.rsplit(':').next().unwrap_or(t)
}

/// DescribeFeatureTypeTest.testDateMappings
#[actix_web::test]
async fn gs_describe_feature_type_test_date_mappings() {
    let srv = geoserver::server().await;
    let r = srv
        .post_wfs(&dft_post(&["sf:PrimitiveGeoFeature"], true))
        .await;
    r.assert_ok();
    let xml = r.xml();
    xml.assert("/xs:schema");
    assert_eq!(
        type_local(&xml.string("//xs:element[@name='dateProperty']/@type")),
        "date"
    );
    assert_eq!(
        type_local(&xml.string("//xs:element[@name='dateTimeProperty']/@type")),
        "dateTime"
    );
}

/// DescribeFeatureTypeTest.testNoNamespaceDeclaration
#[actix_web::test]
async fn gs_describe_feature_type_test_no_namespace_declaration() {
    let srv = geoserver::server().await;
    let r = srv
        .post_wfs(&dft_post(&["sf:PrimitiveGeoFeature"], false))
        .await;
    r.assert_ok();
    r.xml().assert("/xs:schema");
}

/// DescribeFeatureTypeTest.testMultipleTypesImport
#[actix_web::test]
async fn gs_describe_feature_type_test_multiple_types_import() {
    let srv = geoserver::server().await;
    let r = srv
        .post_wfs(&dft_post(
            &["sf:PrimitiveGeoFeature", "sf:GenericEntity"],
            true,
        ))
        .await;
    r.assert_ok();
    let xml = r.xml();
    xml.assert("/xs:schema");
    // all imports precede the first complex type
    xml.assert_count(
        "/xs:schema/xs:complexType[1]/following-sibling::xs:import",
        0,
    );
    // GeoServer's Content-Disposition `filename=schema.xsd` header is a download hint, not ported
}

/// DescribeFeatureTypeTest.testUerSuppliedTypeNameNamespace
#[actix_web::test]
async fn gs_describe_feature_type_test_user_supplied_type_name_namespace() {
    let srv = geoserver::server().await;
    let r = srv
        .get("/wfs?service=WFS&version=1.1.0&request=DescribeFeatureType&typeName=myPrefix:Polygons&namespace=xmlns(myPrefix%3Dhttp%3A%2F%2Fwww.opengis.net%2Fcite%2Fgeometry)")
        .await;
    r.assert_ok();
    r.xml().assert("/xs:schema");
}

/// DescribeFeatureTypeTest.testUerSuppliedTypeNameDefaultNamespace
#[actix_web::test]
async fn gs_describe_feature_type_test_user_supplied_type_name_default_namespace() {
    let srv = geoserver::server().await;
    let r = srv
        .get("/wfs?service=WFS&version=1.1.0&request=DescribeFeatureType&typeName=Polygons&namespace=xmlns(http%3A%2F%2Fwww.opengis.net%2Fcite%2Fgeometry)")
        .await;
    r.assert_ok();
    r.xml().assert("/xs:schema");
}

/// DescribeFeatureTypeTest.testMissingNameNamespacePrefix (unqualified name resolved leniently)
#[actix_web::test]
async fn gs_describe_feature_type_test_missing_name_namespace_prefix() {
    let srv = geoserver::server().await;
    let r = srv
        .get("/wfs?service=WFS&version=1.1.0&request=DescribeFeatureType&typeName=Polygons")
        .await;
    r.assert_ok();
    r.xml().assert("/xs:schema");
}

/// DescribeFeatureTypeTest.testCiteCompliance (non-strict branch: unqualified cite type)
#[actix_web::test]
async fn gs_describe_feature_type_test_cite_compliance() {
    let srv = geoserver::server().await;
    let r = srv
        .get("/wfs?service=WFS&version=1.1.0&request=DescribeFeatureType&typeName=Streams")
        .await;
    r.assert_ok();
    r.xml().assert("/xs:schema");
}

/// DescribeFeatureTypeTest.testPrefixedGetStrictCite
#[actix_web::test]
async fn gs_describe_feature_type_test_prefixed_get_strict_cite() {
    let srv = geoserver::server().await;
    let r = srv
        .get("/wfs?service=WFS&version=1.1.0&request=DescribeFeatureType&typeName=cgf:Polygons")
        .await;
    r.assert_ok();
    r.xml().assert("/xs:schema");
}

/// DescribeFeatureTypeTest.testGML32OutputFormat (GML 3.2 schema on 1.1 must not fail)
#[actix_web::test]
async fn gs_describe_feature_type_test_gml32_output_format() {
    let srv = geoserver::server().await;
    let r = srv
        .get("/wfs?service=WFS&version=1.1.0&request=DescribeFeatureType&outputFormat=text/xml;+subtype%3Dgml/3.2&typename=cgf:Polygons")
        .await;
    r.assert_ok();
    r.xml().assert("/xs:schema");
}

/// DescribeFeatureTypeTest.testGMLAttributeMapping (default: name/description inherited from gml)
#[actix_web::test]
async fn gs_describe_feature_type_test_gml_attribute_mapping() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?service=WFS&version=1.1.0&request=DescribeFeatureType&typename=sf:PrimitiveGeoFeature")
        .await
        .xml();
    xml.assert_count("//xs:element[@name='name']", 0);
    xml.assert_count("//xs:element[@name='description']", 0);
}

/// DescribeFeatureTypeTest.testNoWfsSchemaImport
#[actix_web::test]
async fn gs_describe_feature_type_test_no_wfs_schema_import() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?service=WFS&version=1.1.0&request=DescribeFeatureType&typeName=cgf:Polygons")
        .await
        .xml();
    xml.assert("//xs:complexType[@name='PolygonsType']");
    xml.assert("//xs:element[@name='Polygons']");
    xml.assert("//xs:import[@namespace='http://www.opengis.net/gml']");
    xml.assert_count("//xs:import[@namespace='http://www.opengis.net/wfs']", 0);
}

// ---------------------------------------------------------------- DescribeFeatureResponseTest

/// DescribeFeatureResponseTest.testSingle
#[actix_web::test]
async fn gs_describe_feature_response_test_single() {
    let srv = geoserver::server().await;
    let xml = srv.get("/wfs?service=WFS&version=1.1.0&request=DescribeFeatureType&typeName=cite:BasicPolygons").await.xml();
    xml.assert("/xs:schema");
    xml.assert_count("//xs:complexType", 1);
}

/// DescribeFeatureResponseTest.testWithDifferntNamespaces
#[actix_web::test]
async fn gs_describe_feature_response_test_with_different_namespaces() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?service=WFS&version=1.1.0&request=DescribeFeatureType&typeName=cite:BasicPolygons,cgf:Polygons")
        .await
        .xml();
    xml.assert("/xs:schema");
    xml.assert_count("/xs:schema/xs:import", 2);
}

// ---------------------------------------------------------------- GetFeatureTest (v1_1)

const FIFTEEN: &str = "/wfs?request=GetFeature&typename=cdf:Fifteen&version=1.1.0&service=wfs";

/// GetFeatureTest.testGet
#[actix_web::test]
async fn gs_get_feature_test_get() {
    let srv = geoserver::server().await;
    assert_fifteen_all(&srv.get(FIFTEEN).await);
}

/// GetFeatureTest.testGetPropertyNameEmpty
#[actix_web::test]
async fn gs_get_feature_test_get_property_name_empty() {
    let srv = geoserver::server().await;
    let r = srv.get(&format!("{FIFTEEN}&propertyname=")).await;
    assert_fifteen_all(&r);
    r.xml().assert("//cdf:Fifteen/cdf:pointProperty");
}

/// GetFeatureTest.testGetPropertyNameStar
#[actix_web::test]
async fn gs_get_feature_test_get_property_name_star() {
    let srv = geoserver::server().await;
    let r = srv.get(&format!("{FIFTEEN}&propertyname=*")).await;
    assert_fifteen_all(&r);
    r.xml().assert("//cdf:Fifteen/cdf:pointProperty");
}

/// GetFeatureTest.testGetPropertyNameOneValueServiceNotSet (cql_filter, no service parameter)
#[actix_web::test]
async fn gs_get_feature_test_get_property_name_one_value_service_not_set() {
    let srv = geoserver::server().await;
    let r = srv
        .get("/wfs?request=GetFeature&typename=cite:Ponds&version=1.1.0&cql_filter=TYPE%3D%27Stock%20Pond%27&propertyname=TYPE")
        .await;
    r.assert_ok();
    let xml = r.xml();
    let ponds = format!("//{}", cite("Ponds"));
    let n = xml.count(&ponds);
    assert!(n > 0, "{}", r.body);
    xml.assert_count(&format!("{ponds}/{}", cite("TYPE")), n);
    xml.assert_count(&format!("{ponds}/{}", cite("NAME")), 0);
}

/// GetFeatureTest.testGetWithFeatureId
#[actix_web::test]
async fn gs_get_feature_test_get_with_feature_id() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?request=GetFeature&typeName=cdf:Fifteen&version=1.1.0&service=wfs&featureid=Fifteen.2")
        .await
        .xml();
    xml.assert_count("//cdf:Fifteen", 1);
    xml.assert("//cdf:Fifteen[@gml:id='Fifteen.2']");
    let xml = srv
        .get("/wfs?request=GetFeature&typeName=cite:NamedPlaces&version=1.1.0&service=wfs&featureId=NamedPlaces.1107531895891")
        .await
        .xml();
    let np = format!("//{}", cite("NamedPlaces"));
    xml.assert_count(&np, 1);
    xml.assert(&format!("{np}[@gml:id='NamedPlaces.1107531895891']"));
}

/// GetFeatureTest.testGetWithTwoFeatureId (type inferred from feature ids)
#[actix_web::test]
async fn gs_get_feature_test_get_with_two_feature_id() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?request=GetFeature&&version=1.1.0&service=wfs&featureid=Fifteen.1,Fifteen.2")
        .await
        .xml();
    xml.assert_count("//cdf:Fifteen", 2);
    xml.assert("//cdf:Fifteen[@gml:id='Fifteen.1']");
    xml.assert("//cdf:Fifteen[@gml:id='Fifteen.2']");
}

const OTHER_STRING2: &str = r#"<wfs:Query typeName="cdf:Other"><wfs:PropertyName>cdf:string2</wfs:PropertyName></wfs:Query>"#;

/// GetFeatureTest.testPost
#[actix_web::test]
async fn gs_get_feature_test_post() {
    let srv = geoserver::server().await;
    let r = srv.post_wfs(&post_get_feature(OTHER_STRING2, "")).await;
    r.assert_ok();
    let xml = r.xml();
    xml.assert("/wfs:FeatureCollection");
    assert!(xml.count("//cdf:Other") > 0);
    assert_eq!(xml.count("//cdf:Other[@gml:id]"), xml.count("//cdf:Other"));
    xml.assert("//cdf:Other/cdf:string2");
}

/// GetFeatureTest.testPostSOAP12
#[actix_web::test]
async fn gs_get_feature_test_post_soap12() {
    let srv = geoserver::server().await;
    let body = format!(
        "<soap:Envelope xmlns:soap='http://www.w3.org/2003/05/soap-envelope'><soap:Header/><soap:Body>{}</soap:Body></soap:Envelope>",
        post_get_feature(OTHER_STRING2, "")
    );
    let r = srv
        .post_with_type("/wfs", &body, "application/soap+xml")
        .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(
        r.content_type.starts_with("application/soap+xml"),
        "{}",
        r.content_type
    );
    let xml = r.xml();
    xml.assert("/*[local-name()='Envelope']/*[local-name()='Body']/wfs:FeatureCollection");
    xml.assert_count("//cdf:Other", 1);
}

/// GetFeatureTest.testPostFormEncoded
#[actix_web::test]
async fn gs_get_feature_test_post_form_encoded() {
    let srv = geoserver::server().await;
    let r = srv
        .post_with_type(
            "/wfs",
            "service=WFS&version=1.1.0&request=GetFeature&typename=sf:PrimitiveGeoFeature&namespace=xmlns(sf%3Dhttp%3A%2F%2Fcite.opengeospatial.org%2Fgmlsf)",
            "application/x-www-form-urlencoded",
        )
        .await;
    r.assert_ok();
    let xml = r.xml();
    xml.assert("/wfs:FeatureCollection");
    xml.assert_count("//sf:PrimitiveGeoFeature", 5);
}

/// GetFeatureTest.testPostWithFilter (arithmetic in filter)
#[actix_web::test]
async fn gs_get_feature_test_post_with_filter() {
    let srv = geoserver::server().await;
    let q = r#"<wfs:Query typeName="cdf:Other"><ogc:Filter><ogc:PropertyIsEqualTo><ogc:PropertyName>cdf:integers</ogc:PropertyName><ogc:Add><ogc:Literal>4</ogc:Literal><ogc:Literal>3</ogc:Literal></ogc:Add></ogc:PropertyIsEqualTo></ogc:Filter></wfs:Query>"#;
    let r = srv
        .post_wfs(&post_get_feature(
            q,
            r#"outputFormat="text/xml; subtype=gml/3.1.1""#,
        ))
        .await;
    r.assert_ok();
    let xml = r.xml();
    let n = xml.count("//cdf:Other");
    assert!(n > 0, "{}", r.body);
    assert_eq!(xml.count("//cdf:Other[@gml:id]"), n);
}

fn bbox_query(srs: &str, lower: &str, upper: &str) -> String {
    post_get_feature(
        &format!(
            r#"<wfs:Query typeName="sf:PrimitiveGeoFeature"><ogc:Filter><ogc:BBOX><ogc:PropertyName>pointProperty</ogc:PropertyName><gml:Envelope srsName="{srs}"><gml:lowerCorner>{lower}</gml:lowerCorner><gml:upperCorner>{upper}</gml:upperCorner></gml:Envelope></ogc:BBOX></ogc:Filter></wfs:Query>"#
        ),
        "",
    )
}

/// GetFeatureTest.testPostWithBboxFilter (EPSG:4326 short code: x/y order)
#[actix_web::test]
async fn gs_get_feature_test_post_with_bbox_filter() {
    let srv = geoserver::server().await;
    let r = srv
        .post_wfs(&bbox_query("EPSG:4326", "57.0 -4.5", "62.0 1.0"))
        .await;
    r.assert_ok();
    let xml = r.xml();
    xml.assert_count("//sf:PrimitiveGeoFeature", 1);
    xml.assert("//sf:PrimitiveGeoFeature[@gml:id='PrimitiveGeoFeature.f002']");
}

/// GetFeatureTest.testPostWithFailingUrnBboxFilter (URN: lat/lon order)
#[actix_web::test]
async fn gs_get_feature_test_post_with_failing_urn_bbox_filter() {
    let srv = geoserver::server().await;
    let r = srv
        .post_wfs(&bbox_query(
            "urn:x-ogc:def:crs:EPSG:6.11.2:4326",
            "57.0 -4.5",
            "62.0 1.0",
        ))
        .await;
    r.assert_ok();
    r.xml().assert_count("//sf:PrimitiveGeoFeature", 0);
}

/// GetFeatureTest.testPostWithMatchingUrnBboxFilter
#[actix_web::test]
async fn gs_get_feature_test_post_with_matching_urn_bbox_filter() {
    let srv = geoserver::server().await;
    let r = srv
        .post_wfs(&bbox_query(
            "urn:x-ogc:def:crs:EPSG:6.11.2:4326",
            "-4.5 57.0",
            "1.0 62.0",
        ))
        .await;
    r.assert_ok();
    let xml = r.xml();
    xml.assert("/wfs:FeatureCollection");
    xml.assert("//sf:PrimitiveGeoFeature");
}

/// GetFeatureTest.testResultTypeHitsGet
#[actix_web::test]
async fn gs_get_feature_test_result_type_hits_get() {
    let srv = geoserver::server().await;
    let r = srv.get(&format!("{FIFTEEN}&resultType=hits")).await;
    r.assert_ok();
    let xml = r.xml();
    xml.assert("/wfs:FeatureCollection[@numberOfFeatures='15']");
    xml.assert_count("//cdf:Fifteen", 0);
}

/// GetFeatureTest.testResultTypeHitsPost
#[actix_web::test]
async fn gs_get_feature_test_result_type_hits_post() {
    let srv = geoserver::server().await;
    let r = srv
        .post_wfs(&post_get_feature(
            r#"<wfs:Query typeName="cdf:Seven"/>"#,
            r#"outputFormat="text/xml; subtype=gml/3.1.1" resultType="hits""#,
        ))
        .await;
    r.assert_ok();
    let xml = r.xml();
    xml.assert("/wfs:FeatureCollection[@numberOfFeatures='7']");
    xml.assert_count("//cdf:Seven", 0);
}

/// GetFeatureTest.testWithSRS
#[actix_web::test]
async fn gs_get_feature_test_with_srs() {
    let srv = geoserver::server().await;
    let r = srv
        .post_wfs(&post_get_feature(
            r#"<wfs:Query typeName="cdf:Other" srsName="urn:x-ogc:def:crs:EPSG:6.11.2:4326"/>"#,
            "",
        ))
        .await;
    r.assert_ok();
    r.xml().assert_count("//cdf:Other", 1);
}

/// GetFeatureTest.testWithSillyLiteral (wfs:Native inside a literal: no match, no exception)
#[actix_web::test]
async fn gs_get_feature_test_with_silly_literal() {
    let srv = geoserver::server().await;
    let q = r#"<wfs:Query typeName="cdf:Other" srsName="urn:x-ogc:def:crs:EPSG:6.11.2:4326"><ogc:Filter><ogc:PropertyIsEqualTo><ogc:PropertyName>description</ogc:PropertyName><ogc:Literal><wfs:Native vendorId="foo" safeToIgnore="true"/></ogc:Literal></ogc:PropertyIsEqualTo></ogc:Filter></wfs:Query>"#;
    let r = srv.post_wfs(&post_get_feature(q, "")).await;
    r.assert_ok();
    let xml = r.xml();
    xml.assert("/wfs:FeatureCollection");
    xml.assert_count("//cdf:Other", 0);
}

/// GetFeatureTest.testWithGmlObjectId
#[actix_web::test]
async fn gs_get_feature_test_with_gml_object_id() {
    let srv = geoserver::server().await;
    let xml = srv
        .post_wfs(&post_get_feature(
            r#"<wfs:Query typeName="cdf:Seven" srsName="urn:x-ogc:def:crs:EPSG:6.11.2:4326"/>"#,
            "",
        ))
        .await
        .xml();
    xml.assert_count("//cdf:Seven", 7);
    let id = xml.string("(//cdf:Seven)[1]/@gml:id");
    assert!(!id.is_empty());
    let q = format!(
        r#"<wfs:Query typeName="cdf:Seven" srsName="urn:x-ogc:def:crs:EPSG:6.11.2:4326"><ogc:Filter><ogc:GmlObjectId gml:id="{id}"/></ogc:Filter></wfs:Query>"#
    );
    let xml = srv.post_wfs(&post_get_feature(&q, "")).await.xml();
    xml.assert_count("//cdf:Seven", 1);
}

/// GetFeatureTest.testUserSuppliedNamespacePrefix
#[actix_web::test]
async fn gs_get_feature_test_user_supplied_namespace_prefix() {
    let srv = geoserver::server().await;
    let r = srv
        .get("/wfs?request=GetFeature&typename=myPrefix:Fifteen&version=1.1.0&service=wfs&namespace=xmlns(myPrefix%3Dhttp%3A%2F%2Fwww.opengis.net%2Fcite%2Fdata)")
        .await;
    assert_fifteen_all(&r);
}

/// GetFeatureTest.testUserSuppliedDefaultNamespace
#[actix_web::test]
async fn gs_get_feature_test_user_supplied_default_namespace() {
    let srv = geoserver::server().await;
    let r = srv
        .get("/wfs?request=GetFeature&typename=Fifteen&version=1.1.0&service=wfs&namespace=xmlns(http%3A%2F%2Fwww.opengis.net%2Fcite%2Fdata)")
        .await;
    assert_fifteen_all(&r);
}

/// GetFeatureTest.testGMLAttributeMapping (default: name/description encoded as gml properties)
#[actix_web::test]
async fn gs_get_feature_test_gml_attribute_mapping() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?service=WFS&version=1.1.0&request=GetFeature&typename=sf:PrimitiveGeoFeature")
        .await
        .xml();
    xml.assert("//sf:PrimitiveGeoFeature/gml:name");
    xml.assert("//sf:PrimitiveGeoFeature/gml:description");
    xml.assert_count("//sf:name", 0);
    xml.assert_count("//sf:description", 0);
}

async fn buildings_fids(srv: &TestServer, params: &str) -> Vec<String> {
    let r = srv
        .get(&format!(
            "/wfs?request=GetFeature&typename=cite:Buildings&version=1.1.0&service=wfs&{params}"
        ))
        .await;
    r.assert_ok();
    r.xml()
        .strings(&format!("//{}/{}", cite("Buildings"), cite("FID")))
}

/// GetFeatureTest.testSortedAscending
#[actix_web::test]
async fn gs_get_feature_test_sorted_ascending() {
    let srv = geoserver::server().await;
    assert_eq!(buildings_fids(&srv, "sortBy=ADDRESS").await, ["113", "114"]);
    assert_eq!(
        buildings_fids(&srv, "sortBy=ADDRESS&maxFeatures=1").await,
        ["113"]
    );
    assert_eq!(
        buildings_fids(&srv, "sortBy=ADDRESS&maxFeatures=1&startIndex=0").await,
        ["113"]
    );
    assert_eq!(
        buildings_fids(&srv, "sortBy=ADDRESS&maxFeatures=1&startIndex=1").await,
        ["114"]
    );
}

/// GetFeatureTest.testSortedDescending
#[actix_web::test]
async fn gs_get_feature_test_sorted_descending() {
    let srv = geoserver::server().await;
    assert_eq!(
        buildings_fids(&srv, "sortBy=ADDRESS%20D").await,
        ["114", "113"]
    );
    assert_eq!(
        buildings_fids(&srv, "sortBy=ADDRESS%20D&maxFeatures=1").await,
        ["114"]
    );
    assert_eq!(
        buildings_fids(&srv, "sortBy=ADDRESS%20D&maxFeatures=1&startIndex=0").await,
        ["114"]
    );
    assert_eq!(
        buildings_fids(&srv, "sortBy=ADDRESS%20D&maxFeatures=1&startIndex=1").await,
        ["113"]
    );
}

/// GetFeatureTest.testSortedInvalidAttribute
#[actix_web::test]
async fn gs_get_feature_test_sorted_invalid_attribute() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?request=GetFeature&typename=cite:Buildings&version=1.1.0&service=wfs&sortBy=GODOT").await;
    let xml = assert_ows10_exception(&r, "InvalidParameterValue", None);
    assert!(
        xml.string("//ows:ExceptionText").contains("GODOT"),
        "{}",
        r.body
    );
}

/// GetFeatureTest.testEncodeSrsDimension (GeoServer default emits srsDimension="2" on points)
#[actix_web::test]
#[ignore = "GeoServer encoding choice: srsDimension on 2D points is optional; bbox emits it only for 3D"]
async fn gs_get_feature_test_encode_srs_dimension() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?request=GetFeature&version=1.1.0&service=wfs&typename=sf:PrimitiveGeoFeature")
        .await
        .xml();
    xml.assert("//gml:Point[@srsDimension='2']");
}

/// GetFeatureTest.testWfs20AndGML31 (outputFormat=gml3 on WFS 2.0 gives a 1.1 collection)
#[actix_web::test]
async fn gs_get_feature_test_wfs20_and_gml31() {
    let srv = geoserver::server().await;
    let r = srv
        .get("/wfs?request=GetFeature&typeName=cdf:Fifteen&version=2.0.0&service=wfs&featureid=Fifteen.2&outputFormat=gml3")
        .await;
    r.assert_ok();
    let xml = r.xml();
    xml.assert("/wfs:FeatureCollection");
    xml.assert_count("/wfs:FeatureCollection/*/cdf:Fifteen", 1);
    xml.assert("//cdf:Fifteen[@gml:id='Fifteen.2']");
}

/// GetFeatureTest.testNPEOnPaginationLinks (concurrent requests)
#[actix_web::test]
async fn gs_get_feature_test_npe_on_pagination_links() {
    let srv = geoserver::server().await;
    let body = bbox_query("EPSG:4326", "57.0 -4.5", "62.0 1.0");
    let futs = (0..50).map(|_| srv.post_wfs(&body));
    for r in futures::future::join_all(futs).await {
        r.assert_ok();
        r.xml().assert("/wfs:FeatureCollection");
    }
}

async fn json_paging(srv: &TestServer, start: u32) -> serde_json::Value {
    let r = srv
        .get(&format!("/wfs?request=GetFeature&typenames=cdf:Fifteen&version=1.1.0&service=wfs&maxFeatures=5&startIndex={start}&outputFormat=JSON"))
        .await;
    r.assert_ok();
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body))
}

/// GetFeatureTest.testGetWithCountAndStartIndex0
#[actix_web::test]
async fn gs_get_feature_test_get_with_count_and_start_index0() {
    let srv = geoserver::server().await;
    let json = json_paging(&srv, 0).await;
    assert_eq!(json["features"].as_array().unwrap().len(), 5);
    assert_eq!(json["totalFeatures"], 15);
}

/// GetFeatureTest.testGetWithCountAndStartIndexMiddle
#[actix_web::test]
async fn gs_get_feature_test_get_with_count_and_start_index_middle() {
    let srv = geoserver::server().await;
    let json = json_paging(&srv, 7).await;
    assert_eq!(json["features"].as_array().unwrap().len(), 5);
    assert_eq!(json["totalFeatures"], 15);
}

/// GetFeatureTest.testGetWithCountAndStartIndexEnd
#[actix_web::test]
async fn gs_get_feature_test_get_with_count_and_start_index_end() {
    let srv = geoserver::server().await;
    let json = json_paging(&srv, 11).await;
    assert_eq!(json["features"].as_array().unwrap().len(), 4);
    assert_eq!(json["totalFeatures"], 15);
}

/// GetFeatureTest.testNoGmlIdOnGeometry
#[actix_web::test]
async fn gs_get_feature_test_no_gml_id_on_geometry() {
    let srv = geoserver::server().await;
    let r = srv
        .get("/wfs?request=GetFeature&typeName=cite:NamedPlaces&version=1.1.0&service=wfs&featureId=NamedPlaces.1107531895891")
        .await;
    r.assert_ok();
    let xml = r.xml();
    let geom = format!(
        "//{}/{}/gml:MultiSurface",
        cite("NamedPlaces"),
        cite("the_geom")
    );
    xml.assert(&geom);
    xml.assert_count(&format!("{geom}/gml:surfaceMember/gml:Polygon[@gml:id]"), 0);
}

// ---------------------------------------------------------------- WFS11MultiPolygonAsMultiSurfaceTest

/// WFS11MultiPolygonAsMultiSurfaceTest.testGeometryConsistency (adapted to cite:Buildings)
#[actix_web::test]
async fn gs_wfs11_multi_polygon_as_multi_surface_test_geometry_consistency() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?request=DescribeFeatureType&version=1.1.0&service=wfs&typename=cite:Buildings")
        .await
        .xml();
    xml.assert("//xs:element[@name='the_geom' and @type='gml:MultiSurfacePropertyType']");
    xml.assert_count("//xs:element[@type='gml:MultiPolygonPropertyType']", 0);
    let xml = srv
        .get("/wfs?request=GetFeature&version=1.1.0&service=wfs&typename=cite:Buildings")
        .await
        .xml();
    let geom = format!("//{}/{}", cite("Buildings"), cite("the_geom"));
    xml.assert(&format!("{geom}/gml:MultiSurface"));
    xml.assert_count(&format!("{geom}/gml:MultiPolygon"), 0);
}

// ---------------------------------------------------------------- WFSReprojectionTest

/// Bounds of the first gml:Box (GML2 coordinates `x,y x,y`)
fn first_box(xml: &Xml) -> [f64; 4] {
    let text = xml.string("(//gml:Box/gml:coordinates)[1]");
    let v: Vec<f64> = text
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap())
        .collect();
    assert_eq!(v.len(), 4, "box `{text}`");
    [v[0], v[1], v[2], v[3]]
}

/// Bounds of a rectangle transformed between EPSG codes
fn transform_box(b: [f64; 4], from: u16, to: u16) -> [f64; 4] {
    use geo::BoundingRect;
    let rect = geo::Rect::new(
        geo::coord! { x: b[0], y: b[1] },
        geo::coord! { x: b[2], y: b[3] },
    );
    let mut g = geo::Geometry::Polygon(rect.to_polygon());
    bbox_feature_server::wfs::crs::transform(&mut g, from, to).unwrap();
    let r = g.bounding_rect().unwrap();
    [r.min().x, r.min().y, r.max().x, r.max().y]
}

fn assert_box_close(a: [f64; 4], b: [f64; 4], tol: f64) {
    for i in 0..4 {
        assert!((a[i] - b[i]).abs() < tol, "{a:?} vs {b:?}");
    }
}

/// Native bounds of cgf:Polygons via WFS 1.1 (EPSG:32615, x/y)
async fn polygons_native_bounds(srv: &TestServer) -> [f64; 4] {
    let xml = srv
        .get("/wfs?request=GetFeature&service=wfs&version=1.0.0&typename=cgf:Polygons")
        .await
        .xml();
    first_box(&xml)
}

/// WFSReprojectionTest.testGetFeatureGet (EPSG:900913 replaced by EPSG:3857)
#[actix_web::test]
async fn gs_wfs_reprojection_test_get_feature_get() {
    let srv = geoserver::server().await;
    let native = polygons_native_bounds(&srv).await;
    let xml = srv
        .get(
            "/wfs?request=getfeature&service=wfs&version=1.0.0&typename=Polygons&srsName=EPSG:3857",
        )
        .await
        .xml();
    assert_box_close(first_box(&xml), transform_box(native, 32615, 3857), 0.001);
}

/// WFSReprojectionTest.testGetFeaturePost
#[actix_web::test]
async fn gs_wfs_reprojection_test_get_feature_post() {
    let srv = geoserver::server().await;
    let q = |srs: &str| {
        format!(
            r#"<wfs:GetFeature service="WFS" version="1.0.0" xmlns:cgf="http://www.opengis.net/cite/geometry" xmlns:wfs="http://www.opengis.net/wfs"><wfs:Query typeName="cgf:Polygons"{srs}><wfs:PropertyName>cgf:polygonProperty</wfs:PropertyName></wfs:Query></wfs:GetFeature>"#
        )
    };
    let native = first_box(&srv.post_wfs(&q("")).await.xml());
    let projected = first_box(&srv.post_wfs(&q(r#" srsName="EPSG:3857""#)).await.xml());
    assert_box_close(projected, transform_box(native, 32615, 3857), 0.001);
}

/// WFSReprojectionTest.testGetFeatureWithProjectedBoxGet
#[actix_web::test]
async fn gs_wfs_reprojection_test_get_feature_with_projected_box_get() {
    let srv = geoserver::server().await;
    let b = transform_box(polygons_native_bounds(&srv).await, 32615, 3857);
    let r = srv
        .get(&format!(
            "/wfs?request=getfeature&service=wfs&version=1.1.0&typeName=cgf:Polygons&bbox={},{},{},{},EPSG:3857",
            b[0], b[1], b[2], b[3]
        ))
        .await;
    r.assert_ok();
    r.xml().assert_count("//cgf:Polygons", 1);
}

/// WFSReprojectionTest.testGetFeatureWithProjectedBoxPost
#[actix_web::test]
async fn gs_wfs_reprojection_test_get_feature_with_projected_box_post() {
    let srv = geoserver::server().await;
    let b = transform_box(polygons_native_bounds(&srv).await, 32615, 3857);
    let body = format!(
        r#"<wfs:GetFeature service="WFS" version="1.1.0" xmlns:cgf="http://www.opengis.net/cite/geometry" xmlns:ogc="http://www.opengis.net/ogc" xmlns:gml="http://www.opengis.net/gml" xmlns:wfs="http://www.opengis.net/wfs"><wfs:Query typeName="cgf:Polygons"><wfs:PropertyName>cgf:polygonProperty</wfs:PropertyName><ogc:Filter><ogc:BBOX><ogc:PropertyName>polygonProperty</ogc:PropertyName><gml:Envelope srsName="EPSG:3857"><gml:lowerCorner>{} {}</gml:lowerCorner><gml:upperCorner>{} {}</gml:upperCorner></gml:Envelope></ogc:BBOX></ogc:Filter></wfs:Query></wfs:GetFeature>"#,
        b[0], b[1], b[2], b[3]
    );
    let r = srv.post_wfs(&body).await;
    r.assert_ok();
    r.xml().assert_count("//cgf:Polygons", 1);
}

/// WFSReprojectionTest.testFilterReprojection (filter geometry in Query srsName)
#[actix_web::test]
async fn gs_wfs_reprojection_test_filter_reprojection() {
    let srv = geoserver::server().await;
    let body = r#"<wfs:GetFeature service="WFS" version="1.0.0" xmlns:cgf="http://www.opengis.net/cite/geometry" xmlns:ogc="http://www.opengis.net/ogc" xmlns:gml="http://www.opengis.net/gml" xmlns:wfs="http://www.opengis.net/wfs"><wfs:Query typeName="cgf:Polygons" srsName="EPSG:3857"><ogc:Filter><ogc:Intersects><ogc:PropertyName>polygonProperty</ogc:PropertyName><gml:Point><gml:coordinates decimal="." cs="," ts=" ">-1.035246176730227E7,504135.14926478104</gml:coordinates></gml:Point></ogc:Intersects></ogc:Filter></wfs:Query> ></wfs:GetFeature>"#;
    let r = srv.post_wfs(body).await;
    r.assert_ok();
    r.xml().assert_count("//cgf:Polygons", 1);
}

// ---------------------------------------------------------------- Filter KVP parsing

/// Filter_1_0_0_KvpParserTest.test
#[actix_web::test]
async fn gs_filter_1_0_0_kvp_parser_test_test() {
    let srv = geoserver::server().await;
    let filter = r#"<ogc:Filter xmlns:ogc="http://www.opengis.net/ogc" xmlns:cdf="http://www.opengis.net/cite/data"><ogc:PropertyIsEqualTo><ogc:PropertyName>cdf:integers</ogc:PropertyName><ogc:Add><ogc:Literal>4</ogc:Literal><ogc:Literal>3</ogc:Literal></ogc:Add></ogc:PropertyIsEqualTo></ogc:Filter>"#;
    let r = srv
        .kvp(&[
            ("service", "WFS"),
            ("version", "1.0.0"),
            ("request", "GetFeature"),
            ("typename", "cdf:Other"),
            ("filter", filter),
        ])
        .await;
    r.assert_ok();
    r.xml().assert_count("//cdf:Other", 1);
}

/// Filter_1_0_0_KvpParserTest.testMultiFilter
#[actix_web::test]
async fn gs_filter_1_0_0_kvp_parser_test_multi_filter() {
    let srv = geoserver::server().await;
    let filter = r#"(<Filter xmlns="http://www.opengis.net/ogc"><FeatureId fid="Fifteen.3"/></Filter>)(<Filter xmlns="http://www.opengis.net/ogc"><FeatureId fid="Seven.3"/></Filter>)"#;
    let r = srv
        .kvp(&[
            ("service", "WFS"),
            ("version", "1.0.0"),
            ("request", "GetFeature"),
            ("typename", "cdf:Fifteen,cdf:Seven"),
            ("filter", filter),
        ])
        .await;
    r.assert_ok();
    let xml = r.xml();
    xml.assert_count("//cdf:Fifteen", 1);
    xml.assert_count("//cdf:Seven", 1);
    xml.assert("//cdf:Fifteen[@fid='Fifteen.3']");
    xml.assert("//cdf:Seven[@fid='Seven.3']");
}

/// Filter_1_0_0_KvpParserTest.testEmptyAndNonEmptyFilter
#[actix_web::test]
async fn gs_filter_1_0_0_kvp_parser_test_empty_and_non_empty_filter() {
    let srv = geoserver::server().await;
    let filter =
        r#"()(<Filter xmlns="http://www.opengis.net/ogc"><FeatureId fid="Seven.3"/></Filter>)"#;
    let r = srv
        .kvp(&[
            ("service", "WFS"),
            ("version", "1.0.0"),
            ("request", "GetFeature"),
            ("typename", "cdf:Fifteen,cdf:Seven"),
            ("filter", filter),
        ])
        .await;
    r.assert_ok();
    let xml = r.xml();
    xml.assert_count("//cdf:Fifteen", 15);
    xml.assert_count("//cdf:Seven", 1);
}

/// Filter_1_1_0_KvpParserTest.testParse (GML3 posList polygon in a 1.1 filter)
#[actix_web::test]
async fn gs_filter_1_1_0_kvp_parser_test_parse() {
    let srv = geoserver::server().await;
    let filter = r#"<ogc:Filter xmlns:ogc="http://www.opengis.net/ogc" xmlns:gml="http://www.opengis.net/gml"><ogc:Intersects><ogc:PropertyName>the_geom</ogc:PropertyName><gml:Polygon><gml:exterior><gml:LinearRing><gml:posList>-1 -1 1 -1 1 1 -1 1 -1 -1</gml:posList></gml:LinearRing></gml:exterior></gml:Polygon></ogc:Intersects></ogc:Filter>"#;
    let r = srv
        .kvp(&[
            ("service", "WFS"),
            ("version", "1.1.0"),
            ("request", "GetFeature"),
            ("typename", "cite:Buildings"),
            ("filter", filter),
        ])
        .await;
    r.assert_ok();
    r.xml().assert_count(&format!("//{}", cite("Buildings")), 2);
}

// ---------------------------------------------------------------- GetFeatureKvpRequestReaderTest

/// GetFeatureKvpRequestReaderTest.testInvalidTypeNameBbox
#[actix_web::test]
async fn gs_get_feature_kvp_request_reader_test_invalid_type_name_bbox() {
    let srv = geoserver::server().await;
    let r = srv
        .get("/wfs?service=WFS&version=1.1.0&request=GetFeature&typeName=cite:InvalidTypeName&bbox=-80.4864795578115,25.6176257083275,-80.3401307394915,25.7002737069969")
        .await;
    let xml = assert_ows10_exception(&r, "InvalidParameterValue", Some("typeName"));
    assert!(
        xml.string("//ows:ExceptionText")
            .contains("cite:InvalidTypeName"),
        "{}",
        r.body
    );
}

/// GetFeatureKvpRequestReaderTest.testInvalidTypeName
#[actix_web::test]
async fn gs_get_feature_kvp_request_reader_test_invalid_type_name() {
    let srv = geoserver::server().await;
    let r = srv
        .get("/wfs?service=WFS&version=1.1.0&request=GetFeature&typeName=InvalidTypeName")
        .await;
    let xml = assert_ows10_exception(&r, "InvalidParameterValue", Some("typeName"));
    assert!(
        xml.string("//ows:ExceptionText")
            .contains("InvalidTypeName"),
        "{}",
        r.body
    );
}

/// GetFeatureKvpRequestReaderTest.testUserProvidedNamespace (unencoded `=` in xmlns())
#[actix_web::test]
async fn gs_get_feature_kvp_request_reader_test_user_provided_namespace() {
    let srv = geoserver::server().await;
    let r = srv
        .get("/wfs?service=WFS&version=1.1.0&request=GetFeature&typeName=ex:MLines&namespace=xmlns(ex=http://www.opengis.net/cite/geometry)")
        .await;
    r.assert_ok();
    assert!(r.xml().count("//cgf:MLines") > 0, "{}", r.body);
}

/// GetFeatureKvpRequestReaderTest.testUserProvidedDefaultNamespace
#[actix_web::test]
async fn gs_get_feature_kvp_request_reader_test_user_provided_default_namespace() {
    let srv = geoserver::server().await;
    let r = srv
        .get("/wfs?service=WFS&version=1.1.0&request=GetFeature&typeName=Streams&namespace=xmlns(http://www.opengis.net/cite)")
        .await;
    r.assert_ok();
    assert!(
        r.xml().count(&format!("//{}", cite("Streams"))) > 0,
        "{}",
        r.body
    );
}

// ---------------------------------------------------------------- FeatureTypeInfoSchemaBuilderTest

/// FeatureTypeInfoSchemaBuilderTest.testBuildGml2 (observable via WFS 1.0 DescribeFeatureType)
#[actix_web::test]
async fn gs_feature_type_info_schema_builder_test_build_gml2() {
    let srv = geoserver::server().await;
    let xml = srv
        .get("/wfs?service=WFS&version=1.0.0&request=DescribeFeatureType&typeName=cgf:Lines")
        .await
        .xml();
    xml.assert("/xs:schema/xs:element[@name='Lines']");
    xml.assert("//xs:element[@name='id']");
    assert_eq!(
        xml.string("//xs:element[@name='lineStringProperty']/@type"),
        "gml:LineStringPropertyType"
    );
}

// ---------------------------------------------------------------- GML3FeatureProducerTest

async fn gml3_members(srv: &TestServer, types: &str) -> Xml {
    let r = srv
        .get(&format!(
            "/wfs?service=WFS&version=1.1.0&request=GetFeature&typename={types}"
        ))
        .await;
    r.assert_ok();
    r.xml()
}

/// GML3FeatureProducerTest.testSingle
#[actix_web::test]
async fn gs_gml3_feature_producer_test_single() {
    let srv = geoserver::server().await;
    gml3_members(&srv, "cdf:Seven")
        .await
        .assert_count("//cdf:Seven", 7);
}

/// GML3FeatureProducerTest.testMultipleSameNamespace
#[actix_web::test]
async fn gs_gml3_feature_producer_test_multiple_same_namespace() {
    let srv = geoserver::server().await;
    let xml = gml3_members(&srv, "cdf:Seven,cdf:Fifteen").await;
    xml.assert_count("//cdf:Seven", 7);
    xml.assert_count("//cdf:Fifteen", 15);
}

/// GML3FeatureProducerTest.testMultipleDifferentNamespace
#[actix_web::test]
async fn gs_gml3_feature_producer_test_multiple_different_namespace() {
    let srv = geoserver::server().await;
    let xml = gml3_members(&srv, "cdf:Seven,cgf:Polygons").await;
    xml.assert_count("//cdf:Seven", 7);
    xml.assert_count("//cgf:Polygons", 1);
}
