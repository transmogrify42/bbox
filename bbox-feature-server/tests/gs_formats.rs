//! GeoServer WFS test suite parity: output formats (GeoJSON, JSONP, CSV, SHAPE-ZIP, hits,
//! JSON exceptions) and wfs-core behaviour, re-implemented against the GeoServer test catalog
//! (tests/common/geoserver.rs).
//!
//! GeoServer infers `service=WFS` from the `/wfs` path; bbox requires the SERVICE parameter
//! (ets-wfs11 GetFeature-tc4), so the ported KVP requests add it.
//!
//! Not ported (reason):
//! - GeoJSONTest.testFeatureBoundingDisabledCollection: featureBounding service config switch.
//! - GeoJSONTest.testGetFeatureLine3D, testGetFeatureWhereLayerHasDecimalPointsSet,
//!   testGeometryAndGeometryNameConsistency, testNanInfinite, wfs2_x testGetFeatureAxisSwap,
//!   testGetFeatureNoAxisSwap, testIAULayer (1.x and 2.0): extra datasets (Line3D, PointReduced,
//!   MultiGeometriesWithNull, NanInfinite, PointLatLon/PointLonLat, iau:MarsPoi) and per-layer
//!   numDecimals / IAU CRS configuration.
//! - wfs2_x GeoJSONTest.testGetSkipCounting: skipNumberMatched layer flag.
//! - GeoJsonOutputFormatTest.testMeasuresEncoding, GML32MeasuresTest, GML3MeasuresTest,
//!   GML2MeasuresTest: gs:lineStringZm / gs:lineStringM datasets and encodeMeasures layer flag
//!   (Z output is covered by wfs_getfeature.rs three_d_coordinates).
//! - CSVOutputFormatTest.testEscapes, testComplexFeatureMultiValuedAttributes, testDates,
//!   testIAULayer: Java unit tests, app-schema, csvDateFormat setting, IAU dataset.
//! - ShapeZipTest.testRequestUrlWithProxyBase, testMultiType, testSplitSize, testMultiTypeDots,
//!   testGeometryInTheMiddle, testNullGeometries, testLongNames, testDots, testTemplate*Type,
//!   testESRIFormatFromDefaultValue, testDoNotIncludeWFSRequestDumpFile,
//!   testIncludeWFSRequestDumpFile, testPointZMShp, testMultiPointZMShp, testMultiLineStringZMShp,
//!   testMultiPolygonZMShp: proxy base / size / template / dump file configuration or extra
//!   datasets (AllTypes, All.Types.Dots, geommid, nullgeom, longnames, dots.in.name, ZM shapes).
//! - WFSExceptionTest, WFSWorkspaceQualifierTest, AbstractTransactionCurveTest,
//!   UpdateElementHandlerTest, BatchManagerTest, FeatureCollectionDelegationTest,
//!   GetFeatureBoundedTest, CatalogNamespaceSupportTest, WFSReprojectionUtilTest,
//!   JoinExtractingVisitorTest, RoundingUtilTest, response.CSVOutputFormatTest,
//!   WfsXmlWriterTest, RestrictionToXSDConstrainingFacetVisitorTest, WFSOutputFormatCallbackTest,
//!   WFSXStreamLoaderTest: Java unit tests of GeoServer internals, write operations or
//!   GeoServer configuration.

mod common;
use common::*;
use serde_json::Value;
use std::io::Read;

const PGF_ATTRS: usize = 12;

fn json(resp: &Response) -> Value {
    serde_json::from_str(&resp.body)
        .unwrap_or_else(|e| panic!("invalid JSON ({e}):\n{}", resp.body))
}

fn enc(s: &str) -> String {
    serde_urlencoded::to_string([("x", s)]).unwrap()[2..].to_string()
}

/// JSON inside a JSONP callback
fn jsonp(body: &str, callback: &str) -> Value {
    let body = body.trim();
    assert!(
        body.starts_with(&format!("{callback}(")),
        "{}",
        &body[..body.len().min(80)]
    );
    assert!(body.ends_with(')'), "JSONP must end with `)`");
    serde_json::from_str(&body[callback.len() + 1..body.len() - 1]).unwrap()
}

/// Split CSV text into records of fields (RFC 4180 quoting)
fn csv_records(text: &str, sep: char) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' {
            quoted = true;
        } else if c == sep {
            record.push(std::mem::take(&mut field));
        } else if c == '\n' || c == '\r' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            record.push(std::mem::take(&mut field));
            records.push(std::mem::take(&mut record));
        } else {
            field.push(c);
        }
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    records
}

fn zip_archive(bytes: &[u8]) -> zip::ZipArchive<std::io::Cursor<Vec<u8>>> {
    zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).expect("valid zip")
}

fn zip_names(bytes: &[u8]) -> Vec<String> {
    zip_archive(bytes)
        .file_names()
        .map(str::to_string)
        .collect()
}

fn zip_entry(bytes: &[u8], name: &str) -> Vec<u8> {
    let mut a = zip_archive(bytes);
    let mut f = a
        .by_name(name)
        .unwrap_or_else(|_| panic!("zip entry {name} missing"));
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).unwrap();
    buf
}

/// Number of records of a shapefile (from the .shx index)
fn shx_records(shx: &[u8]) -> usize {
    (shx.len() - 100) / 8
}

