//! DescribeFeatureType (WFS 1.0, 1.1, 2.0). Assertions follow ets-wfs10/11/20.

mod common;
use common::*;

fn assert_schema(resp: &Response) -> Xml {
    resp.assert_ok();
    let xml = resp.xml();
    xml.assert("/xs:schema");
    xml.assert_valid("http://www.w3.org/2001/XMLSchema.xsd");
    xml
}

// ---------------------------------------------------------------- WFS 1.0

#[actix_web::test]
async fn wfs10_single_type() {
    let srv = cite10_server().await;
    for req in [
        "/wfs?service=WFS&version=1.0.0&request=DescribeFeatureType&typeName=cgf:Points",
        "/wfs?service=WFS&version=1.0.0&request=DescribeFeatureType&outputFormat=XMLSCHEMA&typename=cgf:Points",
    ] {
        let xml = assert_schema(&srv.get(req).await);
        xml.assert("/xs:schema/@targetNamespace = 'http://www.opengis.net/cite/geometry'");
        xml.assert("/xs:schema/@elementFormDefault = 'qualified'");
        xml.assert("//xs:import[@namespace='http://www.opengis.net/gml' and @schemaLocation='http://schemas.opengis.net/gml/2.1.2/feature.xsd']");
        xml.assert("/xs:schema/xs:element[@name='Points' and @substitutionGroup]");
    }
    let resp = srv
        .post_wfs(r#"<wfs:DescribeFeatureType service="WFS" version="1.0.0" xmlns:wfs="http://www.opengis.net/wfs" xmlns:cgf="http://www.opengis.net/cite/geometry"><wfs:TypeName>cgf:Points</wfs:TypeName><wfs:TypeName>cgf:Lines</wfs:TypeName></wfs:DescribeFeatureType>"#)
        .await;
    let xml = assert_schema(&resp);
    xml.assert("/xs:schema/xs:element[@name='Points']");
    xml.assert("/xs:schema/xs:element[@name='Lines']");
}

#[actix_web::test]
async fn wfs10_schema_content_matches_application_schema() {
    let srv = cite10_server().await;
    let xml = assert_schema(
        &srv.get("/wfs?service=WFS&version=1.0.0&request=DescribeFeatureType&typeName=cdf:Other")
            .await,
    );
    // properties of cdf:Other in schema order
    let props = xml.strings("//xs:complexType[@name='OtherType']//xs:sequence/xs:element/@name | //xs:complexType[@name='OtherType']//xs:sequence/xs:element/@ref");
    assert_eq!(
        props,
        vec![
            "gml:pointProperty",
            "string1",
            "string2",
            "integers",
            "dates"
        ]
    );
}

#[actix_web::test]
async fn wfs10_errors() {
    let srv = cite10_server().await;
    for req in [
        "/wfs?service=WFS&version=1.0.0&request=DescribeFeatureType&outputFormat=DUMMYFORMAT",
        "/wfs?service=WFS&version=1.0.0&request=DescribeFeatureType&typeName=cgf:DummyFeature",
        "/wfs?service=WFS&version=1.0.0&request=DescribeFeatureType&typeName=cgf:Points,cgf:DummyFeature",
    ] {
        let resp = srv.get(req).await;
        assert_eq!(resp.status, 200);
        resp.xml().assert("/ogc:ServiceExceptionReport");
    }
}

#[actix_web::test]
async fn wfs10_multiple_namespaces() {
    let srv = cite10_server().await;
    let xml = assert_schema(&srv.get("/wfs?service=WFS&version=1.0.0&request=DescribeFeatureType&typeName=cgf:Points,cdf:Other").await);
    // one schema importing each namespace by absolute URL
    for ns in [
        "http://www.opengis.net/cite/geometry",
        "http://www.opengis.net/cite/data",
    ] {
        let loc = xml.string(&format!("//xs:import[@namespace='{ns}']/@schemaLocation"));
        assert!(loc.starts_with("http://localhost:8080/wfs?"), "{loc}");
        assert!(loc.contains("DescribeFeatureType"), "{loc}");
    }
    // all types: every namespace imported
    let xml = assert_schema(
        &srv.get("/wfs?service=WFS&version=1.0.0&request=DescribeFeatureType")
            .await,
    );
    for ns in [
        "http://www.opengis.net/cite/geometry",
        "http://www.opengis.net/cite/data",
        "http://www.opengis.net/cite/complex",
    ] {
        xml.assert(&format!("//xs:import[@namespace='{ns}']"));
    }
}

// ---------------------------------------------------------------- WFS 1.1

#[actix_web::test]
async fn wfs11_describe() {
    let srv = cite11_server().await;
    let resp = srv.get("/wfs?service=WFS&version=1.1.0&request=DescribeFeatureType&typeName=sf:PrimitiveGeoFeature").await;
    let xml = assert_schema(&resp);
    assert!(
        resp.content_type.starts_with("text/xml; subtype=gml/3.1.1"),
        "{}",
        resp.content_type
    );
    xml.assert("/xs:schema/@targetNamespace = 'http://cite.opengeospatial.org/gmlsf'");
    xml.assert("//xs:import[@namespace='http://www.opengis.net/gml' and starts-with(@schemaLocation, 'http://schemas.opengis.net/gml/3.1.1/')]");
    xml.assert("/xs:schema/xs:element[@name='PrimitiveGeoFeature']");
    // non-ASCII type name, percent encoded in KVP
    let xml = assert_schema(&srv.get("/wfs?service=WFS&version=1.1.0&request=DescribeFeatureType&typeName=sf:Entit%C3%A9G%C3%A9n%C3%A9rique").await);
    xml.assert("/xs:schema/xs:element[@name='EntitéGénérique']");
    // no type names: all types of the namespace
    let xml = assert_schema(
        &srv.get("/wfs?service=WFS&version=1.1.0&request=DescribeFeatureType")
            .await,
    );
    xml.assert("/xs:schema/xs:element[@name='AggregateGeoFeature']");
    let resp = srv
        .get("/wfs?service=WFS&version=1.1.0&request=DescribeFeatureType&typeName=sf:Unknown")
        .await;
    let xml = resp.assert_exception("InvalidParameterValue");
    assert_eq!(
        xml.string("//ows:Exception/@locator").to_lowercase(),
        "typename"
    );
}

// ---------------------------------------------------------------- WFS 2.0

#[actix_web::test]
async fn wfs20_describe_generated_schema() {
    let srv = ne_server().await;
    for req in [
        "/wfs?service=WFS&version=2.0.0&request=DescribeFeatureType",
        "/wfs?service=WFS&version=2.0.0&request=DescribeFeatureType&typeNames=ne:lakes,ne:rivers,ne:populated_places",
    ] {
        let xml = assert_schema(&srv.get(req).await);
        xml.assert("/xs:schema/@targetNamespace = 'http://www.naturalearthdata.com'");
        xml.assert("//xs:import[@namespace='http://www.opengis.net/gml/3.2' and @schemaLocation='http://schemas.opengis.net/gml/3.2.1/gml.xsd']");
        for t in ["lakes", "rivers", "populated_places"] {
            xml.assert(&format!("/xs:schema/xs:element[@name='{t}' and @substitutionGroup='gml:AbstractFeature']"));
        }
        xml.assert("//xs:complexType[@name='lakesType']/xs:complexContent/xs:extension[@base='gml:AbstractFeatureType']");
        xml.assert("//xs:complexType[@name='lakesType']//xs:element[@name='geom' and @type='gml:MultiSurfacePropertyType']");
        xml.assert("//xs:complexType[@name='populated_placesType']//xs:element[@name='geom' and @type='gml:PointPropertyType']");
        xml.assert("//xs:complexType[@name='lakesType']//xs:element[@name='scalerank' and @type='xs:integer' and @minOccurs='0' and @nillable='true']");
        // column `name` is mapped to the inherited gml:name (GeoServer parity)
        xml.assert_not("//xs:complexType[@name='lakesType']//xs:element[@name='name']");
    }
    // KVP with NAMESPACES and unprefixed type names
    let xml = assert_schema(&srv.get("/wfs?service=WFS&version=2.0.0&request=DescribeFeatureType&typename=x:lakes&namespaces=xmlns(xml,http://www.w3.org/XML/1998/namespace),xmlns(wfs,http://www.opengis.net/wfs/2.0),xmlns(x,http://www.naturalearthdata.com)").await);
    xml.assert("/xs:schema/xs:element[@name='lakes']");
    let xml = assert_schema(
        &srv.get("/wfs?service=WFS&version=2.0.0&request=DescribeFeatureType&typeNames=lakes")
            .await,
    );
    xml.assert("/xs:schema/xs:element[@name='lakes']");
    // POST
    let xml = assert_schema(&srv.post_wfs(r#"<wfs:DescribeFeatureType service="WFS" version="2.0.0" xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:n="http://www.naturalearthdata.com"><wfs:TypeName>n:rivers</wfs:TypeName></wfs:DescribeFeatureType>"#).await);
    xml.assert("/xs:schema/xs:element[@name='rivers']");
    xml.assert_not("/xs:schema/xs:element[@name='lakes']");
}

#[actix_web::test]
async fn wfs20_describe_unknown_type() {
    let srv = ne_server().await;
    let resp = srv.get("/wfs?service=WFS&version=2.0.0&request=DescribeFeatureType&typename=ns42:Unknown1.Type&namespaces=xmlns(xml,http://www.w3.org/XML/1998/namespace),xmlns(wfs,http://www.opengis.net/wfs/2.0),xmlns(ns42,http://example.org)").await;
    assert_eq!(resp.status, 400);
    let xml = resp.assert_exception("InvalidParameterValue");
    assert!(!xml.string("//ows11:Exception/@locator").is_empty());
    let resp = srv
        .post_wfs(r#"<wfs:DescribeFeatureType service="WFS" version="2.0.0" xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:ns42="http://example.org"><wfs:TypeName>ns42:Unknown1.Type</wfs:TypeName></wfs:DescribeFeatureType>"#)
        .await;
    assert_eq!(resp.status, 400);
    resp.assert_exception("InvalidParameterValue");
}

#[actix_web::test]
async fn wfs20_schema_compiles_with_gml32() {
    // The returned schema must be usable to validate GetFeature responses (ets-gml32 compiles it)
    let srv = ne_server().await;
    let dft = srv
        .get("/wfs?service=WFS&version=2.0.0&request=DescribeFeatureType")
        .await
        .body;
    let fc = srv
        .get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=ne:lakes&count=3")
        .await;
    fc.assert_ok();
    fc.xml()
        .assert_valid_with_schema_doc("http://schemas.opengis.net/wfs/2.0/wfs.xsd", &dft);
}
