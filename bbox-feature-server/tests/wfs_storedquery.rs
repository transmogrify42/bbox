//! Stored queries and GetPropertyValue (WFS 2.0).
//! Assertions follow ets-wfs20 simple.* / querymgmt.* / GetPropertyValueTests and DGIWG.

mod common;
use common::*;

const WFS20: &str = "http://schemas.opengis.net/wfs/2.0/wfs.xsd";
const URN: &str = "urn:ogc:def:query:OGC-WFS::GetFeatureById";
const HTTP: &str = "http://www.opengis.net/def/query/OGC-WFS/0/GetFeatureById";

#[actix_web::test]
async fn list_stored_queries() {
    let srv = ne_server().await;
    for resp in [
        srv.get("/wfs?service=WFS&version=2.0.0&request=ListStoredQueries").await,
        srv.post_wfs(r#"<wfs:ListStoredQueries version="2.0.0" service="WFS" handle="h1" xmlns:wfs="http://www.opengis.net/wfs/2.0"/>"#).await,
    ] {
        resp.assert_ok();
        let xml = resp.xml();
        xml.assert_valid(WFS20);
        xml.assert(&format!("//wfs2:StoredQuery[@id='{HTTP}' or @id='{URN}']"));
        xml.assert(&format!("//wfs2:StoredQuery[@id='{URN}']/wfs2:Title"));
    }
}

#[actix_web::test]
async fn describe_stored_queries() {
    let srv = ne_server().await;
    let resp = srv
        .get("/wfs?service=WFS&version=2.0.0&request=DescribeStoredQueries")
        .await;
    resp.assert_ok();
    let xml = resp.xml();
    xml.assert_valid(WFS20);
    assert!(xml.count("//wfs2:StoredQueryDescription") > 0);
    // DGIWG: every description has title, parameter name/type/title, QueryExpressionText with returnFeatureTypes and language
    for path in [
        "//wfs2:StoredQueryDescription[not(wfs2:Title)]",
        "//wfs2:Parameter[not(@name) or not(@type) or not(wfs2:Title)]",
        "//wfs2:QueryExpressionText[not(@returnFeatureTypes) or not(@language)]",
    ] {
        xml.assert_count(path, 0);
    }
    for id in [URN, HTTP] {
        let xml = srv
            .get(&format!(
                "/wfs?service=WFS&version=2.0.0&request=DescribeStoredQueries&storedquery_id={id}"
            ))
            .await
            .xml();
        xml.assert(&format!("//wfs2:StoredQueryDescription[@id='{id}']"));
        xml.assert_count("//wfs2:StoredQueryDescription", 1);
    }
    let xml = srv
        .post_wfs(&format!(r#"<wfs:DescribeStoredQueries version="2.0.0" service="WFS" xmlns:wfs="http://www.opengis.net/wfs/2.0"><wfs:StoredQueryId>{URN}</wfs:StoredQueryId></wfs:DescribeStoredQueries>"#))
        .await
        .xml();
    xml.assert(&format!("//wfs2:StoredQueryDescription[@id='{URN}']"));
    let resp = srv.get("/wfs?service=WFS&version=2.0.0&request=DescribeStoredQueries&storedquery_id=urn:unknown").await;
    resp.assert_exception("InvalidParameterValue");
}

#[actix_web::test]
async fn get_feature_by_id() {
    let srv = ne_server().await;
    for id in [URN, HTTP] {
        let resp = srv
            .get(&format!(
                "/wfs?service=WFS&version=2.0.0&request=GetFeature&storedquery_id={id}&id=lakes.5"
            ))
            .await;
        resp.assert_ok();
        let xml = resp.xml();
        // bare feature as document root
        xml.assert("/*[local-name()='lakes' and @gml32:id='lakes.5']");
        xml.assert_not("//wfs2:FeatureCollection");
    }
    let resp = srv
        .post_wfs(&format!(r#"<wfs:GetFeature version="2.0.2" service="WFS" xmlns:wfs="http://www.opengis.net/wfs/2.0"><wfs:StoredQuery id="{HTTP}"><wfs:Parameter name="id">rivers.7</wfs:Parameter></wfs:StoredQuery></wfs:GetFeature>"#))
        .await;
    resp.assert_ok();
    resp.xml().assert("/*[@gml32:id='rivers.7']");
    // unknown id -> 404 NotFound
    let resp = srv
        .get(&format!(
            "/wfs?service=WFS&version=2.0.0&request=GetFeature&storedquery_id={URN}&id=uuid-1234"
        ))
        .await;
    assert_eq!(resp.status, 404);
    resp.assert_exception("NotFound");
    // unknown stored query
    let resp = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&storedquery_id=http://docbook.org/ns/docbook").await;
    let xml = resp.assert_exception("InvalidParameterValue");
    assert!(!xml.string("//ows11:Exception/@locator").is_empty());
}

const CREATE_BY_TYPENAME: &str = r#"<CreateStoredQuery version="2.0.0" service="WFS" xmlns="http://www.opengis.net/wfs/2.0">
 <StoredQueryDefinition id="urn:example:wfs2-query:GetFeatureByTypeName" xmlns:xsd="http://www.w3.org/2001/XMLSchema">
  <Title>GetFeatureByTypeName</Title><Abstract>Returns features of the given type</Abstract>
  <Parameter name="typeName" type="xsd:QName"><Abstract>Qualified name of feature type</Abstract></Parameter>
  <QueryExpressionText returnFeatureTypes="" language="urn:ogc:def:queryLanguage:OGC-WFS::WFSQueryExpression" isPrivate="false">
    <Query typeNames="${typeName}"/>
  </QueryExpressionText>
 </StoredQueryDefinition>
</CreateStoredQuery>"#;

#[actix_web::test]
async fn create_invoke_drop_stored_query() {
    let srv = ne_server().await;
    let resp = srv.post_wfs(CREATE_BY_TYPENAME).await;
    resp.assert_ok();
    resp.xml().assert("/wfs2:CreateStoredQueryResponse");
    resp.xml().assert_valid(WFS20);
    // listed and described
    srv.get("/wfs?service=WFS&version=2.0.0&request=ListStoredQueries")
        .await
        .xml()
        .assert("//wfs2:StoredQuery[@id='urn:example:wfs2-query:GetFeatureByTypeName']");
    // invoke via KVP without NAMESPACES binding
    let xml = srv
        .get("/wfs?service=WFS&version=2.0.0&request=GetFeature&storedquery_id=urn:example:wfs2-query:GetFeatureByTypeName&typeName=ne:rivers&count=3")
        .await
        .xml();
    xml.assert_count("//wfs2:member/*[local-name()='rivers']", 3);
    // duplicate
    let resp = srv.post_wfs(CREATE_BY_TYPENAME).await;
    let xml = resp.assert_exception("DuplicateStoredQueryIdValue");
    assert!(xml
        .string("//ows11:Exception/@locator")
        .contains("urn:example:wfs2-query:GetFeatureByTypeName"));
    // drop
    let resp = srv
        .post_wfs(r#"<DropStoredQuery version="2.0.0" service="WFS" xmlns="http://www.opengis.net/wfs/2.0" id="urn:example:wfs2-query:GetFeatureByTypeName"/>"#)
        .await;
    resp.assert_ok();
    resp.xml().assert("/wfs2:DropStoredQueryResponse");
    let resp = srv
        .get("/wfs?service=WFS&version=2.0.0&request=GetFeature&storedquery_id=urn:example:wfs2-query:GetFeatureByTypeName&typeName=ne:rivers")
        .await;
    let xml = resp.assert_exception("InvalidParameterValue");
    assert!(xml
        .string("//ows11:Exception/@locator")
        .to_lowercase()
        .contains("id"));
    // drop unknown
    let resp = srv
        .post_wfs(r#"<DropStoredQuery version="2.0.0" service="WFS" xmlns="http://www.opengis.net/wfs/2.0" id="urn:uuid:0000"/>"#)
        .await;
    let xml = resp.assert_exception("InvalidParameterValue");
    assert!(xml
        .string("//ows11:Exception/@locator")
        .to_lowercase()
        .contains("id"));
    // built-in queries can not be dropped
    let resp = srv
        .post_wfs(&format!(r#"<DropStoredQuery version="2.0.0" service="WFS" xmlns="http://www.opengis.net/wfs/2.0" id="{URN}"/>"#))
        .await;
    resp.assert_exception("InvalidParameterValue");
}

#[actix_web::test]
async fn create_stored_query_with_filter_template() {
    let srv = ne_server().await;
    let resp = srv
        .post_wfs(r#"<CreateStoredQuery version="2.0.0" service="WFS" xmlns="http://www.opengis.net/wfs/2.0" xmlns:fes="http://www.opengis.net/fes/2.0">
 <StoredQueryDefinition id="urn:example:wfs2-query:GetFeatureByName">
  <Title>GetFeatureByName</Title>
  <Parameter name="name" type="xsd:string"/>
  <QueryExpressionText returnFeatureTypes="ns1:populated_places" language="urn:ogc:def:queryLanguage:OGC-WFS::WFSQueryExpression" isPrivate="false" xmlns:ns1="http://www.naturalearthdata.com">
    <Query typeNames="ns1:populated_places">
      <fes:Filter><fes:PropertyIsLike wildCard="*" singleChar="?" escapeChar="\"><fes:ValueReference>ns1:NAME</fes:ValueReference><fes:Literal>*${name}*</fes:Literal></fes:PropertyIsLike></fes:Filter>
    </Query>
  </QueryExpressionText>
 </StoredQueryDefinition>
</CreateStoredQuery>"#)
        .await;
    resp.assert_ok();
    let xml = srv
        .get("/wfs?service=WFS&version=2.0.0&request=GetFeature&storedquery_id=urn:example:wfs2-query:GetFeatureByName&name=Bern")
        .await
        .xml();
    xml.assert("//*[local-name()='NAME']='Bern'");
    // unsupported language
    let resp = srv
        .post_wfs(r#"<CreateStoredQuery version="2.0.0" service="WFS" xmlns="http://www.opengis.net/wfs/2.0"><StoredQueryDefinition id="urn:example:wfs2-query:InvalidLang"><Title>x</Title><QueryExpressionText returnFeatureTypes="" language="http://qry.example.org" isPrivate="false"><Query typeNames="ne:lakes"/></QueryExpressionText></StoredQueryDefinition></CreateStoredQuery>"#)
        .await;
    let xml = resp.assert_exception("InvalidParameterValue");
    assert!(xml
        .string("//ows11:Exception/@locator")
        .to_lowercase()
        .contains("language"));
}

#[actix_web::test]
async fn get_property_value() {
    let srv = ne_server().await;
    for resp in [
        srv.get("/wfs?service=WFS&version=2.0.0&request=GetPropertyValue&typeNames=ne:lakes&count=5&valueReference=ne:name").await,
        srv.post_wfs(r#"<wfs:GetPropertyValue version="2.0.0" service="WFS" count="5" valueReference="ne:name" xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:ne="http://www.naturalearthdata.com"><wfs:Query typeNames="ne:lakes"/></wfs:GetPropertyValue>"#).await,
    ] {
        resp.assert_ok();
        let xml = resp.xml();
        xml.assert("/wfs2:ValueCollection[@numberMatched='1355' and @numberReturned='5' and @timeStamp]");
        xml.assert_count("//wfs2:member", 5);
        xml.assert_valid(WFS20);
    }
    // gml:id values
    let xml = srv.get("/wfs?service=WFS&version=2.0.0&request=GetPropertyValue&typeNames=ne:lakes&count=3&valueReference=@gml:id").await.xml();
    let ids = xml.strings("//wfs2:member");
    assert_eq!(ids.len(), 3);
    assert!(ids.iter().all(|id| id.starts_with("lakes.")), "{ids:?}");
    // geometry values are GML elements
    let xml = srv.get("/wfs?service=WFS&version=2.0.0&request=GetPropertyValue&typeNames=ne:lakes&count=1&valueReference=geom").await.xml();
    xml.assert("//wfs2:member/gml32:MultiSurface");
    // empty value reference
    let resp = srv.get("/wfs?service=WFS&version=2.0.0&request=GetPropertyValue&typeNames=ne:lakes&valueReference=").await;
    resp.assert_exception("InvalidParameterValue");
    // unknown property
    let resp = srv.get("/wfs?service=WFS&version=2.0.0&request=GetPropertyValue&typeNames=ne:lakes&valueReference=ne:unknown").await;
    resp.assert_exception("InvalidParameterValue");
}