fn assert_json_ct(resp: &Response) {
    resp.assert_ok();
    assert!(
        resp.content_type.starts_with("application/json"),
        "content type {}",
        resp.content_type
    );
}

// ------------------------------------------------------------------ wfs1_x json.GeoJSONTest

/// GeoServer wfs1_x GeoJSONTest.testGet
#[actix_web::test]
async fn gs_geojson_test_get() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&request=GetFeature&version=1.0.0&typename=sf:PrimitiveGeoFeature&maxfeatures=1&outputformat=application/json").await;
    assert_json_ct(&r);
    let j = json(&r);
    assert_eq!(j["type"], "FeatureCollection");
    assert_eq!(j["features"][0]["geometry_name"], "surfaceProperty");
    let ts = j["timeStamp"].as_str().expect("timeStamp");
    let b = ts.as_bytes();
    assert!(
        ts.len() == 24 && b[4] == b'-' && b[10] == b'T' && b[19] == b'.' && ts.ends_with('Z'),
        "timeStamp format {ts}"
    );
}

/// GeoServer wfs1_x GeoJSONTest.testGetSimpleJson
#[actix_web::test]
async fn gs_geojson_test_get_simple_json() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&request=GetFeature&version=1.0.0&typename=sf:PrimitiveGeoFeature&maxfeatures=1&outputformat=json").await;
    assert_json_ct(&r);
    assert!(
        r.content_type.to_lowercase().contains("utf-8"),
        "charset in {}",
        r.content_type
    );
    let j = json(&r);
    assert_eq!(j["type"], "FeatureCollection");
    assert_eq!(j["features"][0]["geometry_name"], "surfaceProperty");
}

/// GeoServer wfs1_x GeoJSONTest.testGetJsonIdPolicyTrue
#[actix_web::test]
async fn gs_geojson_test_get_json_id_policy_true() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&request=GetFeature&version=1.0.0&typename=sf:PrimitiveGeoFeature&maxfeatures=1&outputformat=json&format_options=id_policy:true").await;
    assert_json_ct(&r);
    assert_eq!(json(&r)["features"][0]["id"], "PrimitiveGeoFeature.f001");
}

/// GeoServer wfs1_x GeoJSONTest.testGetJsonIdPolicyFalse
#[actix_web::test]
async fn gs_geojson_test_get_json_id_policy_false() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&request=GetFeature&version=1.0.0&typename=sf:PrimitiveGeoFeature&maxfeatures=1&outputformat=json&format_options=id_policy:false").await;
    assert_json_ct(&r);
    let j = json(&r);
    assert!(
        j["features"][0].get("id").is_none(),
        "feature must not have an id: {}",
        j["features"][0]
    );
}

/// GeoServer wfs1_x GeoJSONTest.testGetJsonIdPolicyAttribute
#[actix_web::test]
async fn gs_geojson_test_get_json_id_policy_attribute() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&request=GetFeature&version=1.0.0&typename=sf:PrimitiveGeoFeature&maxfeatures=1&outputformat=json&format_options=id_policy:name").await;
    assert_json_ct(&r);
    let f = &json(&r)["features"][0];
    assert_eq!(f["id"], "name-f001");
    assert!(
        f["properties"].get("name").is_none(),
        "id attribute removed from properties: {f}"
    );
}

/// GeoServer wfs1_x GeoJSONTest.testPost
#[actix_web::test]
async fn gs_geojson_test_post() {
    let srv = geoserver::server().await;
    let body = r#"<wfs:GetFeature service="WFS" outputFormat="application/json" version="1.0.0" xmlns:cdf="http://www.opengis.net/cite/data" xmlns:ogc="http://www.opengis.net/ogc" xmlns:wfs="http://www.opengis.net/wfs" xmlns:sf="http://cite.opengeospatial.org/gmlsf"><wfs:Query typeName="sf:PrimitiveGeoFeature"></wfs:Query></wfs:GetFeature>"#;
    let r = srv.post_wfs(body).await;
    assert_json_ct(&r);
    let j = json(&r);
    assert_eq!(j["type"], "FeatureCollection");
    assert_eq!(j["features"][0]["geometry_name"], "surfaceProperty");
}

/// GeoServer wfs1_x GeoJSONTest.testGeometryCollection
#[actix_web::test]
async fn gs_geojson_test_geometry_collection() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&request=GetFeature&version=1.0.0&typename=sf:AggregateGeoFeature&maxfeatures=3&outputformat=application/json").await;
    assert_json_ct(&r);
    let j = json(&r);
    let mc = &j["features"][1]["properties"]["multiCurveProperty"];
    assert_eq!(mc["type"], "MultiLineString");
    assert_eq!(mc["coordinates"][0][0][0].as_f64(), Some(55.174));
    assert_eq!(j["crs"]["type"], "name");
    let crs = j["crs"]["properties"]["name"].as_str().unwrap_or("");
    assert!(crs.ends_with("4326"), "crs {crs}");
}

