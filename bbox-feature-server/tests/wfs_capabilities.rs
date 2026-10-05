//! GetCapabilities, version negotiation and exception reports (WFS 1.0, 1.1, 2.0).
//! Assertions follow ets-wfs10, ets-wfs11, ets-wfs20 and the DGIWG WFS 2.0 profile.

mod common;
use common::*;

const WFS10_CAPS_XSD: &str = "http://schemas.opengis.net/wfs/1.0.0/WFS-capabilities.xsd";
const WFS11_XSD: &str = "http://schemas.opengis.net/wfs/1.1.0/wfs.xsd";
const WFS20_XSD: &str = "http://schemas.opengis.net/wfs/2.0/wfs.xsd";

// ---------------------------------------------------------------- WFS 1.0

#[actix_web::test]
async fn wfs10_capabilities_get() {
    let srv = cite10_server().await;
    let resp = srv
        .get("/wfs?service=WFS&version=1.0.0&request=GetCapabilities")
        .await;
    resp.assert_ok();
    assert!(resp.content_type.contains("xml"), "{}", resp.content_type);
    let xml = resp.xml();
    xml.assert("/wfs:WFS_Capabilities[@version='1.0.0']");
    xml.assert_valid(WFS10_CAPS_XSD);
    // basic-getcapabilities-5..8
    xml.assert("//wfs:Capability/wfs:Request/wfs:GetCapabilities/wfs:DCPType/wfs:HTTP/wfs:Get");
    xml.assert("//wfs:Capability/wfs:Request/wfs:DescribeFeatureType/wfs:DCPType/wfs:HTTP/wfs:Get");
    xml.assert(
        "//wfs:Capability/wfs:Request/wfs:DescribeFeatureType/wfs:DCPType/wfs:HTTP/wfs:Post",
    );
    xml.assert("//wfs:Capability/wfs:Request/wfs:GetFeature/wfs:DCPType/wfs:HTTP/wfs:Post");
    xml.assert("//wfs:Capability/wfs:Request/wfs:DescribeFeatureType/wfs:SchemaDescriptionLanguage/wfs:XMLSCHEMA");
    xml.assert("//wfs:Capability/wfs:Request/wfs:GetFeature/wfs:ResultFormat/wfs:GML2");
    xml.assert("//wfs:FeatureTypeList/wfs:Operations/wfs:Query or //wfs:FeatureTypeList/wfs:FeatureType/wfs:Operations/wfs:Query");
    // read-only service: no write operations advertised
    xml.assert_not("//wfs:Capability/wfs:Request/wfs:Transaction");
    xml.assert_not("//wfs:Capability/wfs:Request/wfs:LockFeature");
    xml.assert_not("//wfs:Capability/wfs:Request/wfs:GetFeatureWithLock");
    xml.assert_not("//wfs:Operations/wfs:Insert");
    // online resources are absolute and advertise the public URL
    assert!(xml
        .string("//wfs:GetFeature/wfs:DCPType/wfs:HTTP/wfs:Get/@onlineResource")
        .starts_with("http://localhost:8080/wfs"));
    // feature types of all CITE namespaces
    for name in [
        "cdf:Other",
        "cdf:Seven",
        "cdf:Fifteen",
        "cdf:Nulls",
        "cdf:Locks",
        "cdf:Deletes",
        "cdf:Inserts",
        "cdf:Updates",
        "cgf:Points",
        "cgf:Lines",
        "cgf:Polygons",
        "cgf:MPoints",
        "cgf:MLines",
        "cgf:MPolygons",
        "ccf:Complex",
    ] {
        xml.assert(&format!(
            "//wfs:FeatureType[wfs:Name='{name}']/wfs:SRS='EPSG:32615'"
        ));
    }
    xml.assert("//wfs:FeatureType[wfs:Name='cgf:Points']/wfs:LatLongBoundingBox");
    // filter capabilities with all operators (spatial intersects is singular in FE 1.0)
    for op in [
        "BBOX",
        "Beyond",
        "Contains",
        "Crosses",
        "Disjoint",
        "DWithin",
        "Equals",
        "Intersect",
        "Overlaps",
        "Touches",
        "Within",
    ] {
        xml.assert(&format!(
            "//ogc:Filter_Capabilities/ogc:Spatial_Capabilities/ogc:Spatial_Operators/ogc:{op}"
        ));
    }
    for op in ["Simple_Comparisons", "Like", "Between", "NullCheck"] {
        xml.assert(&format!(
            "//ogc:Scalar_Capabilities/ogc:Comparison_Operators/ogc:{op}"
        ));
    }
    xml.assert("//ogc:Scalar_Capabilities/ogc:Logical_Operators");
    xml.assert("//ogc:Scalar_Capabilities/ogc:Arithmetic_Operators/ogc:Simple_Arithmetic");
}

