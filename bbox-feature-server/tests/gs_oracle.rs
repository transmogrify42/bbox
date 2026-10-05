//! Differential tests against a GeoServer reference instance serving the same catalog.
//!
//!   tests/geoserver/start-oracle.sh
//!   BBOX_WFS_TEST_GEOSERVER=http://127.0.0.1:18082/geoserver cargo test --test gs_oracle -- --ignored --nocapture
//!
//! Responses are normalized (feature ids, namespace qualified property names and values,
//! geometry coordinates rounded to 1e-6, counts, exception codes and HTTP status) and compared.
//! Known, intended differences are listed in `ACCEPTED`.

mod common;
use common::*;
use std::collections::BTreeMap;

/// Percent-encode a query string (keeping `&`, `=` and `,` separators)
fn encode_query(q: &str) -> String {
    let mut out = String::new();
    for b in q.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'&'
            | b'='
            | b','
            | b':' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn geoserver_url() -> Option<String> {
    std::env::var("BBOX_WFS_TEST_GEOSERVER").ok()
}

const GML_NS: [&str; 2] = [
    "http://www.opengis.net/gml",
    "http://www.opengis.net/gml/3.2",
];

#[derive(Debug, PartialEq)]
enum Normalized {
    Features {
        matched: Option<String>,
        returned: Option<String>,
        /// feature id -> property -> value
        features: BTreeMap<String, BTreeMap<String, String>>,
        order: Vec<String>,
    },
    Values(Vec<String>),
    Exception {
        status: u16,
        code: String,
    },
    Json(serde_json::Value),
    Other(String),
}

fn coords(text: &str) -> String {
    text.split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .map(|n| match n.parse::<f64>() {
            Ok(v) => format!("{:.6}", v),
            Err(_) => n.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Value of a property element: geometry coordinates or text
fn property_value(node: roxmltree::Node) -> String {
    if let Some(g) = node.children().find(|c| c.is_element()) {
        let ns = g.tag_name().namespace().unwrap_or("");
        if GML_NS.contains(&ns) {
            let nums: Vec<String> = g
                .descendants()
                .filter(|d| {
                    d.is_element()
                        && matches!(
                            d.tag_name().name(),
                            "pos" | "posList" | "coordinates" | "lowerCorner" | "upperCorner"
                        )
                })
                .map(|d| coords(d.text().unwrap_or("")))
                .collect();
            return format!("{}({})", g.tag_name().name(), nums.join(" | "));
        }
        // complex content: element names and text
        return g
            .descendants()
            .filter(|d| d.is_text())
            .map(|d| d.text().unwrap_or("").trim().to_string())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
    }
    let t = node.text().unwrap_or("").trim().to_string();
    if node.attribute(("http://www.w3.org/2001/XMLSchema-instance", "nil")) == Some("true") {
        return "<nil>".to_string();
    }
    t
}

fn clark(node: &roxmltree::Node) -> String {
    format!(
        "{{{}}}{}",
        node.tag_name().namespace().unwrap_or(""),
        node.tag_name().name()
    )
}

fn normalize(status: u16, content_type: &str, body: &str) -> Normalized {
    if content_type.contains("json") {
        let mut v: serde_json::Value =
            serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
        if let Some(o) = v.as_object_mut() {
            for k in ["timeStamp", "crs", "bbox", "links", "totalFeatures"] {
                o.remove(k);
            }
        }
        return Normalized::Json(v);
    }
    let Ok(doc) = roxmltree::Document::parse(body) else {
        return Normalized::Other(format!(
            "{status} {content_type} {}",
            &body[..body.len().min(200)]
        ));
    };
    let root = doc.root_element();
    match root.tag_name().name() {
        "ExceptionReport" | "ServiceExceptionReport" => {
            let code = root
                .descendants()
                .find(|n| {
                    n.is_element()
                        && matches!(n.tag_name().name(), "Exception" | "ServiceException")
                })
                .and_then(|n| n.attribute("exceptionCode").or(n.attribute("code")))
                .unwrap_or("")
                .to_string();
            Normalized::Exception { status, code }
        }
        "FeatureCollection" => {
            let matched = root
                .attribute("numberMatched")
                .or(root.attribute("numberOfFeatures"))
                .map(str::to_string);
            let returned = root.attribute("numberReturned").map(str::to_string);
            let mut features = BTreeMap::new();
            let mut order = Vec::new();
            for m in root.children().filter(|c| {
                c.is_element()
                    && matches!(
                        c.tag_name().name(),
                        "featureMember" | "featureMembers" | "member"
                    )
            }) {
                for f in m.children().filter(|c| c.is_element()) {
                    let id = f
                        .attributes()
                        .find(|a| a.name() == "id" || a.name() == "fid")
                        .map(|a| a.value().to_string())
                        .unwrap_or_default();
                    let mut props = BTreeMap::new();
                    for p in f.children().filter(|c| c.is_element()) {
                        if p.tag_name().name() == "boundedBy"
                            && GML_NS.contains(&p.tag_name().namespace().unwrap_or(""))
                        {
                            continue;
                        }
                        props.insert(clark(&p), property_value(p));
                    }
                    order.push(id.clone());
                    features.insert(id, props);
                }
            }
            Normalized::Features {
                matched,
                returned,
                features,
                order,
            }
        }
        "ValueCollection" => Normalized::Values(
            root.children()
                .filter(|c| c.is_element() && c.tag_name().name() == "member")
                .map(|m| property_value(m))
                .collect(),
        ),
        other => Normalized::Other(format!("{status} <{other}>")),
    }
}

/// Requests (path with query, or POST body) sent to both servers
fn requests() -> Vec<(String, Option<String>)> {
    let mut r: Vec<(String, Option<String>)> = Vec::new();
    let types: Vec<String> = geoserver::NAMESPACES
        .iter()
        .flat_map(|ns| ns.types.iter().map(move |t| format!("{}:{t}", ns.prefix)))
        .collect();
    for t in &types {
        for v in ["1.0.0", "1.1.0", "2.0.0"] {
            let tn = if v == "2.0.0" {
                "typeNames"
            } else {
                "typeName"
            };
            r.push((
                format!("service=WFS&version={v}&request=GetFeature&{tn}={t}"),
                None,
            ));
        }
        r.push((
            format!("service=WFS&version=2.0.0&request=GetFeature&typeNames={t}&resultType=hits"),
            None,
        ));
    }
    let gf20 = "service=WFS&version=2.0.0&request=GetFeature";
    let gf11 = "service=WFS&version=1.1.0&request=GetFeature";
    for q in [
        // paging, sorting, projection
        format!("{gf20}&typeNames=cite:Buildings&count=1&startIndex=1"),
        format!("{gf20}&typeNames=cdf:Fifteen&count=5&startIndex=12"),
        format!("{gf20}&typeNames=sf:PrimitiveGeoFeature&sortBy=intProperty DESC"),
        format!("{gf20}&typeNames=sf:PrimitiveGeoFeature&sortBy=name ASC"),
        format!("{gf20}&typeNames=sf:PrimitiveGeoFeature&propertyName=intProperty,pointProperty"),
        format!("{gf11}&typeName=sf:PrimitiveGeoFeature&maxFeatures=3"),
        // ids
        format!("{gf20}&resourceId=PrimitiveGeoFeature.f001,PrimitiveGeoFeature.f008"),
        format!("{gf11}&featureId=Fifteen.3"),
        format!("{gf20}&typeNames=cdf:Fifteen&resourceId=Fifteen.0"),
        // bbox and srsName
        format!("{gf20}&typeNames=sf:PrimitiveGeoFeature&bbox=30,0,60,30,urn:ogc:def:crs:EPSG::4326"),
        format!("{gf20}&typeNames=sf:PrimitiveGeoFeature&bbox=0,30,30,60"),
        format!("{gf11}&typeName=sf:PrimitiveGeoFeature&bbox=0,30,30,60,EPSG:4326"),
        format!("{gf20}&typeNames=cite:Lakes&srsName=EPSG:3857"),
        format!("{gf20}&typeNames=cdf:Seven&srsName=urn:ogc:def:crs:EPSG::4326"),
        "service=WFS&version=1.0.0&request=GetFeature&typeName=cgf:Points&srsName=EPSG:4326".to_string(),
        // CQL
        format!("{gf20}&typeNames=sf:PrimitiveGeoFeature&cql_filter=intProperty > 150"),
        format!("{gf20}&typeNames=sf:PrimitiveGeoFeature&cql_filter=name LIKE 'name-f00%'"),
        format!("{gf20}&typeNames=cite:Buildings&cql_filter=ADDRESS = '123 Main Street'"),
        format!("{gf20}&typeNames=sf:PrimitiveGeoFeature&cql_filter=BBOX(pointProperty, 0, 30, 30, 60)"),
        format!("{gf20}&typeNames=sf:PrimitiveGeoFeature&cql_filter=dateProperty AFTER 2006-01-01T00:00:00Z"),
        format!("{gf20}&typeNames=sf:PrimitiveGeoFeature&cql_filter=intProperty BETWEEN 150 AND 300 AND NOT (booleanProperty = true)"),
        // formats
        format!("{gf20}&typeNames=sf:PrimitiveGeoFeature&outputFormat=application/json"),
        format!("{gf20}&typeNames=cite:Lakes&outputFormat=application/json"),
        // GetPropertyValue, stored query
        "service=WFS&version=2.0.0&request=GetPropertyValue&typeNames=sf:PrimitiveGeoFeature&valueReference=intProperty".to_string(),
        "service=WFS&version=2.0.0&request=GetFeature&storedQuery_id=urn:ogc:def:query:OGC-WFS::GetFeatureById&ID=PrimitiveGeoFeature.f002".to_string(),
        // errors
        format!("{gf20}&typeNames=sf:Nope"),
        format!("{gf20}&typeNames=sf:PrimitiveGeoFeature&count=abc"),
        format!("{gf20}&typeNames=sf:PrimitiveGeoFeature&propertyName=nope"),
        "service=WFS&version=2.0.0&request=GetFeature".to_string(),
        "service=WFS&version=2.0.0&request=Unknown".to_string(),
        "service=WFS&version=1.1.0&request=GetFeature&typeName=sf:Nope".to_string(),
        "service=WFS&version=1.0.0&request=GetFeature&typeName=sf:Nope".to_string(),
    ] {
        r.push((q, None));
    }
    let fes = |body: &str| {
        format!(
            r#"<wfs:GetFeature service="WFS" version="2.0.0" xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:sf="http://cite.opengeospatial.org/gmlsf" xmlns:cdf="http://www.opengis.net/cite/data"><wfs:Query typeNames="sf:PrimitiveGeoFeature"><fes:Filter>{body}</fes:Filter></wfs:Query></wfs:GetFeature>"#
        )
    };
    for f in [
        "<fes:PropertyIsEqualTo><fes:ValueReference>sf:intProperty</fes:ValueReference><fes:Literal>155</fes:Literal></fes:PropertyIsEqualTo>",
        "<fes:PropertyIsLessThan><fes:ValueReference>sf:decimalProperty</fes:ValueReference><fes:Literal>10</fes:Literal></fes:PropertyIsLessThan>",
        "<fes:PropertyIsLike wildCard=\"*\" singleChar=\"#\" escapeChar=\"!\"><fes:ValueReference>sf:uriProperty</fes:ValueReference><fes:Literal>*opengeospatial*</fes:Literal></fes:PropertyIsLike>",
        "<fes:PropertyIsNull><fes:ValueReference>sf:curveProperty</fes:ValueReference></fes:PropertyIsNull>",
        "<fes:PropertyIsBetween><fes:ValueReference>sf:intProperty</fes:ValueReference><fes:LowerBoundary><fes:Literal>100</fes:Literal></fes:LowerBoundary><fes:UpperBoundary><fes:Literal>200</fes:Literal></fes:UpperBoundary></fes:PropertyIsBetween>",
        "<fes:BBOX><fes:ValueReference>sf:pointProperty</fes:ValueReference><gml:Envelope srsName=\"urn:ogc:def:crs:EPSG::4326\"><gml:lowerCorner>30 0</gml:lowerCorner><gml:upperCorner>60 30</gml:upperCorner></gml:Envelope></fes:BBOX>",
        "<fes:Intersects><fes:ValueReference>sf:pointProperty</fes:ValueReference><gml:Polygon gml:id=\"p\" srsName=\"urn:ogc:def:crs:EPSG::4326\"><gml:exterior><gml:LinearRing><gml:posList>30 0 60 0 60 30 30 30 30 0</gml:posList></gml:LinearRing></gml:exterior></gml:Polygon></fes:Intersects>",
        "<fes:DWithin><fes:ValueReference>sf:pointProperty</fes:ValueReference><gml:Point gml:id=\"p\" srsName=\"urn:ogc:def:crs:EPSG::4326\"><gml:pos>39.73245 2.00342</gml:pos></gml:Point><fes:Distance uom=\"m\">1000000</fes:Distance></fes:DWithin>",
        "<fes:ResourceId rid=\"PrimitiveGeoFeature.f001\"/><fes:ResourceId rid=\"PrimitiveGeoFeature.f015\"/>",
        "<fes:Or><fes:PropertyIsEqualTo><fes:ValueReference>sf:intProperty</fes:ValueReference><fes:Literal>155</fes:Literal></fes:PropertyIsEqualTo><fes:Not><fes:PropertyIsGreaterThan><fes:ValueReference>sf:intProperty</fes:ValueReference><fes:Literal>0</fes:Literal></fes:PropertyIsGreaterThan></fes:Not></fes:Or>",
        "<fes:PropertyIsEqualTo><fes:ValueReference>gml:name</fes:ValueReference><fes:Literal>name-f003</fes:Literal></fes:PropertyIsEqualTo>",
        "<fes:After><fes:ValueReference>sf:dateTimeProperty</fes:ValueReference><gml:TimeInstant gml:id=\"t\"><gml:timePosition>2006-01-01T00:00:00Z</gml:timePosition></gml:TimeInstant></fes:After>",
    ] {
        r.push(("POST".to_string(), Some(fes(f))));
    }
    r
}

/// Differences accepted as intended (substring match on the diff text)
const ACCEPTED: &[&str] = &[
    // GeoServer drops the time of day of `2006-06-27 22:08:00-07` (property file timestamp)
    "geoserver `2006-06-27T00:00:00Z` bbox `2006-06-28T05:08:00Z`",
    r#"geoserver "2006-06-27T00:00:00Z" bbox "2006-06-28T05:08:00Z""#,
    // unknown operation: WFS 2.0 Table 3 specifies 501 for OperationNotSupported
    r#"geoserver Exception { status: 400, code: "OperationNotSupported" } bbox Exception { status: 501, code: "OperationNotSupported" }"#,
    // GeoServer ignores the distance unit of DWithin on geographic data (treats 1000000 as
    // degrees); bbox converts metres
    r#"<fes:Distance uom="m">1000000</fes:Distance>"#,
];

/// Value equality with intended representation differences:
/// - xs:date with UTC designator (`2006-10-25Z`, GeoServer) and without (bbox keeps the stored value)
/// - numerically equal numbers (`0.0` / `0`)
/// - GML 3.1.1 gml:MultiLineString (GeoServer) and gml:MultiCurve (bbox, required by
///   MultiCurvePropertyType properties) with identical coordinates
fn equivalent(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    if a.strip_suffix('Z') == Some(b) && b.len() == 10 {
        return true;
    }
    if let (Ok(x), Ok(y)) = (a.parse::<f64>(), b.parse::<f64>()) {
        return x == y;
    }
    match (
        a.strip_prefix("MultiLineString("),
        b.strip_prefix("MultiCurve("),
    ) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// JSON with representation differences normalized (see `equivalent`)
fn normalize_json(v: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match v {
        Value::Number(n) => n
            .as_f64()
            .map(|f| serde_json::json!(f))
            .unwrap_or(v.clone()),
        Value::String(s) if s.len() == 11 && s.ends_with('Z') && s.as_bytes()[4] == b'-' => {
            Value::String(s[..10].to_string())
        }
        Value::Array(a) => Value::Array(a.iter().map(normalize_json).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), normalize_json(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[actix_web::test]
#[ignore]
async fn geoserver_differential() {
    let Some(gs) = geoserver_url() else { return };
    let srv = geoserver::server().await;
    let client = reqwest::Client::new();
    let mut diffs = Vec::new();
    let reqs = requests();
    for (q, body) in &reqs {
        let (gs_resp, bbox_resp) = match body {
            None => {
                let r = client
                    .get(format!("{gs}/wfs?{}", encode_query(q)))
                    .send()
                    .await
                    .unwrap();
                let bb = srv.get(&format!("/wfs?{}", encode_query(q))).await;
                (r, bb)
            }
            Some(b) => {
                let r = client
                    .post(format!("{gs}/wfs"))
                    .header("Content-Type", "text/xml")
                    .body(b.clone())
                    .send()
                    .await
                    .unwrap();
                let bb = srv.post_with_type("/wfs", b, "text/xml").await;
                (r, bb)
            }
        };
        let gs_status = gs_resp.status().as_u16();
        let gs_ct = gs_resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let gs_body = gs_resp.text().await.unwrap();
        let a = normalize(gs_status, &gs_ct, &gs_body);
        let b = normalize(bbox_resp.status, &bbox_resp.content_type, &bbox_resp.body);
        let label = body
            .as_deref()
            .map(|b| b.split("<fes:Filter>").nth(1).unwrap_or(b).to_string())
            .unwrap_or_else(|| q.clone());
        let paged_unsorted = q.contains("startIndex") && !q.contains("sortBy");
        for d in compare(&a, &b) {
            if paged_unsorted && d.starts_with("feature ids:") {
                // property store pages in string order of ids; counts are compared separately
                continue;
            }
            let line = format!("{label}\n    {d}");
            if !ACCEPTED.iter().any(|acc| line.contains(acc)) {
                diffs.push(line);
            }
        }
    }
    for d in &diffs {
        println!("- {d}");
    }
    println!("{} requests, {} differences", reqs.len(), diffs.len());
    assert!(diffs.is_empty(), "{} differences to GeoServer", diffs.len());
}

fn compare(gs: &Normalized, bbox: &Normalized) -> Vec<String> {
    use Normalized::*;
    let mut out = Vec::new();
    match (gs, bbox) {
        (
            Features {
                matched: m1,
                returned: r1,
                features: f1,
                order: o1,
            },
            Features {
                matched: m2,
                returned: r2,
                features: f2,
                order: o2,
            },
        ) => {
            if m1 != m2 {
                out.push(format!("numberMatched: geoserver {m1:?} bbox {m2:?}"));
            }
            if r1 != r2 {
                out.push(format!("numberReturned: geoserver {r1:?} bbox {r2:?}"));
            }
            let ids1: Vec<&String> = f1.keys().collect();
            let ids2: Vec<&String> = f2.keys().collect();
            if ids1 != ids2 {
                // pages of GeoServer's property store follow string order of ids ("Fifteen.10" <
                // "Fifteen.2"), bbox pages follow the store order: compare only counts
                out.push(format!("feature ids: geoserver {ids1:?} bbox {ids2:?}"));
            } else if o1 != o2 {
                out.push(format!("feature order: geoserver {o1:?} bbox {o2:?}"));
            }
            for (id, p1) in f1 {
                let Some(p2) = f2.get(id) else { continue };
                for (k, v1) in p1 {
                    match p2.get(k) {
                        None => out.push(format!(
                            "{id}: property {k} missing in bbox (geoserver `{v1}`)"
                        )),
                        Some(v2) if !equivalent(v1, v2) => {
                            out.push(format!("{id}: {k} geoserver `{v1}` bbox `{v2}`"))
                        }
                        _ => {}
                    }
                }
                for k in p2.keys().filter(|k| !p1.contains_key(*k)) {
                    out.push(format!("{id}: property {k} only in bbox (`{}`)", p2[k]));
                }
            }
        }
        (Json(a), Json(b)) => {
            let (a, b) = (normalize_json(a), normalize_json(b));
            if a != b {
                let mut paths = Vec::new();
                json_diff("", &a, &b, &mut paths);
                for p in paths.into_iter().take(10) {
                    out.push(format!("json: {p}"));
                }
            }
        }
        (a, b) if a != b => out.push(format!("geoserver {a:?} bbox {b:?}")),
        _ => {}
    }
    // summarize long lists
    if out.len() > 40 {
        let n = out.len();
        out.truncate(40);
        out.push(format!("... {} more", n - 40));
    }
    out
}

/// Paths where two JSON values differ
fn json_diff(path: &str, a: &serde_json::Value, b: &serde_json::Value, out: &mut Vec<String>) {
    use serde_json::Value;
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for k in x.keys().chain(y.keys().filter(|k| !x.contains_key(*k))) {
                match (x.get(k), y.get(k)) {
                    (Some(v1), Some(v2)) => json_diff(&format!("{path}/{k}"), v1, v2, out),
                    (v1, v2) => out.push(format!("{path}/{k}: geoserver {v1:?} bbox {v2:?}")),
                }
            }
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            for (i, (v1, v2)) in x.iter().zip(y).enumerate() {
                json_diff(&format!("{path}/{i}"), v1, v2, out);
            }
        }
        (x, y) if x != y => out.push(format!("{path}: geoserver {} bbox {}", short(x), short(y))),
        _ => {}
    }
}

fn short(v: &serde_json::Value) -> String {
    let s = v.to_string();
    if s.len() > 600 {
        format!("{}...", &s[..600])
    } else {
        s
    }
}