/// GeoServer wfs1_x GeoJSONTest.testMixedCollection
#[actix_web::test]
async fn gs_geojson_test_mixed_collection() {
    let srv = geoserver::server().await;
    let body = r#"<wfs:GetFeature service="WFS" outputFormat="application/json" version="1.0.0" xmlns:ogc="http://www.opengis.net/ogc" xmlns:wfs="http://www.opengis.net/wfs" xmlns:sf="http://cite.opengeospatial.org/gmlsf"><wfs:Query typeName="sf:PrimitiveGeoFeature"/><wfs:Query typeName="sf:AggregateGeoFeature"/></wfs:GetFeature>"#;
    let r = srv.post_wfs(body).await;
    assert_json_ct(&r);
    let j = json(&r);
    let id = |i: usize| j["features"][i]["id"].as_str().unwrap_or("").to_string();
    assert!(id(1).starts_with("PrimitiveGeoFeature"), "{}", id(1));
    assert!(id(6).starts_with("AggregateGeoFeature"), "{}", id(6));
    assert_eq!(
        j["features"][6]["properties"]["multiCurveProperty"]["type"],
        "MultiLineString"
    );
}

/// GeoServer wfs1_x GeoJSONTest.testCallbackFunction
#[actix_web::test]
async fn gs_geojson_test_callback_function() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&request=GetFeature&version=1.0.0&typename=sf:PrimitiveGeoFeature&maxfeatures=1&outputformat=text/javascript&format_options=callback:myFunc").await;
    r.assert_ok();
    assert!(
        r.content_type.starts_with("text/javascript"),
        "{}",
        r.content_type
    );
    let j = jsonp(&r.body, "myFunc");
    assert_eq!(j["type"], "FeatureCollection");
    assert_eq!(j["features"][0]["geometry_name"], "surfaceProperty");
}

/// GeoServer wfs1_x GeoJSONTest.testGetFeatureCountNoFilter
#[actix_web::test]
async fn gs_geojson_test_get_feature_count_no_filter() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&request=GetFeature&version=1.0.0&typename=sf:PrimitiveGeoFeature&maxfeatures=10&outputformat=application/json").await;
    assert_json_ct(&r);
    assert_eq!(json(&r)["totalFeatures"], 5);
}

/// GeoServer wfs1_x GeoJSONTest.testGetFeatureCountFilter
#[actix_web::test]
async fn gs_geojson_test_get_feature_count_filter() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&request=GetFeature&version=1.0.0&typename=sf:PrimitiveGeoFeature&maxfeatures=10&outputformat=application/json&featureid=PrimitiveGeoFeature.f001").await;
    assert_json_ct(&r);
    assert_eq!(json(&r)["totalFeatures"], 1);
}

/// GeoServer wfs1_x GeoJSONTest.testGetFeatureCountMaxFeatures
#[actix_web::test]
async fn gs_geojson_test_get_feature_count_max_features() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&request=GetFeature&version=1.0.0&typename=sf:PrimitiveGeoFeature&maxfeatures=1&outputformat=application/json&featureid=PrimitiveGeoFeature.f001,PrimitiveGeoFeature.f002").await;
    assert_json_ct(&r);
    let j = json(&r);
    assert_eq!(j["totalFeatures"], 2);
    assert_eq!(j["features"].as_array().unwrap().len(), 1);
}

/// GeoServer wfs1_x GeoJSONTest.testGetFeatureCountMultipleFeatureTypes
#[actix_web::test]
async fn gs_geojson_test_get_feature_count_multiple_feature_types() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&request=GetFeature&version=1.0.0&typename=sf:PrimitiveGeoFeature,sf:AggregateGeoFeature&outputformat=application/json&featureid=PrimitiveGeoFeature.f001,PrimitiveGeoFeature.f002,AggregateGeoFeature.f009").await;
    assert_json_ct(&r);
    assert_eq!(json(&r)["totalFeatures"], 3);
}

/// GeoServer wfs1_x GeoJSONTest.testGetFeatureCountSpatialFilter
#[actix_web::test]
async fn gs_geojson_test_get_feature_count_spatial_filter() {
    let srv = geoserver::server().await;
    let body = r#"<wfs:GetFeature service="WFS" outputFormat="application/json" version="1.1.0" xmlns:wfs="http://www.opengis.net/wfs" xmlns:ogc="http://www.opengis.net/ogc" xmlns:gml="http://www.opengis.net/gml" xmlns:sf="http://cite.opengeospatial.org/gmlsf"><wfs:Query typeName="sf:AggregateGeoFeature" srsName="EPSG:900913"><ogc:Filter><ogc:Intersects><ogc:PropertyName></ogc:PropertyName><gml:Polygon srsName="EPSG:900913"><gml:exterior><gml:LinearRing><gml:posList>7666573.330932751 3485566.812628661 8010550.557483965 3485566.812628661 8010550.557483965 3788277.001334882 7666573.330932751 3788277.001334882 7666573.330932751 3485566.812628661</gml:posList></gml:LinearRing></gml:exterior></gml:Polygon></ogc:Intersects></ogc:Filter></wfs:Query></wfs:GetFeature>"#;
    let r = srv.post_wfs(body).await;
    assert_json_ct(&r);
    assert_eq!(json(&r)["totalFeatures"], 1);
}

/// GeoServer wfs1_x GeoJSONTest.testGetFeatureCRS
#[actix_web::test]
async fn gs_geojson_test_get_feature_crs() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&request=GetFeature&version=1.0.0&typename=cgf:Lines&outputformat=application/json").await;
    assert_json_ct(&r);
    let j = json(&r);
    let crs = j["crs"]["properties"]["name"].as_str().unwrap_or("");
    assert!(crs.ends_with("32615"), "crs {crs}");
}