#[actix_web::test]
async fn wfs10_capabilities_post() {
    let srv = cite10_server().await;
    let resp = srv
        .post_wfs(r#"<?xml version="1.0" encoding="UTF-8"?><wfs:GetCapabilities service="WFS" version="1.0.0" xmlns:wfs="http://www.opengis.net/wfs"/>"#)
        .await;
    resp.assert_ok();
    let xml = resp.xml();
    xml.assert("/wfs:WFS_Capabilities[@version='1.0.0']");
    xml.assert_valid(WFS10_CAPS_XSD);
}

#[actix_web::test]
async fn version_negotiation_kvp_version_param() {
    let srv = cite10_server().await;
    // basic-getcapabilities-get-2: unknown higher version -> highest supported (never an exception)
    let xml = srv
        .get("/wfs?service=WFS&version=99.99.99&request=GetCapabilities")
        .await
        .xml();
    xml.assert("/*[local-name()='WFS_Capabilities']");
    assert_eq!(xml.string("/*/@version"), "2.0.2");
    // get-3: lower than any supported -> lowest supported
    let xml = srv
        .get("/wfs?service=WFS&version=0.99.99&request=GetCapabilities")
        .await
        .xml();
    assert_eq!(xml.string("/*/@version"), "1.0.0");
    // between supported versions -> next lower supported
    let xml = srv
        .get("/wfs?service=WFS&version=1.5.0&request=GetCapabilities")
        .await
        .xml();
    assert_eq!(xml.string("/*/@version"), "1.1.0");
    // get-4: no version -> highest
    let xml = srv
        .get("/wfs?service=WFS&request=GetCapabilities")
        .await
        .xml();
    assert_eq!(xml.string("/*/@version"), "2.0.2");
    // POST variants
    let xml = srv
        .post_wfs(r#"<wfs:GetCapabilities service="WFS" version="0.99.99" xmlns:wfs="http://www.opengis.net/wfs"/>"#)
        .await
        .xml();
    assert_eq!(xml.string("/*/@version"), "1.0.0");
    let xml = srv
        .post_wfs(r#"<wfs:GetCapabilities service="WFS" xmlns:wfs="http://www.opengis.net/wfs"/>"#)
        .await
        .xml();
    xml.assert("/*[local-name()='WFS_Capabilities']");
}

#[actix_web::test]
async fn kvp_parameter_names_are_case_insensitive() {
    let srv = cite10_server().await;
    let xml = srv
        .get("/wfs?SERVICE=WFS&Version=1.0.0&REQUEST=GetCapabilities")
        .await
        .xml();
    xml.assert("/wfs:WFS_Capabilities[@version='1.0.0']");
    // request value case-insensitive too (GeoServer leniency)
    let xml = srv
        .get("/wfs?service=wfs&version=1.0.0&request=getcapabilities")
        .await
        .xml();
    xml.assert("/wfs:WFS_Capabilities[@version='1.0.0']");
}

#[actix_web::test]
async fn wfs10_exceptions_are_http_200() {
    let srv = cite10_server().await;
    let resp = srv.get("/wfs?service=WFS&version=1.0.0&request=Foo").await;
    assert_eq!(resp.status, 200);
    let xml = resp.assert_exception("OperationNotSupported");
    xml.assert("/ogc:ServiceExceptionReport[@version='1.2.0']");
    xml.assert_valid("http://schemas.opengis.net/wfs/1.0.0/OGC-exception.xsd");
    // write operations are not supported
    for op in ["Transaction", "LockFeature", "GetFeatureWithLock"] {
        let resp = srv
            .get(&format!("/wfs?service=WFS&version=1.0.0&request={op}"))
            .await;
        resp.assert_exception("OperationNotSupported");
    }
}

// ---------------------------------------------------------------- WFS 1.1

#[actix_web::test]
async fn wfs11_capabilities() {
    let srv = cite11_server().await;
    let resp = srv
        .get("/wfs?service=WFS&version=1.1.0&request=GetCapabilities")
        .await;
    resp.assert_ok();
    let xml = resp.xml();
    xml.assert("/wfs:WFS_Capabilities[@version='1.1.0']");
    xml.assert_valid(WFS11_XSD);
    // Capabilities.sch RequiredBasicElementsPhase
    for e in [
        "ows:ServiceIdentification",
        "ows:ServiceProvider",
        "ows:OperationsMetadata",
        "wfs:FeatureTypeList",
        "ogc:Filter_Capabilities",
    ] {
        xml.assert(&format!("/wfs:WFS_Capabilities/{e}"));
    }
    xml.assert("//ows:ServiceIdentification/ows:ServiceType = 'WFS'");
    xml.assert("//ows:ServiceIdentification/ows:ServiceTypeVersion = '1.1.0'");
    for op in [
        "GetCapabilities",
        "DescribeFeatureType",
        "GetFeature",
        "GetGmlObject",
    ] {
        xml.assert(&format!(
            "//ows:Operation[@name='{op}']/ows:DCP/ows:HTTP/ows:Get/@xlink:href"
        ));
        xml.assert(&format!(
            "//ows:Operation[@name='{op}']/ows:DCP/ows:HTTP/ows:Post/@xlink:href"
        ));
    }
    for op in ["Transaction", "LockFeature", "GetFeatureWithLock"] {
        xml.assert_not(&format!("//ows:Operation[@name='{op}']"));
    }
    xml.assert(
        "//ows:Operation[@name='GetFeature']/ows:Parameter[@name='resultType']/ows:Value='hits'",
    );
    xml.assert("//ows:Operation[@name='GetFeature']/ows:Parameter[@name='outputFormat']/ows:Value='text/xml; subtype=gml/3.1.1'");
    // first feature type is probed by the active basic-cc tests
    assert_eq!(
        xml.string("//wfs:FeatureTypeList/wfs:FeatureType[1]/wfs:Name"),
        "sf:PrimitiveGeoFeature"
    );
    for name in [
        "sf:PrimitiveGeoFeature",
        "sf:AggregateGeoFeature",
        "sf:EntitéGénérique",
        "sf:ComplexGeoFeature",
        "sf:LinkedFeature",
    ] {
        let ft = format!("//wfs:FeatureType[wfs:Name='{name}']");
        xml.assert(&ft);
        let srs = xml.string(&format!("{ft}/wfs:DefaultSRS"));
        assert_eq!(srs, "urn:ogc:def:crs:EPSG::4326");
        xml.assert(&format!("{ft}/ows:WGS84BoundingBox/ows:LowerCorner"));
    }
    // WGS84BoundingBox is lon/lat
    let lower = xml.string(
        "//wfs:FeatureType[wfs:Name='sf:PrimitiveGeoFeature']/ows:WGS84BoundingBox/ows:LowerCorner",
    );
    let lon: f64 = lower.split_whitespace().next().unwrap().parse().unwrap();
    assert!((lon - -10.52).abs() < 1e-6, "{lower}");
    // Filter capabilities required by Capabilities.sch
    xml.assert("//ogc:Spatial_Capabilities/ogc:SpatialOperators/ogc:SpatialOperator[@name='BBOX']");
    xml.assert("//ogc:Id_Capabilities/ogc:EID");
    xml.assert("//ogc:Id_Capabilities/ogc:FID");
    for op in [
        "EqualTo",
        "NotEqualTo",
        "LessThan",
        "GreaterThan",
        "LessThanEqualTo",
        "GreaterThanEqualTo",
        "Like",
        "Between",
        "NullCheck",
    ] {
        xml.assert(&format!(
            "//ogc:ComparisonOperators/ogc:ComparisonOperator='{op}'"
        ));
    }
    xml.assert("//ogc:Scalar_Capabilities/ogc:LogicalOperators");
    xml.assert("//ogc:ArithmeticOperators/ogc:SimpleArithmetic");
    xml.assert("//ogc:ArithmeticOperators/ogc:Functions/ogc:FunctionNames/ogc:FunctionName");
}

#[actix_web::test]
async fn wfs11_capabilities_are_stable() {
    // tc6: two consecutive responses are identical
    let srv = cite11_server().await;
    let a = srv
        .get("/wfs?service=WFS&version=1.1.0&request=GetCapabilities")
        .await
        .body;
    let b = srv
        .get("/wfs?service=WFS&version=1.1.0&request=GetCapabilities")
        .await
        .body;
    assert_eq!(a, b);
}

#[actix_web::test]
async fn wfs11_accept_versions_sections_update_sequence() {
    let srv = cite11_server().await;
    let xml = srv
        .get("/wfs?service=WFS&request=GetCapabilities&AcceptVersions=1.1.0,1.0.0")
        .await
        .xml();
    assert_eq!(xml.string("/*/@version"), "1.1.0");
    let resp = srv
        .get("/wfs?service=WFS&version=1.1.0&request=GetCapabilities&AcceptVersions=2006.10.25")
        .await;
    let xml = resp.assert_exception("VersionNegotiationFailed");
    xml.assert("/ows:ExceptionReport");
    // sections
    let xml = srv
        .get(
            "/wfs?service=WFS&version=1.1.0&request=GetCapabilities&sections=ServiceIdentification",
        )
        .await
        .xml();
    xml.assert("/wfs:WFS_Capabilities/ows:ServiceIdentification");
    xml.assert_not("/wfs:WFS_Capabilities/wfs:FeatureTypeList");
    xml.assert_valid(WFS11_XSD);
    // updateSequence
    let seq = srv
        .get("/wfs?service=WFS&version=1.1.0&request=GetCapabilities")
        .await
        .xml()
        .string("/*/@updateSequence");
    assert!(!seq.is_empty());
    let resp = srv
        .get(&format!(
            "/wfs?service=WFS&version=1.1.0&request=GetCapabilities&updateSequence={}9",
            seq
        ))
        .await;
    resp.assert_exception("InvalidUpdateSequence");
}

#[actix_web::test]
async fn wfs11_exception_report() {
    let srv = cite11_server().await;
    let resp = srv.get("/wfs?version=1.1.0&request=GetCapabilities").await;
    let xml = resp.assert_exception("MissingParameterValue");
    xml.assert("/ows:ExceptionReport[@version='1.0.0']");
    xml.assert("count(//ows:Exception) = 1");
    assert_eq!(
        xml.string("//ows:Exception/@locator").to_lowercase(),
        "service"
    );
    assert!(!xml.string("//ows:Exception/ows:ExceptionText").is_empty());
    // owsExceptionReport.xsd and owsAll.xsd include each other; libxml2 needs the latter
    xml.assert_valid("http://schemas.opengis.net/ows/1.0.0/owsAll.xsd");
    let resp = srv.get("/wfs?service=WFS&version=1.1.0").await;
    let xml = resp.assert_exception("MissingParameterValue");
    assert_eq!(
        xml.string("//ows:Exception/@locator").to_lowercase(),
        "request"
    );
}

// ---------------------------------------------------------------- WFS 2.0

#[actix_web::test]
async fn wfs20_capabilities() {
    let srv = ne_server().await;
    let resp = srv
        .get("/wfs?service=WFS&version=2.0.0&request=GetCapabilities")
        .await;
    resp.assert_ok();
    let xml = resp.xml();
    xml.assert("/wfs2:WFS_Capabilities[@version='2.0.0']");
    xml.assert_valid(WFS20_XSD);
    // TopLevelElementsPattern
    for e in [
        "ows11:ServiceIdentification",
        "ows11:ServiceProvider",
        "ows11:OperationsMetadata",
        "wfs2:FeatureTypeList",
        "fes:Filter_Capabilities",
    ] {
        xml.assert(&format!("/wfs2:WFS_Capabilities/{e}"));
    }
    xml.assert("//ows11:ServiceIdentification/ows11:ServiceType = 'WFS'");
    xml.assert("//ows11:ServiceIdentification/ows11:ServiceTypeVersion = '2.0.0'");
    xml.assert("//ows11:ServiceIdentification/ows11:ServiceTypeVersion = '2.0.2'");
    // operations (read-only), hrefs without query string
    for op in [
        "GetCapabilities",
        "DescribeFeatureType",
        "ListStoredQueries",
        "DescribeStoredQueries",
        "GetFeature",
        "GetPropertyValue",
        "CreateStoredQuery",
        "DropStoredQuery",
    ] {
        let href = xml.string(&format!("//ows11:Operation[@name='{op}']//ows11:Get/@xlink:href | //ows11:Operation[@name='{op}']//ows11:Post/@xlink:href"));
        assert_eq!(href, "http://localhost:8080/wfs", "{op}");
    }
    for op in ["Transaction", "LockFeature", "GetFeatureWithLock"] {
        xml.assert_not(&format!("//ows11:Operation[@name='{op}']"));
    }
    xml.assert("//ows11:Operation[@name='CreateStoredQuery']/ows11:Parameter[@name='language']//ows11:Value='urn:ogc:def:queryLanguage:OGC-WFS::WFSQueryExpression'");
    xml.assert("//ows11:OperationsMetadata/ows11:Parameter[@name='resolve']//ows11:Value='local'");
    xml.assert("//ows11:OperationsMetadata/ows11:Parameter[@name='resolve']//ows11:Value='remote'");
    // conformance constraints: everything except write classes
    let constraint = |name: &str| {
        format!("//ows11:OperationsMetadata/ows11:Constraint[@name='{name}']/ows11:DefaultValue")
    };
    for c in [
        "ImplementsBasicWFS",
        "KVPEncoding",
        "XMLEncoding",
        "SOAPEncoding",
        "ImplementsInheritance",
        "ImplementsRemoteResolve",
        "ImplementsResultPaging",
        "ImplementsStandardJoins",
        "ImplementsSpatialJoins",
        "ImplementsTemporalJoins",
        "ManageStoredQueries",
    ] {
        assert_eq!(xml.string(&constraint(c)), "TRUE", "{c}");
        xml.assert(&format!("//ows11:Constraint[@name='{c}']/ows11:NoValues"));
    }
    // Feature versions conformance requires the Transactional WFS class (ISO 19142 Table 1);
    // read-only version navigation is announced by the FES ImplementsVersionNav constraint.
    for c in [
        "ImplementsTransactionalWFS",
        "ImplementsLockingWFS",
        "ImplementsFeatureVersioning",
    ] {
        assert_eq!(xml.string(&constraint(c)), "FALSE", "{c}");
    }
    assert_eq!(xml.string(&constraint("CountDefault")), "1000");
    xml.assert("//ows11:Constraint[@name='QueryExpressions']//ows11:Value='wfs:Query'");
    xml.assert("//ows11:Constraint[@name='QueryExpressions']//ows11:Value='wfs:StoredQuery'");
    // feature types
    for name in ["ne:populated_places", "ne:lakes", "ne:rivers"] {
        let ft = format!("//wfs2:FeatureType[wfs2:Name='{name}']");
        xml.assert(&ft);
        assert_eq!(
            xml.string(&format!("{ft}/wfs2:DefaultCRS")),
            "urn:ogc:def:crs:EPSG::4326"
        );
        xml.assert(&format!("{ft}/ows11:WGS84BoundingBox"));
        xml.assert(&format!("{ft}/wfs2:Title"));
    }
    xml.assert(
        "//wfs2:FeatureType[wfs2:Name='ne:lakes']/wfs2:OtherCRS='urn:ogc:def:crs:EPSG::3857'",
    );
    // FES conformance: all classes except version navigation needs no history... all TRUE
    let fes =
        |name: &str| format!("//fes:Conformance/fes:Constraint[@name='{name}']/ows11:DefaultValue");
    for c in [
        "ImplementsQuery",
        "ImplementsAdHocQuery",
        "ImplementsFunctions",
        "ImplementsResourceId",
        "ImplementsMinStandardFilter",
        "ImplementsStandardFilter",
        "ImplementsMinSpatialFilter",
        "ImplementsSpatialFilter",
        "ImplementsMinTemporalFilter",
        "ImplementsTemporalFilter",
        "ImplementsVersionNav",
        "ImplementsSorting",
        "ImplementsExtendedOperators",
        "ImplementsMinimumXPath",
        "ImplementsSchemaElementFunc",
    ] {
        assert_eq!(xml.string(&fes(c)), "TRUE", "{c}");
    }
    for op in [
        "BBOX",
        "Equals",
        "Disjoint",
        "Intersects",
        "Touches",
        "Crosses",
        "Within",
        "Contains",
        "Overlaps",
        "Beyond",
        "DWithin",
    ] {
        xml.assert(&format!(
            "//fes:Spatial_Capabilities/fes:SpatialOperators/fes:SpatialOperator[@name='{op}']"
        ));
    }
    // AnyInteracts is supported but not part of the FES 2.0 TemporalOperatorNameType enumeration
    for op in [
        "After",
        "Before",
        "Begins",
        "BegunBy",
        "TContains",
        "During",
        "TEquals",
        "TOverlaps",
        "Meets",
        "OverlappedBy",
        "MetBy",
        "Ends",
        "EndedBy",
    ] {
        xml.assert(&format!(
            "//fes:Temporal_Capabilities/fes:TemporalOperators/fes:TemporalOperator[@name='{op}']"
        ));
    }
    for op in [
        "PropertyIsEqualTo",
        "PropertyIsNotEqualTo",
        "PropertyIsLessThan",
        "PropertyIsGreaterThan",
        "PropertyIsLessThanOrEqualTo",
        "PropertyIsGreaterThanOrEqualTo",
        "PropertyIsLike",
        "PropertyIsNull",
        "PropertyIsNil",
        "PropertyIsBetween",
    ] {
        xml.assert(&format!(
            "//fes:ComparisonOperators/fes:ComparisonOperator[@name='{op}']"
        ));
    }
    xml.assert("//fes:Id_Capabilities/fes:ResourceIdentifier[@name='fes:ResourceId']");
    xml.assert("//fes:Functions/fes:Function[@name='strToUpperCase']/fes:Returns");
    xml.assert("//fes:Extended_Capabilities/fes:AdditionalOperators/fes:Operator");
}

#[actix_web::test]
async fn wfs20_version_negotiation() {
    let srv = ne_server().await;
    // ets-wfs20 getCapabilities_acceptVersions: first supported version of the list
    let xml = srv
        .get("/wfs?service=WFS&request=GetCapabilities&acceptversions=10.0.0,2.0.0,1.1.0")
        .await
        .xml();
    assert_eq!(xml.string("/*/@version"), "2.0.0");
    let xml = srv
        .post_wfs(r#"<wfs:GetCapabilities service="WFS" xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:ows="http://www.opengis.net/ows/1.1"><ows:AcceptVersions><ows:Version>10.0.0</ows:Version><ows:Version>2.0.0</ows:Version><ows:Version>1.1.0</ows:Version></ows:AcceptVersions></wfs:GetCapabilities>"#)
        .await
        .xml();
    assert_eq!(xml.string("/*/@version"), "2.0.0");
    // default is 2.0.2 (DGIWG)
    let xml = srv
        .get("/wfs?service=WFS&request=GetCapabilities")
        .await
        .xml();
    xml.assert("/wfs2:WFS_Capabilities[@version='2.0.2']");
    xml.assert_valid(WFS20_XSD);
}

#[actix_web::test]
async fn wfs20_missing_service_parameter() {
    // default: SERVICE defaults to WFS on /wfs (GeoServer non-CITE mode)
    let srv = ne_server().await;
    srv.get("/wfs?request=GetCapabilities").await.assert_ok();
    // strict CITE mode
    let srv = synth_server_with("cite_compliant = true").await;
    let resp = srv.get("/wfs?request=GetCapabilities").await;
    assert_eq!(resp.status, 400);
    let xml = resp.assert_exception("MissingParameterValue");
    xml.assert("/ows11:ExceptionReport[@version='2.0.0']");
    assert!(!xml.string("//ows11:Exception/@locator").is_empty());
    assert!(!xml
        .string("//ows11:Exception/ows11:ExceptionText")
        .is_empty());
    xml.assert_valid("http://schemas.opengis.net/ows/1.1.0/owsAll.xsd");
}

#[actix_web::test]
async fn wfs20_exception_status_codes() {
    let srv = ne_server().await;
    let resp = srv
        .get("/wfs?service=WFS&version=2.0.0&request=Transaction")
        .await;
    assert_eq!(resp.status, 501);
    resp.assert_exception("OperationNotSupported");
    let resp = srv
        .get("/wfs?service=WFS&version=2.0.0&request=Bogus")
        .await;
    assert_eq!(resp.status, 501);
    resp.assert_exception("OperationNotSupported");
    let resp = srv
        .get("/wfs?service=WMS&version=2.0.0&request=GetCapabilities")
        .await;
    assert_eq!(resp.status, 400);
    resp.assert_exception("InvalidParameterValue");
}

#[actix_web::test]
async fn post_dispatch_ignores_query_parameters() {
    // ets-wfs20 sends every POST to {href}?request=GetCapabilities
    let srv = ne_server().await;
    let resp = srv
        .post(
            "/wfs?request=GetCapabilities",
            r#"<wfs:DescribeStoredQueries service="WFS" version="2.0.0" xmlns:wfs="http://www.opengis.net/wfs/2.0"/>"#,
        )
        .await;
    resp.assert_ok();
    resp.xml().assert("/wfs2:DescribeStoredQueriesResponse");
}

#[actix_web::test]
async fn malformed_post_body() {
    let srv = ne_server().await;
    let resp = srv.post_wfs("<wfs:GetFeature").await;
    assert_eq!(resp.status, 400);
    resp.assert_exception("OperationParsingFailed");
}
