//! GeoServer compatible output formats: GeoJSON, JSONP, CSV, SHAPE-ZIP, KML.

mod common;
use common::*;

#[actix_web::test]
async fn geojson() {
    let srv = ne_server().await;
    for format in ["application/json", "json", "application/geo%2Bjson"] {
        let resp = srv.get(&format!("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=ne:populated_places&count=3&outputFormat={format}")).await;
        resp.assert_ok();
        assert!(
            resp.content_type.starts_with("application/json")
                || resp.content_type.starts_with("application/geo+json"),
            "{}",
            resp.content_type
        );
        let json: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(json["type"], "FeatureCollection");
        assert_eq!(json["numberMatched"], 7342);
        assert_eq!(json["totalFeatures"], 7342);
        assert_eq!(json["numberReturned"], 3);
        let features = json["features"].as_array().unwrap();
        assert_eq!(features.len(), 3);
        let f = &features[0];
        assert_eq!(f["type"], "Feature");
        assert!(f["id"].as_str().unwrap().starts_with("populated_places."));
        assert_eq!(f["geometry_name"], "geom");
        assert_eq!(f["geometry"]["type"], "Point");
        // GeoJSON is lon/lat
        let c = f["geometry"]["coordinates"].as_array().unwrap();
        assert!(c[0].as_f64().unwrap().abs() <= 180.0);
        assert!(f["properties"]["NAME"].is_string());
        assert!(f["properties"]["SCALERANK"].is_number());
        assert!(f["properties"].get("geom").is_none());
    }
    // filters and other versions
    let resp = srv.get("/wfs?service=WFS&version=1.1.0&request=GetFeature&typeName=ne:lakes&maxFeatures=2&outputFormat=application/json").await;
    let json: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(json["features"].as_array().unwrap().len(), 2);
    assert_eq!(json["features"][0]["geometry"]["type"], "MultiPolygon");
    // reprojected output declares crs
    let resp = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=ne:lakes&count=1&outputFormat=application/json&srsName=EPSG:3857").await;
    let json: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(
        json["crs"]["properties"]["name"],
        "urn:ogc:def:crs:EPSG::3857"
    );
    let x = json["features"][0]["geometry"]["coordinates"][0][0][0][0]
        .as_f64()
        .unwrap();
    assert!(x.abs() > 1000.0);
    // hits
    let resp = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=ne:lakes&resultType=hits&outputFormat=application/json").await;
    let json: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(json["numberMatched"], 1355);
    assert_eq!(json["features"].as_array().unwrap().len(), 0);
}

#[actix_web::test]
async fn jsonp() {
    let srv = ne_server().await;
    let resp = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=ne:lakes&count=1&outputFormat=text/javascript&format_options=callback:handle").await;
    resp.assert_ok();
    assert!(
        resp.content_type.starts_with("text/javascript"),
        "{}",
        resp.content_type
    );
    assert!(resp.body.starts_with("handle("), "{}", &resp.body[..40]);
    assert!(resp.body.trim_end().ends_with(')'));
    let inner = &resp.body["handle(".len()..resp.body.trim_end().len() - 1];
    let json: serde_json::Value = serde_json::from_str(inner).unwrap();
    assert_eq!(json["type"], "FeatureCollection");
    let resp = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=ne:lakes&count=1&outputFormat=text/javascript").await;
    assert!(resp.body.starts_with("parseResponse("));
}

#[actix_web::test]
async fn csv() {
    let srv = ne_server().await;
    let resp = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=ne:lakes&count=3&outputFormat=csv").await;
    resp.assert_ok();
    assert!(
        resp.content_type.starts_with("text/csv"),
        "{}",
        resp.content_type
    );
    let lines: Vec<&str> = resp.body.lines().collect();
    assert_eq!(lines.len(), 4);
    // `name` is mapped to gml:name, standard GML properties come first
    assert_eq!(lines[0], "FID,name,geom,scalerank,featurecla");
    assert!(
        lines[1].starts_with("lakes.1,Eğirdir,\"MULTIPOLYGON"),
        "{}",
        lines[1]
    );
}

#[actix_web::test]
async fn shape_zip() {
    let srv = ne_server().await;
    let resp = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=ne:populated_places&count=10&outputFormat=SHAPE-ZIP").await;
    resp.assert_ok();
    assert_eq!(resp.content_type, "application/zip");
    let bytes = &resp.bytes;
    assert_eq!(&bytes[..2], b"PK");
    let names = zip_names(bytes);
    for ext in ["shp", "shx", "dbf", "prj"] {
        assert!(
            names
                .iter()
                .any(|n| n == &format!("populated_places.{ext}")),
            "{names:?}"
        );
    }
}

/// File names in the central directory of a zip archive
fn zip_names(bytes: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut i = 0;
    while i + 46 <= bytes.len() {
        if bytes[i..i + 4] == [0x50, 0x4b, 0x01, 0x02] {
            let n = u16::from_le_bytes([bytes[i + 28], bytes[i + 29]]) as usize;
            let m = u16::from_le_bytes([bytes[i + 30], bytes[i + 31]]) as usize;
            let k = u16::from_le_bytes([bytes[i + 32], bytes[i + 33]]) as usize;
            names.push(String::from_utf8_lossy(&bytes[i + 46..i + 46 + n]).to_string());
            i += 46 + n + m + k;
        } else {
            i += 1;
        }
    }
    names
}

#[actix_web::test]
async fn kml() {
    let srv = ne_server().await;
    let resp = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=ne:populated_places&count=2&outputFormat=application/vnd.google-earth.kml%2Bxml").await;
    resp.assert_ok();
    assert!(
        resp.content_type
            .starts_with("application/vnd.google-earth.kml+xml"),
        "{}",
        resp.content_type
    );
    let xml = resp.xml();
    assert_eq!(xml.count("//*[local-name()='Placemark']"), 2);
    let coords = xml.string("(//*[local-name()='Point']/*[local-name()='coordinates'])[1]");
    let c: Vec<f64> = coords
        .split(',')
        .map(|v| v.trim().parse().unwrap())
        .collect();
    assert!(c[0].abs() <= 180.0 && c[1].abs() <= 90.0);
    assert!(xml.boolean("//*[local-name()='ExtendedData']/*[local-name()='Data'][@name='NAME']"));
}

#[actix_web::test]
async fn unknown_format() {
    let srv = ne_server().await;
    let resp = srv.get("/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=ne:lakes&outputFormat=image/png").await;
    assert_eq!(resp.status, 400);
    let xml = resp.assert_exception("InvalidParameterValue");
    assert_eq!(xml.string("//ows11:Exception/@locator"), "outputFormat");
}