/// GeoServer wfs1_x GeoJSONTest.testBoundingBoxAxisOrderInWfs10AndWfs11 (on cite:BasicPolygons:
/// the GeoJSON bbox stays east/north for urn lat/lon srsName)
#[actix_web::test]
async fn gs_geojson_test_bounding_box_axis_order_in_wfs10_and_wfs11() {
    let srv = geoserver::server().await;
    let a = json(&srv.get("/wfs?service=WFS&request=GetFeature&version=1.0.0&typename=cite:BasicPolygons&outputformat=application/json").await);
    let b = json(&srv.get("/wfs?service=WFS&request=GetFeature&version=1.1.0&typename=cite:BasicPolygons&outputformat=application/json&srsName=urn:x-ogc:def:crs:EPSG:4326").await);
    assert!(a["bbox"].is_array(), "bbox present: {}", a["bbox"]);
    assert_eq!(a["bbox"], b["bbox"]);
    // BasicPolygons extent: x -2..2, y -1..6
    assert_eq!(a["bbox"], serde_json::json!([-2.0, -1.0, 2.0, 6.0]));
}

// ------------------------------------------------------------- wfs1_x json.GeoJsonDescribeTest

fn check_describe_json(j: &Value) {
    assert_eq!(j["elementFormDefault"], "qualified");
    assert_eq!(j["targetNamespace"], "http://cite.opengeospatial.org/gmlsf");
    assert_eq!(j["targetPrefix"], "sf");
    let fts = j["featureTypes"].as_array().expect("featureTypes");
    assert_eq!(fts.len(), 1);
    assert_eq!(fts[0]["typeName"], "PrimitiveGeoFeature");
    let props = fts[0]["properties"].as_array().expect("properties");
    let names: Vec<&str> = props
        .iter()
        .map(|p| p["name"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        names,
        [
            "description",
            "name",
            "surfaceProperty",
            "pointProperty",
            "curveProperty",
            "intProperty",
            "uriProperty",
            "measurand",
            "dateTimeProperty",
            "dateProperty",
            "decimalProperty",
            "booleanProperty"
        ]
    );
    for p in props {
        assert_eq!(p["minOccurs"], 0, "{p}");
        assert_eq!(p["maxOccurs"], 1, "{p}");
        assert_eq!(p["nillable"], true, "{p}");
    }
    assert_eq!(props[0]["localType"], "string");
    assert_eq!(props[2]["type"], "gml:Polygon");
    assert_eq!(props[2]["localType"], "Polygon");
    assert_eq!(props[11]["localType"], "boolean");
}

/// GeoServer wfs1_x GeoJsonDescribeTest.testDescribePrimitiveGeoFeatureJSON
#[actix_web::test]
async fn gs_geojson_describe_test_describe_primitive_geo_feature_json() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&request=DescribeFeatureType&version=1.0.0&outputFormat=application/json&typeName=sf:PrimitiveGeoFeature").await;
    assert_json_ct(&r);
    check_describe_json(&json(&r));
}

/// GeoServer wfs1_x GeoJsonDescribeTest.testDescribePrimitiveGeoFeatureJSONP
#[actix_web::test]
async fn gs_geojson_describe_test_describe_primitive_geo_feature_jsonp() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&request=DescribeFeatureType&version=1.0.0&outputFormat=text/javascript&typeName=sf:PrimitiveGeoFeature").await;
    r.assert_ok();
    check_describe_json(&jsonp(&r.body, "parseResponse"));
}

/// GeoServer wfs1_x GeoJsonDescribeTest.testDescribePrimitiveGeoFeatureJSONPCustom
#[actix_web::test]
async fn gs_geojson_describe_test_describe_primitive_geo_feature_jsonp_custom() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&request=DescribeFeatureType&version=1.0.0&outputFormat=text/javascript&typeName=sf:PrimitiveGeoFeature&format_options=callback:custom").await;
    r.assert_ok();
    check_describe_json(&jsonp(&r.body, "custom"));
}

// ------------------------------------------------------------------ wfs2_x json.GeoJSONTest

/// GeoServer wfs2_x GeoJSONTest.testGetFeatureCountWfs20
#[actix_web::test]
async fn gs_geojson_test_get_feature_count_wfs20() {
    let srv = geoserver::server().await;
    let base = "/wfs?service=WFS&request=GetFeature&version=2.0.0&outputformat=application/json";
    for (q, n) in [
        ("&typename=sf:PrimitiveGeoFeature&count=10", 5),
        ("&typename=sf:PrimitiveGeoFeature&count=10&featureid=PrimitiveGeoFeature.f001", 1),
        ("&typename=sf:PrimitiveGeoFeature&count=1&featureid=PrimitiveGeoFeature.f001,PrimitiveGeoFeature.f002", 2),
        ("&typename=sf:PrimitiveGeoFeature,sf:AggregateGeoFeature&featureid=PrimitiveGeoFeature.f001,PrimitiveGeoFeature.f002,AggregateGeoFeature.f009", 3),
    ] {
        let r = srv.get(&format!("{base}{q}")).await;
        assert_json_ct(&r);
        let j = json(&r);
        assert_eq!(j["totalFeatures"], n, "{q}");
        assert_eq!(j["numberMatched"], n, "{q}");
        assert!(j.get("links").map(|l| l.is_null()).unwrap_or(true), "no links for {q}: {}", j["links"]);
    }
}

fn check_link(link: &Value, rel: &str, start: u32) {
    let title = if rel == "next" {
        "next page"
    } else {
        "previous page"
    };
    assert_eq!(link["title"], title, "{link}");
    assert_eq!(link["rel"], rel, "{link}");
    assert_eq!(link["type"], "application/json", "{link}");
    let href = link["href"].as_str().unwrap_or("").to_lowercase();
    assert!(
        href.contains("startindex=") && href.contains(&format!("startindex={start}")),
        "{href}"
    );
    assert!(href.contains("count=2"), "{href}");
    assert!(
        href.contains("outputformat=application%2fjson")
            || href.contains("outputformat=application/json"),
        "{href}"
    );
}

/// GeoServer wfs2_x GeoJSONTest.getGetFeatureWithPagingFirstPage
#[actix_web::test]
async fn gs_geojson_test_get_get_feature_with_paging_first_page() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&request=GetFeature&version=2.0.0&typename=sf:PrimitiveGeoFeature&startIndex=0&&count=2&outputformat=application/json").await;
    assert_json_ct(&r);
    let j = json(&r);
    assert_eq!(j["totalFeatures"], 5);
    assert_eq!(j["numberMatched"], 5);
    assert_eq!(j["numberReturned"], 2);
    let links = j["links"].as_array().expect("links");
    assert_eq!(links.len(), 1);
    check_link(&links[0], "next", 2);
}

/// GeoServer wfs2_x GeoJSONTest.getGetFeatureWithPagingMidPage
#[actix_web::test]
async fn gs_geojson_test_get_get_feature_with_paging_mid_page() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&request=GetFeature&version=2.0.0&typename=sf:PrimitiveGeoFeature&startIndex=2&count=2&outputformat=application/json").await;
    assert_json_ct(&r);
    let j = json(&r);
    assert_eq!(j["numberReturned"], 2);
    let links = j["links"].as_array().expect("links");
    assert_eq!(links.len(), 2);
    check_link(&links[0], "previous", 0);
    check_link(&links[1], "next", 4);
}

/// GeoServer wfs2_x GeoJSONTest.getGetFeatureWithPagingLastPage
#[actix_web::test]
async fn gs_geojson_test_get_get_feature_with_paging_last_page() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&request=GetFeature&version=2.0.0&typename=sf:PrimitiveGeoFeature&startIndex=4&count=2&outputformat=application/json").await;
    assert_json_ct(&r);
    let j = json(&r);
    assert_eq!(j["numberReturned"], 1);
    let links = j["links"].as_array().expect("links");
    assert_eq!(links.len(), 1);
    check_link(&links[0], "previous", 2);
}

// ------------------------------------------------------ wfs2_x response.GeoJsonOutputFormatTest

/// GeoServer wfs2_x GeoJsonOutputFormatTest.testCountZero
#[actix_web::test]
async fn gs_geojson_output_format_test_count_zero() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeName=sf:PrimitiveGeoFeature&outputFormat=json&count=0").await;
    assert_json_ct(&r);
    assert!(
        r.content_type.to_lowercase().contains("utf-8"),
        "charset in {}",
        r.content_type
    );
    assert_eq!(
        r.header("content-disposition"),
        Some("inline; filename=PrimitiveGeoFeature.json")
    );
    let j = json(&r);
    assert_eq!(j["totalFeatures"], 5);
    assert_eq!(j["numberMatched"], 5);
    assert_eq!(j["numberReturned"], 0);
    assert_eq!(j["features"].as_array().map(|a| a.len()), Some(0));
}

// --------------------------------------------------------- wfs1_x response.HitsOutputFormatTest

/// GeoServer wfs1_x HitsOutputFormatTest.testMimeType
#[actix_web::test]
async fn gs_hits_output_format_test_mime_type() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&version=1.1.0&request=GetFeature&typeName=sf:PrimitiveGeoFeature&resultType=hits").await;
    r.assert_ok();
    assert_eq!(r.content_type, "text/xml; subtype=gml/3.1.1");
    r.xml()
        .assert("/wfs:FeatureCollection[@numberOfFeatures='5']");
}

// ---------------------------------------------------------- wfs1_x response.CSVOutputFormatTest

fn assert_csv(r: &Response, filename: &str) {
    r.assert_ok();
    assert!(r.content_type.starts_with("text/csv"), "{}", r.content_type);
    assert!(
        r.content_type.to_lowercase().contains("utf-8"),
        "charset in {}",
        r.content_type
    );
    assert_eq!(
        r.header("content-disposition"),
        Some(format!("attachment; filename={filename}").as_str())
    );
}

/// GeoServer wfs1_x CSVOutputFormatTest.testWithAttributesRemoved (attribute selection by
/// PROPERTYNAME instead of layer configuration)
#[actix_web::test]
async fn gs_csv_output_format_test_with_attributes_removed() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&version=1.0.0&request=GetFeature&typeName=cite:RoadSegments&maxFeatures=50&outputFormat=text%2Fcsv&propertyname=NAME").await;
    assert_csv(&r, "RoadSegments.csv");
    assert_eq!(r.body.lines().next(), Some("FID,NAME"));
}

/// GeoServer wfs1_x CSVOutputFormatTest.testFullRequest
#[actix_web::test]
async fn gs_csv_output_format_test_full_request() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&version=1.1.0&request=GetFeature&typeName=sf:PrimitiveGeoFeature&outputFormat=csv").await;
    assert_csv(&r, "PrimitiveGeoFeature.csv");
    let records = csv_records(&r.body, ',');
    assert_eq!(records.len(), 6);
    for rec in &records {
        assert_eq!(rec.len(), PGF_ATTRS + 1, "{rec:?}");
    }
    assert_eq!(records[0][0], "FID");
}

/// GeoServer wfs1_x CSVOutputFormatTest.testHTMLStuff
#[actix_web::test]
async fn gs_csv_output_format_test_html_stuff() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&version=1.1.0&request=GetFeature&typeName=sf:PrimitiveGeoFeature&outputFormat=csv&format_options=filename:test").await;
    assert_csv(&r, "test.csv");
}

/// GeoServer wfs1_x CSVOutputFormatTest.testFullRequestWithDynamicCsvSeparator
#[actix_web::test]
async fn gs_csv_output_format_test_full_request_with_dynamic_csv_separator() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&version=1.1.0&request=GetFeature&typeName=sf:PrimitiveGeoFeature&outputFormat=csv&format_options=csvSeparator:-").await;
    assert_csv(&r, "PrimitiveGeoFeature.csv");
    let records = csv_records(&r.body, '-');
    assert_eq!(records.len(), 6);
    for rec in &records {
        assert_eq!(rec.len(), PGF_ATTRS + 1, "{rec:?}");
    }
}

/// GeoServer wfs1_x CSVOutputFormatTest.testDoubleQuotesAsCsvSeparator (via HTTP)
#[actix_web::test]
async fn gs_csv_output_format_test_double_quotes_as_csv_separator() {
    let srv = geoserver::server().await;
    let r = srv.get(&format!("/wfs?service=WFS&version=1.1.0&request=GetFeature&typeName=sf:PrimitiveGeoFeature&outputFormat=csv&format_options={}", enc("csvSeparator:\""))).await;
    assert!(
        r.body.contains("ExceptionReport"),
        "exception expected:\n{}",
        r.body
    );
}

/// GeoServer wfs1_x CSVOutputFormatTest.testSemicolonAsCsvSeparator
#[actix_web::test]
async fn gs_csv_output_format_test_semicolon_as_csv_separator() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&version=1.1.0&request=GetFeature&typeName=sf:PrimitiveGeoFeature&outputFormat=csv&format_options=csvSeparator:semicolon").await;
    assert_csv(&r, "PrimitiveGeoFeature.csv");
    let records = csv_records(&r.body, ';');
    assert_eq!(records.len(), 6);
    for rec in &records {
        assert_eq!(rec.len(), PGF_ATTRS + 1, "{rec:?}");
    }
}

// ---------------------------------------------------------- wfs2_x response.CSVOutputFormatTest

/// GeoServer wfs2_x CSVOutputFormatTest.testCountZero
#[actix_web::test]
async fn gs_csv_output_format_test_count_zero() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeName=sf:PrimitiveGeoFeature&outputFormat=csv&count=0").await;
    assert_csv(&r, "PrimitiveGeoFeature.csv");
    assert_eq!(r.body.lines().count(), 1, "{}", r.body);
}

// ------------------------------------------------------------------- wfs1_x response.ShapeZipTest

fn assert_zip(r: &Response, filename: &str) {
    r.assert_ok();
    assert_eq!(r.content_type, "application/zip");
    assert_eq!(
        r.header("content-disposition"),
        Some(format!("attachment; filename={filename}").as_str())
    );
}

fn assert_shapefile(names: &[String], base: &str) {
    for ext in ["shp", "shx", "dbf", "prj"] {
        assert!(
            names.iter().any(|n| n == &format!("{base}.{ext}")),
            "{base}.{ext} in {names:?}"
        );
    }
}

/// GeoServer wfs1_x ShapeZipTest.testNoNativeProjection
#[actix_web::test]
async fn gs_shape_zip_test_no_native_projection() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&version=1.0.0&request=GetFeature&typeName=cite:BasicPolygons&outputFormat=SHAPE-ZIP").await;
    assert_zip(&r, "BasicPolygons.zip");
    assert_shapefile(&zip_names(&r.bytes), "BasicPolygons");
    assert_eq!(shx_records(&zip_entry(&r.bytes, "BasicPolygons.shx")), 3);
}

/// GeoServer wfs1_x ShapeZipTest.testCharset
#[actix_web::test]
async fn gs_shape_zip_test_charset() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&version=1.0.0&request=GetFeature&typeName=cite:BasicPolygons&outputFormat=SHAPE-ZIP&format_options=CHARSET:ISO-8859-15").await;
    assert_zip(&r, "BasicPolygons.zip");
    let cst = String::from_utf8_lossy(&zip_entry(&r.bytes, "BasicPolygons.cst")).to_string();
    assert_eq!(cst.trim(), "ISO-8859-15");
}

/// GeoServer wfs1_x ShapeZipTest.testRequestUrlNoProxy
#[actix_web::test]
async fn gs_shape_zip_test_request_url_no_proxy() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&version=1.0.0&request=GetFeature&typeName=cite:BasicPolygons&outputFormat=SHAPE-ZIP").await;
    assert_zip(&r, "BasicPolygons.zip");
    let txt = String::from_utf8_lossy(&zip_entry(&r.bytes, "BasicPolygons.txt")).to_string();
    assert!(
        txt.contains(&format!("{BASE_URL}/wfs?")),
        "request dump:\n{txt}"
    );
    assert!(
        txt.to_lowercase().contains("request=getfeature"),
        "request dump:\n{txt}"
    );
}

/// GeoServer wfs1_x ShapeZipTest.testEmptyResult
#[actix_web::test]
async fn gs_shape_zip_test_empty_result() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&version=1.0.0&request=GetFeature&typeName=cite:BasicPolygons&outputFormat=SHAPE-ZIP&featureid=BasicPolygons.0").await;
    assert_zip(&r, "BasicPolygons.zip");
    assert_shapefile(&zip_names(&r.bytes), "BasicPolygons");
    assert_eq!(shx_records(&zip_entry(&r.bytes, "BasicPolygons.shx")), 0);
}

/// GeoServer wfs1_x ShapeZipTest.testEmptyResultMultiGeom (generic Geometry type: sf:GenericEntity)
#[actix_web::test]
async fn gs_shape_zip_test_empty_result_multi_geom() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&version=1.0.0&request=GetFeature&typeName=sf:GenericEntity&outputFormat=SHAPE-ZIP&featureid=GenericEntity.none").await;
    assert_zip(&r, "GenericEntity.zip");
    let names = zip_names(&r.bytes);
    assert!(names.iter().any(|n| n == "README.TXT"), "{names:?}");
}

/// GeoServer wfs1_x ShapeZipTest.testTemplatePOSTRequest10
#[actix_web::test]
async fn gs_shape_zip_test_template_post_request10() {
    let srv = geoserver::server().await;
    let body = r#"<wfs:GetFeature service="WFS" version="1.0.0" outputFormat="shape-zip" xmlns:cdf="http://www.opengis.net/cite/data" xmlns:ogc="http://www.opengis.net/ogc" xmlns:wfs="http://www.opengis.net/wfs"><wfs:Query typeName="cdf:Other"/></wfs:GetFeature>"#;
    let r = srv.post_wfs(body).await;
    r.assert_ok();
    assert_eq!(r.content_type, "application/zip");
    assert!(zip_names(&r.bytes).iter().any(|n| n.ends_with(".shp")));
}

/// GeoServer wfs1_x ShapeZipTest.testOutputZipFileNameSpecifiedInFormatOptions (on cite:BasicPolygons)
#[actix_web::test]
async fn gs_shape_zip_test_output_zip_file_name_specified_in_format_options() {
    let srv = geoserver::server().await;
    let base = "/wfs?service=WFS&version=1.0.0&request=GetFeature&typeName=cite:BasicPolygons&outputFormat=SHAPE-ZIP";
    assert_zip(&srv.get(base).await, "BasicPolygons.zip");
    assert_zip(
        &srv.get(&format!(
            "{base}&format_options=FILENAME:REQUEST_SUFFIX.zip"
        ))
        .await,
        "REQUEST_SUFFIX.zip",
    );
}

/// GeoServer wfs1_x ShapeZipTest.testTemplatePOSTRequest11
#[actix_web::test]
async fn gs_shape_zip_test_template_post_request11() {
    let srv = geoserver::server().await;
    let body = r#"<wfs:GetFeature service="WFS" version="1.1.0" outputFormat="shape-zip" maxFeatures="100" xmlns:cdf="http://www.opengis.net/cite/data" xmlns:ogc="http://www.opengis.net/ogc" xmlns:wfs="http://www.opengis.net/wfs"><wfs:Query typeName="cdf:Other" srsName="urn:ogc:def:crs:EPSG::4326"/></wfs:GetFeature>"#;
    let r = srv.post_wfs(body).await;
    r.assert_ok();
    assert_eq!(r.content_type, "application/zip");
    assert!(zip_names(&r.bytes).iter().any(|n| n.ends_with(".shp")));
}

/// GeoServer wfs1_x ShapeZipTest.testESRIFormat
#[actix_web::test]
async fn gs_shape_zip_test_esri_format() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&version=1.0.0&request=GetFeature&typeName=cite:BasicPolygons&outputFormat=SHAPE-ZIP&format_options=PRJFILEFORMAT:ESRI").await;
    assert_zip(&r, "BasicPolygons.zip");
    let prj = String::from_utf8_lossy(&zip_entry(&r.bytes, "BasicPolygons.prj")).to_string();
    assert!(
        prj.starts_with("GEOGCS[\"GCS_WGS_1984\""),
        "ESRI prj:\n{prj}"
    );
}

/// GeoServer wfs1_x ShapeZipTest.testMultiGeometryColumns
#[actix_web::test]
async fn gs_shape_zip_test_multi_geometry_columns() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&version=1.0.0&request=GetFeature&typeName=sf:PrimitiveGeoFeature&outputFormat=SHAPE-ZIP").await;
    assert_zip(&r, "PrimitiveGeoFeature.zip");
    let names = zip_names(&r.bytes);
    for (prop, n) in [
        ("curveProperty", 1),
        ("surfaceProperty", 1),
        ("pointProperty", 3),
    ] {
        let base = format!("PrimitiveGeoFeature{prop}");
        assert_shapefile(&names, &base);
        assert_eq!(
            shx_records(&zip_entry(&r.bytes, &format!("{base}.shx"))),
            n,
            "{base}"
        );
    }
}

/// GeoServer wfs1_x ShapeZipTest.testCountZero
#[actix_web::test]
async fn gs_shape_zip_test_count_zero() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&version=1.1.0&request=GetFeature&typeName=cite:BasicPolygons&outputFormat=SHAPE-ZIP&maxfeatures=0").await;
    assert_zip(&r, "BasicPolygons.zip");
    assert_shapefile(&zip_names(&r.bytes), "BasicPolygons");
    assert_eq!(shx_records(&zip_entry(&r.bytes, "BasicPolygons.shx")), 0);
}

// ----------------------------------------------------------------------------------- wfs-core

fn check_json_exception(j: &Value, version: &str) {
    assert_eq!(j["version"], version);
    let ex = j["exceptions"].as_array().expect("exceptions");
    assert_eq!(ex.len(), 1);
    assert!(ex[0]["code"].is_string(), "{}", ex[0]);
    assert!(
        ex[0]["text"].as_str().unwrap_or("").contains("foobar"),
        "{}",
        ex[0]
    );
}

/// GeoServer WFSServiceExceptionTestSupport.testJsonException (WFS 1.1.0 and 2.0.0)
#[actix_web::test]
async fn gs_wfs_service_exception_test_json_exception() {
    let srv = geoserver::server().await;
    for v in ["1.1.0", "2.0.0"] {
        let r = srv.get(&format!("/wfs?service=wfs&version={v}&request=DescribeFeatureType&typeName=foobar&format_options=callback:myMethod&EXCEPTIONS=application/json")).await;
        assert!(
            r.content_type.starts_with("application/json"),
            "{v}: {} {}",
            r.content_type,
            r.body
        );
        check_json_exception(&json(&r), v);
    }
}

/// GeoServer WFSServiceExceptionTestSupport.testJsonpException (WFS 1.1.0 and 2.0.0)
#[actix_web::test]
async fn gs_wfs_service_exception_test_jsonp_exception() {
    let srv = geoserver::server().await;
    for v in ["1.1.0", "2.0.0"] {
        let r = srv.get(&format!("/wfs?service=wfs&version={v}&request=DescribeFeatureType&typeName=foobar&format_options=callback:myMethod&EXCEPTIONS=text/javascript")).await;
        assert!(
            r.content_type.starts_with("text/javascript"),
            "{v}: {} {}",
            r.content_type,
            r.body
        );
        check_json_exception(&jsonp(&r.body, "myMethod"), v);
    }
}

/// GeoServer ExternalEntitiesTest.testAllowListFilter: filters referencing external schemas
/// parse without fetching them; external entities are never resolved.
#[actix_web::test]
async fn gs_external_entities_test_allow_list_filter() {
    let srv = geoserver::server().await;
    // FeatureId filter with an external xsi:schemaLocation: evaluated normally
    let filter = r#"<ogc:Filter xmlns:ogc="http://www.opengis.net/ogc" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:schemaLocation="http://www.opengis.net/ogc http://localhost:8080/dmt.xsd"><ogc:FeatureId fid="PrimitiveGeoFeature.f001"/></ogc:Filter>"#;
    let r = srv.get(&format!("/wfs?service=WFS&version=1.1.0&request=GetFeature&typeName=sf:PrimitiveGeoFeature&filter={}", enc(filter))).await;
    r.assert_ok();
    r.xml()
        .assert("/wfs:FeatureCollection[@numberOfFeatures='1']");
    // external entity in the filter: not resolved, request rejected
    let filter = r#"<!DOCTYPE ogc:Filter [<!ENTITY c SYSTEM "file:///etc/passwd">]><ogc:Filter xmlns:ogc="http://www.opengis.net/ogc"><ogc:PropertyIsEqualTo><ogc:PropertyName>name</ogc:PropertyName><ogc:Literal>&c;</ogc:Literal></ogc:PropertyIsEqualTo></ogc:Filter>"#;
    let r = srv.get(&format!("/wfs?service=WFS&version=1.1.0&request=GetFeature&typeName=sf:PrimitiveGeoFeature&filter={}", enc(filter))).await;
    assert!(!r.body.contains("root:"), "external entity resolved");
    assert!(r.body.contains("ExceptionReport"), "{}", r.body);
}

/// GeoServer xml.GML3ProfileTest.testMultiSurfacePolygon: MultiPolygon properties are described
/// as gml:MultiSurfacePropertyType in GML 3
#[actix_web::test]
async fn gs_gml3_profile_test_multi_surface_polygon() {
    let srv = geoserver::server().await;
    let r = srv.get("/wfs?service=WFS&version=1.1.0&request=DescribeFeatureType&typeName=sf:AggregateGeoFeature").await;
    r.assert_ok();
    let xml = r.xml();
    assert_eq!(
        xml.string("//xs:element[@name='multiSurfaceProperty']/@type"),
        "gml:MultiSurfacePropertyType",
        "{}",
        r.body
    );
}
