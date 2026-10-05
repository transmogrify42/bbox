//! GetCapabilities (WFS 1.0.0, 1.1.0, 2.0.0/2.0.2).

use crate::wfs::crs::{Crs, CrsNotation};
use crate::wfs::exception::{WfsError, WfsResult};
use crate::wfs::filter::{SpatialOp, TemporalOp, FUNCTIONS};
use crate::wfs::request::RawRequest;
use crate::wfs::service::WfsService;
use crate::wfs::storedquery::WFS_QUERY_LANGUAGE;
use crate::wfs::version::{negotiate_accept_versions, negotiate_version_param, Version};
use crate::wfs::xml::XmlWriter;
use std::sync::Arc;

pub const SECTIONS: [&str; 5] = [
    "ServiceIdentification",
    "ServiceProvider",
    "OperationsMetadata",
    "FeatureTypeList",
    "Filter_Capabilities",
];

/// Operations offered (read-only)
const OPERATIONS_V2: [&str; 8] = [
    "GetCapabilities",
    "DescribeFeatureType",
    "ListStoredQueries",
    "DescribeStoredQueries",
    "GetFeature",
    "GetPropertyValue",
    "CreateStoredQuery",
    "DropStoredQuery",
];

/// Output formats of GetFeature per version (first = default)
pub fn getfeature_formats(version: Version) -> Vec<&'static str> {
    let mut f = match version {
        Version::V100 => vec!["GML2", "text/xml; subtype=gml/2.1.2"],
        Version::V110 => vec![
            "text/xml; subtype=gml/3.1.1",
            "GML2",
            "text/xml; subtype=gml/2.1.2",
        ],
        _ => vec![
            "application/gml+xml; version=3.2",
            "text/xml; subtype=gml/3.2",
            "text/xml; subtype=gml/3.1.1",
            "GML2",
        ],
    };
    f.extend([
        "application/json",
        "text/javascript",
        "csv",
        "SHAPE-ZIP",
        "application/vnd.google-earth.kml+xml",
    ]);
    f
}

pub fn describe_formats(version: Version) -> Vec<&'static str> {
    match version {
        Version::V100 => vec!["XMLSCHEMA"],
        Version::V110 => vec!["text/xml; subtype=gml/3.1.1", "XMLSCHEMA"],
        _ => vec![
            "application/gml+xml; version=3.2",
            "text/xml; subtype=gml/3.2",
            "text/xml; subtype=gml/3.1.1",
        ],
    }
}

/// GetCapabilities parameters
struct CapsRequest {
    version: Version,
    sections: Vec<String>,
    update_sequence: Option<String>,
}

fn parse_request(raw: &RawRequest, strict: bool) -> WfsResult<CapsRequest> {
    raw.check_service(strict)?;
    let (accept, version_param, sections, update_sequence) = if raw.is_kvp() {
        (
            raw.kvp.list("acceptversions"),
            raw.kvp.value("version").map(str::to_string),
            raw.kvp.list("sections"),
            raw.kvp.value("updatesequence").map(str::to_string),
        )
    } else {
        let xml = raw.xml.as_deref().unwrap_or("");
        let doc = roxmltree::Document::parse(xml)
            .map_err(|e| WfsError::parsing("request", e.to_string()))?;
        let root = doc.root_element();
        let list = |name: &str, item: &str| -> Option<Vec<String>> {
            root.children()
                .find(|c| c.is_element() && c.tag_name().name() == name)
                .map(|n| {
                    n.children()
                        .filter(|c| c.is_element() && c.tag_name().name() == item)
                        .filter_map(|c| c.text().map(|t| t.trim().to_string()))
                        .collect()
                })
        };
        (
            list("AcceptVersions", "Version"),
            root.attribute("version").map(str::to_string),
            list("Sections", "Section"),
            root.attribute("updateSequence").map(str::to_string),
        )
    };
    let version = match accept {
        Some(list) if !list.is_empty() => negotiate_accept_versions(&list).ok_or_else(|| {
            WfsError::new(
                "VersionNegotiationFailed",
                Some("AcceptVersions"),
                format!("None of the requested versions {list:?} is supported"),
            )
        })?,
        _ => match &version_param {
            Some(v) => negotiate_version_param(v),
            None => Version::highest(),
        },
    };
    let sections = sections.unwrap_or_default();
    for s in &sections {
        if s != "All" && !SECTIONS.contains(&s.as_str()) {
            return Err(WfsError::invalid(
                "Sections",
                format!("Unknown section `{s}`"),
            ));
        }
    }
    Ok(CapsRequest {
        version,
        sections,
        update_sequence,
    })
}

/// Handle GetCapabilities. Returns (version, document).
pub fn get_capabilities(
    svc: &WfsService,
    raw: &RawRequest,
) -> Result<(Version, Arc<String>), (Version, WfsError)> {
    let req = parse_request(raw, svc.cfg.cite_compliant).map_err(|e| (raw.error_version(), e))?;
    if let Some(seq) = &req.update_sequence {
        let current: i64 = svc.update_sequence.parse().unwrap_or(0);
        if let Ok(requested) = seq.trim().parse::<i64>() {
            if requested > current {
                return Err((
                    req.version,
                    WfsError::new(
                        "InvalidUpdateSequence",
                        Some("updateSequence"),
                        "updateSequence is greater than the current value",
                    ),
                ));
            }
        }
    }
    let sections = if req.sections.is_empty() || req.sections.iter().any(|s| s == "All") {
        "All".to_string()
    } else {
        req.sections.join(",")
    };
    // GeoServer vendor parameter NAMESPACE=<prefix>: feature types of one namespace
    // (`xmlns(..)` values are namespace bindings, not a filter)
    let ns_filter = raw
        .kvp
        .value("namespace")
        .map(str::trim)
        .filter(|v| !v.is_empty() && !v.starts_with("xmlns("));
    let key = (
        req.version,
        format!("{sections}|{}", ns_filter.unwrap_or("")),
    );
    if let Some(doc) = svc.caps_cache.lock().unwrap().get(&key) {
        return Ok((req.version, doc.clone()));
    }
    let doc = Arc::new(render(svc, req.version, &sections, ns_filter));
    svc.caps_cache.lock().unwrap().insert(key, doc.clone());
    Ok((req.version, doc))
}

fn section(sections: &str, name: &str) -> bool {
    sections == "All" || sections.split(',').any(|s| s == name)
}

/// Render a capabilities document; `ns_filter`: only feature types with this namespace prefix
pub fn render(
    svc: &WfsService,
    version: Version,
    sections: &str,
    ns_filter: Option<&str>,
) -> String {
    match version {
        Version::V100 => render_v100(svc, ns_filter),
        Version::V110 => render_v110(svc, sections, ns_filter),
        _ => render_v200(svc, version, sections, ns_filter),
    }
}

fn ns_attrs(svc: &WfsService) -> Vec<(String, String)> {
    svc.used_namespaces()
        .iter()
        .map(|n| (format!("xmlns:{}", n.prefix), n.uri.clone()))
        .collect()
}

fn with_ns<'a>(
    base: &[(&'a str, &'a str)],
    extra: &'a [(String, String)],
) -> Vec<(&'a str, &'a str)> {
    let mut v: Vec<(&str, &str)> = base.to_vec();
    for (k, val) in extra {
        if !v.iter().any(|(bk, _)| bk == k) {
            v.push((k.as_str(), val.as_str()));
        }
    }
    v
}

/// EPSG codes offered for a feature type: native first, then configured other CRS
fn crs_codes(svc: &WfsService, native: u16) -> Vec<u16> {
    let mut codes = vec![native];
    for c in &svc.cfg.other_crs {
        if !codes.contains(c) && Crs::is_known(*c) {
            codes.push(*c);
        }
    }
    codes
}

fn bbox_or_world(def: &crate::wfs::model::FeatureTypeDef) -> (f64, f64, f64, f64) {
    match def.wgs84_bbox {
        Some(r) => (r.min().x, r.min().y, r.max().x, r.max().y),
        None => (-180.0, -90.0, 180.0, 90.0),
    }
}

fn fmt(v: f64) -> String {
    crate::wfs::model::format_double(v)
}

// ---------------------------------------------------------------- 1.0.0

fn render_v100(svc: &WfsService, ns_filter: Option<&str>) -> String {
    let url = &svc.endpoint_url;
    let get_url = format!("{url}?");
    let cfg = &svc.cfg;
    let mut w = XmlWriter::new();
    let nss = ns_attrs(svc);
    w.decl().open(
        "wfs:WFS_Capabilities",
        &with_ns(
            &[
                ("version", "1.0.0"),
                ("updateSequence", &svc.update_sequence),
                ("xmlns:wfs", "http://www.opengis.net/wfs"),
                ("xmlns:ogc", "http://www.opengis.net/ogc"),
                ("xmlns:gml", "http://www.opengis.net/gml"),
                ("xmlns:xsi", "http://www.w3.org/2001/XMLSchema-instance"),
                (
                    "xsi:schemaLocation",
                    "http://www.opengis.net/wfs http://schemas.opengis.net/wfs/1.0.0/WFS-capabilities.xsd",
                ),
            ],
            &nss,
        ),
    );
    w.open("wfs:Service", &[])
        .elem("wfs:Name", "WFS")
        .elem("wfs:Title", &cfg.title);
    if let Some(a) = &cfg.abstract_ {
        w.elem("wfs:Abstract", a);
    }
    if !cfg.keywords.is_empty() {
        w.elem("wfs:Keywords", &cfg.keywords.join(", "));
    }
    w.elem("wfs:OnlineResource", url)
        .elem("wfs:Fees", &cfg.fees)
        .elem("wfs:AccessConstraints", &cfg.access_constraints)
        .close("wfs:Service");
    let dcp = |w: &mut XmlWriter, get: bool| {
        if get {
            w.open("wfs:DCPType", &[])
                .open("wfs:HTTP", &[])
                .empty("wfs:Get", &[("onlineResource", &get_url)])
                .close("wfs:HTTP")
                .close("wfs:DCPType");
        }
        w.open("wfs:DCPType", &[])
            .open("wfs:HTTP", &[])
            .empty("wfs:Post", &[("onlineResource", url)])
            .close("wfs:HTTP")
            .close("wfs:DCPType");
    };
    w.open("wfs:Capability", &[]).open("wfs:Request", &[]);
    w.open("wfs:GetCapabilities", &[]);
    dcp(&mut w, true);
    w.close("wfs:GetCapabilities");
    w.open("wfs:DescribeFeatureType", &[])
        .open("wfs:SchemaDescriptionLanguage", &[])
        .empty("wfs:XMLSCHEMA", &[])
        .close("wfs:SchemaDescriptionLanguage");
    dcp(&mut w, true);
    w.close("wfs:DescribeFeatureType");
    w.open("wfs:GetFeature", &[])
        .open("wfs:ResultFormat", &[])
        .empty("wfs:GML2", &[])
        .close("wfs:ResultFormat");
    dcp(&mut w, true);
    w.close("wfs:GetFeature");
    w.close("wfs:Request").close("wfs:Capability");
    w.open("wfs:FeatureTypeList", &[])
        .open("wfs:Operations", &[])
        .empty("wfs:Query", &[])
        .close("wfs:Operations");
    for t in svc
        .types
        .iter()
        .filter(|t| ns_filter.is_none_or(|p| t.def.name.prefix == p))
    {
        let def = &t.def;
        w.open("wfs:FeatureType", &[])
            .elem("wfs:Name", &def.name.prefixed());
        w.elem("wfs:Title", def.title.as_deref().unwrap_or(&def.name.local));
        if let Some(a) = &def.abstract_ {
            w.elem("wfs:Abstract", a);
        }
        if !def.keywords.is_empty() {
            w.elem("wfs:Keywords", &def.keywords.join(", "));
        }
        w.elem("wfs:SRS", &format!("EPSG:{}", def.srid));
        let (a, b, c, d) = bbox_or_world(def);
        w.empty(
            "wfs:LatLongBoundingBox",
            &[
                ("minx", &fmt(a)),
                ("miny", &fmt(b)),
                ("maxx", &fmt(c)),
                ("maxy", &fmt(d)),
            ],
        );
        w.close("wfs:FeatureType");
    }
    w.close("wfs:FeatureTypeList");
    w.open("ogc:Filter_Capabilities", &[])
        .open("ogc:Spatial_Capabilities", &[])
        .open("ogc:Spatial_Operators", &[]);
    for op in [
        "BBOX",
        "Equals",
        "Disjoint",
        "Intersect",
        "Touches",
        "Crosses",
        "Within",
        "Contains",
        "Overlaps",
        "Beyond",
        "DWithin",
    ] {
        w.empty(&format!("ogc:{op}"), &[]);
    }
    w.close("ogc:Spatial_Operators")
        .close("ogc:Spatial_Capabilities");
    w.open("ogc:Scalar_Capabilities", &[])
        .empty("ogc:Logical_Operators", &[]);
    w.open("ogc:Comparison_Operators", &[]);
    for op in ["Simple_Comparisons", "Like", "Between", "NullCheck"] {
        w.empty(&format!("ogc:{op}"), &[]);
    }
    w.close("ogc:Comparison_Operators");
    w.open("ogc:Arithmetic_Operators", &[])
        .empty("ogc:Simple_Arithmetic", &[]);
    w.open("ogc:Functions", &[]).open("ogc:Function_Names", &[]);
    for f in FUNCTIONS {
        w.elem_attrs(
            "ogc:Function_Name",
            &[("nArgs", &f.args.len().to_string())],
            f.name,
        );
    }
    w.close("ogc:Function_Names").close("ogc:Functions");
    w.close("ogc:Arithmetic_Operators")
        .close("ogc:Scalar_Capabilities")
        .close("ogc:Filter_Capabilities");
    w.close("wfs:WFS_Capabilities");
    w.out
}

// ---------------------------------------------------------------- OWS common

fn service_identification(w: &mut XmlWriter, svc: &WfsService, versions: &[&str]) {
    let cfg = &svc.cfg;
    w.open("ows:ServiceIdentification", &[])
        .elem("ows:Title", &cfg.title);
    if let Some(a) = &cfg.abstract_ {
        w.elem("ows:Abstract", a);
    }
    if !cfg.keywords.is_empty() {
        w.open("ows:Keywords", &[]);
        for k in &cfg.keywords {
            w.elem("ows:Keyword", k);
        }
        w.close("ows:Keywords");
    }
    w.elem("ows:ServiceType", "WFS");
    for v in versions {
        w.elem("ows:ServiceTypeVersion", v);
    }
    w.elem("ows:Fees", &cfg.fees)
        .elem("ows:AccessConstraints", &cfg.access_constraints);
    w.close("ows:ServiceIdentification");
}

fn service_provider(w: &mut XmlWriter, svc: &WfsService) {
    let p = &svc.cfg.provider;
    w.open("ows:ServiceProvider", &[]);
    w.elem("ows:ProviderName", p.name.as_deref().unwrap_or("BBOX"));
    if let Some(site) = &p.site {
        w.empty("ows:ProviderSite", &[("xlink:href", site)]);
    }
    w.open("ows:ServiceContact", &[]);
    if let Some(v) = &p.individual_name {
        w.elem("ows:IndividualName", v);
    }
    if let Some(v) = &p.position_name {
        w.elem("ows:PositionName", v);
    }
    let has_address = p.delivery_point.is_some()
        || p.city.is_some()
        || p.administrative_area.is_some()
        || p.postal_code.is_some()
        || p.country.is_some()
        || p.email.is_some();
    if p.phone.is_some()
        || p.fax.is_some()
        || has_address
        || p.hours_of_service.is_some()
        || p.contact_instructions.is_some()
    {
        w.open("ows:ContactInfo", &[]);
        if p.phone.is_some() || p.fax.is_some() {
            w.open("ows:Phone", &[]);
            if let Some(v) = &p.phone {
                w.elem("ows:Voice", v);
            }
            if let Some(v) = &p.fax {
                w.elem("ows:Facsimile", v);
            }
            w.close("ows:Phone");
        }
        if has_address {
            w.open("ows:Address", &[]);
            for (name, v) in [
                ("ows:DeliveryPoint", &p.delivery_point),
                ("ows:City", &p.city),
                ("ows:AdministrativeArea", &p.administrative_area),
                ("ows:PostalCode", &p.postal_code),
                ("ows:Country", &p.country),
                ("ows:ElectronicMailAddress", &p.email),
            ] {
                if let Some(v) = v {
                    w.elem(name, v);
                }
            }
            w.close("ows:Address");
        }
        if let Some(v) = &p.hours_of_service {
            w.elem("ows:HoursOfService", v);
        }
        if let Some(v) = &p.contact_instructions {
            w.elem("ows:ContactInstructions", v);
        }
        w.close("ows:ContactInfo");
    }
    if let Some(v) = &p.role {
        w.elem("ows:Role", v);
    }
    w.close("ows:ServiceContact").close("ows:ServiceProvider");
}

// ---------------------------------------------------------------- 1.1.0

fn render_v110(svc: &WfsService, sections: &str, ns_filter: Option<&str>) -> String {
    let url = &svc.endpoint_url;
    let get_url = format!("{url}?");
    let mut w = XmlWriter::new();
    let nss = ns_attrs(svc);
    w.decl().open(
        "wfs:WFS_Capabilities",
        &with_ns(
            &[
                ("version", "1.1.0"),
                ("updateSequence", &svc.update_sequence),
                ("xmlns:wfs", "http://www.opengis.net/wfs"),
                ("xmlns:ows", "http://www.opengis.net/ows"),
                ("xmlns:ogc", "http://www.opengis.net/ogc"),
                ("xmlns:gml", "http://www.opengis.net/gml"),
                ("xmlns:xlink", "http://www.w3.org/1999/xlink"),
                ("xmlns:xsi", "http://www.w3.org/2001/XMLSchema-instance"),
                (
                    "xsi:schemaLocation",
                    "http://www.opengis.net/wfs http://schemas.opengis.net/wfs/1.1.0/wfs.xsd",
                ),
            ],
            &nss,
        ),
    );
    if section(sections, "ServiceIdentification") {
        service_identification(&mut w, svc, &["1.1.0"]);
    }
    if section(sections, "ServiceProvider") {
        service_provider(&mut w, svc);
    }
    if section(sections, "OperationsMetadata") {
        w.open("ows:OperationsMetadata", &[]);
        let values = |w: &mut XmlWriter, name: &str, vals: &[&str]| {
            w.open("ows:Parameter", &[("name", name)]);
            for v in vals {
                w.elem("ows:Value", v);
            }
            w.close("ows:Parameter");
        };
        for op in [
            "GetCapabilities",
            "DescribeFeatureType",
            "GetFeature",
            "GetGmlObject",
        ] {
            w.open("ows:Operation", &[("name", op)])
                .open("ows:DCP", &[])
                .open("ows:HTTP", &[]);
            w.empty("ows:Get", &[("xlink:href", &get_url)])
                .empty("ows:Post", &[("xlink:href", url)]);
            w.close("ows:HTTP").close("ows:DCP");
            match op {
                "GetCapabilities" => {
                    values(
                        &mut w,
                        "AcceptVersions",
                        &["1.0.0", "1.1.0", "2.0.0", "2.0.2"],
                    );
                    values(&mut w, "AcceptFormats", &["text/xml"]);
                    let mut s: Vec<&str> = SECTIONS.to_vec();
                    s.push("All");
                    values(&mut w, "Sections", &s);
                }
                "DescribeFeatureType" => {
                    values(&mut w, "outputFormat", &describe_formats(Version::V110))
                }
                "GetFeature" => {
                    values(&mut w, "resultType", &["results", "hits"]);
                    values(&mut w, "outputFormat", &getfeature_formats(Version::V110));
                }
                _ => {}
            }
            w.close("ows:Operation");
        }
        values(
            &mut w,
            "srsName",
            &all_crs_names(svc, CrsNotation::Urn)
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        w.close("ows:OperationsMetadata");
    }
    if section(sections, "FeatureTypeList") {
        w.open("wfs:FeatureTypeList", &[])
            .open("wfs:Operations", &[])
            .elem("wfs:Operation", "Query")
            .close("wfs:Operations");
        for t in svc
            .types
            .iter()
            .filter(|t| ns_filter.is_none_or(|p| t.def.name.prefix == p))
        {
            let def = &t.def;
            w.open("wfs:FeatureType", &[])
                .elem("wfs:Name", &def.name.prefixed());
            w.elem("wfs:Title", def.title.as_deref().unwrap_or(&def.name.local));
            if let Some(a) = &def.abstract_ {
                w.elem("wfs:Abstract", a);
            }
            feature_type_keywords(&mut w, svc, def);
            let codes = crs_codes(svc, def.srid);
            w.elem(
                "wfs:DefaultSRS",
                &Crs::new(codes[0], CrsNotation::Urn).name(),
            );
            for c in &codes[1..] {
                w.elem("wfs:OtherSRS", &Crs::new(*c, CrsNotation::Urn).name());
            }
            w.open("wfs:OutputFormats", &[]);
            for f in getfeature_formats(Version::V110) {
                w.elem("wfs:Format", f);
            }
            w.close("wfs:OutputFormats");
            wgs84_bbox(&mut w, def);
            w.close("wfs:FeatureType");
        }
        w.close("wfs:FeatureTypeList");
    }
    // ogc:Filter_Capabilities is mandatory in the WFS 1.1 schema
    {
        w.open("ogc:Filter_Capabilities", &[])
            .open("ogc:Spatial_Capabilities", &[])
            .open("ogc:GeometryOperands", &[]);
        // Filter 1.1 GeometryOperandType enumeration
        for g in ["gml:Envelope", "gml:Point", "gml:LineString", "gml:Polygon"] {
            w.elem("ogc:GeometryOperand", g);
        }
        w.close("ogc:GeometryOperands")
            .open("ogc:SpatialOperators", &[]);
        for op in SpatialOp::ALL {
            w.empty("ogc:SpatialOperator", &[("name", op.name())]);
        }
        w.close("ogc:SpatialOperators")
            .close("ogc:Spatial_Capabilities");
        w.open("ogc:Scalar_Capabilities", &[])
            .empty("ogc:LogicalOperators", &[])
            .open("ogc:ComparisonOperators", &[]);
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
            w.elem("ogc:ComparisonOperator", op);
        }
        w.close("ogc:ComparisonOperators")
            .open("ogc:ArithmeticOperators", &[])
            .empty("ogc:SimpleArithmetic", &[]);
        w.open("ogc:Functions", &[]).open("ogc:FunctionNames", &[]);
        for f in FUNCTIONS {
            w.elem_attrs(
                "ogc:FunctionName",
                &[("nArgs", &f.args.len().to_string())],
                f.name,
            );
        }
        w.close("ogc:FunctionNames")
            .close("ogc:Functions")
            .close("ogc:ArithmeticOperators")
            .close("ogc:Scalar_Capabilities");
        w.open("ogc:Id_Capabilities", &[])
            .empty("ogc:EID", &[])
            .empty("ogc:FID", &[])
            .close("ogc:Id_Capabilities");
        w.close("ogc:Filter_Capabilities");
    }
    w.close("wfs:WFS_Capabilities");
    w.out
}

fn feature_type_keywords(
    w: &mut XmlWriter,
    svc: &WfsService,
    def: &crate::wfs::model::FeatureTypeDef,
) {
    let mut keywords: Vec<&str> = def.keywords.iter().map(String::as_str).collect();
    if keywords.is_empty() {
        keywords.push(&def.name.local);
        keywords.extend(svc.cfg.keywords.iter().map(String::as_str));
    }
    w.open("ows:Keywords", &[]);
    for k in keywords {
        w.elem("ows:Keyword", k);
    }
    w.close("ows:Keywords");
}

fn wgs84_bbox(w: &mut XmlWriter, def: &crate::wfs::model::FeatureTypeDef) {
    let (a, b, c, d) = bbox_or_world(def);
    w.open("ows:WGS84BoundingBox", &[])
        .elem("ows:LowerCorner", &format!("{} {}", fmt(a), fmt(b)))
        .elem("ows:UpperCorner", &format!("{} {}", fmt(c), fmt(d)))
        .close("ows:WGS84BoundingBox");
}

fn all_crs_names(svc: &WfsService, notation: CrsNotation) -> Vec<String> {
    let mut codes: Vec<u16> = Vec::new();
    for t in &svc.types {
        for c in crs_codes(svc, t.def.srid) {
            if !codes.contains(&c) {
                codes.push(c);
            }
        }
    }
    if !codes.contains(&4326) {
        codes.insert(0, 4326);
    }
    let mut names: Vec<String> = codes
        .iter()
        .map(|c| Crs::new(*c, notation).name())
        .collect();
    names.push(Crs::new(4326, CrsNotation::Crs84).name());
    names
}

// ---------------------------------------------------------------- 2.0.x

fn render_v200(
    svc: &WfsService,
    version: Version,
    sections: &str,
    ns_filter: Option<&str>,
) -> String {
    let url = &svc.endpoint_url;
    let mut w = XmlWriter::new();
    let nss = ns_attrs(svc);
    w.decl().open(
        "wfs:WFS_Capabilities",
        &with_ns(
            &[
                ("version", version.label()),
                ("updateSequence", &svc.update_sequence),
                ("xmlns:wfs", "http://www.opengis.net/wfs/2.0"),
                ("xmlns:ows", "http://www.opengis.net/ows/1.1"),
                ("xmlns:fes", "http://www.opengis.net/fes/2.0"),
                ("xmlns:gml", "http://www.opengis.net/gml/3.2"),
                ("xmlns:xlink", "http://www.w3.org/1999/xlink"),
                ("xmlns:xs", "http://www.w3.org/2001/XMLSchema"),
                ("xmlns:xsi", "http://www.w3.org/2001/XMLSchema-instance"),
                (
                    "xsi:schemaLocation",
                    "http://www.opengis.net/wfs/2.0 http://schemas.opengis.net/wfs/2.0/wfs.xsd",
                ),
            ],
            &nss,
        ),
    );
    if section(sections, "ServiceIdentification") {
        service_identification(&mut w, svc, &["2.0.0", "2.0.2"]);
    }
    if section(sections, "ServiceProvider") {
        service_provider(&mut w, svc);
    }
    if section(sections, "OperationsMetadata") {
        let allowed = |w: &mut XmlWriter, name: &str, vals: &[&str]| {
            w.open("ows:Parameter", &[("name", name)])
                .open("ows:AllowedValues", &[]);
            for v in vals {
                w.elem("ows:Value", v);
            }
            w.close("ows:AllowedValues").close("ows:Parameter");
        };
        w.open("ows:OperationsMetadata", &[]);
        for op in OPERATIONS_V2 {
            w.open("ows:Operation", &[("name", op)])
                .open("ows:DCP", &[])
                .open("ows:HTTP", &[]);
            if !matches!(op, "CreateStoredQuery" | "DropStoredQuery") {
                w.empty("ows:Get", &[("xlink:href", url)]);
            }
            w.open("ows:Post", &[("xlink:href", url)])
                .open("ows:Constraint", &[("name", "PostEncoding")])
                .open("ows:AllowedValues", &[])
                .elem("ows:Value", "XML")
                .elem("ows:Value", "SOAP")
                .close("ows:AllowedValues")
                .close("ows:Constraint")
                .close("ows:Post");
            w.close("ows:HTTP").close("ows:DCP");
            match op {
                "GetCapabilities" => {
                    allowed(
                        &mut w,
                        "AcceptVersions",
                        &["2.0.2", "2.0.0", "1.1.0", "1.0.0"],
                    );
                    allowed(&mut w, "AcceptFormats", &["text/xml"]);
                    let mut s: Vec<&str> = SECTIONS.to_vec();
                    s.push("All");
                    allowed(&mut w, "Sections", &s);
                }
                "DescribeFeatureType" => {
                    allowed(&mut w, "outputFormat", &describe_formats(version))
                }
                "GetFeature" | "GetPropertyValue" => {
                    allowed(&mut w, "outputFormat", &getfeature_formats(version));
                    allowed(&mut w, "resultType", &["results", "hits"]);
                }
                "CreateStoredQuery" => allowed(&mut w, "language", &[WFS_QUERY_LANGUAGE]),
                _ => {}
            }
            w.close("ows:Operation");
        }
        allowed(&mut w, "version", &["2.0.0", "2.0.2"]);
        allowed(&mut w, "resolve", &["local", "remote", "all", "none"]);
        let crs_names = all_crs_names(svc, CrsNotation::Urn);
        let mut crs_refs: Vec<&str> = crs_names.iter().map(String::as_str).collect();
        let crs84_urn = "urn:ogc:def:crs:OGC:1.3:CRS84";
        crs_refs.push(crs84_urn);
        allowed(&mut w, "srsName", &crs_refs);
        let constraint = |w: &mut XmlWriter, name: &str, value: &str| {
            w.open("ows:Constraint", &[("name", name)])
                .empty("ows:NoValues", &[])
                .elem("ows:DefaultValue", value)
                .close("ows:Constraint");
        };
        for (c, v) in [
            ("ImplementsBasicWFS", "TRUE"),
            ("ImplementsTransactionalWFS", "FALSE"),
            ("ImplementsLockingWFS", "FALSE"),
            ("KVPEncoding", "TRUE"),
            ("XMLEncoding", "TRUE"),
            ("SOAPEncoding", "TRUE"),
            ("ImplementsInheritance", "TRUE"),
            ("ImplementsRemoteResolve", "TRUE"),
            ("ImplementsResultPaging", "TRUE"),
            ("ImplementsStandardJoins", "TRUE"),
            ("ImplementsSpatialJoins", "TRUE"),
            ("ImplementsTemporalJoins", "TRUE"),
            ("ImplementsFeatureVersioning", "FALSE"),
            ("ManageStoredQueries", "TRUE"),
            ("PagingIsTransactionSafe", "FALSE"),
            ("AutomaticDataLocking", "FALSE"),
        ] {
            constraint(&mut w, c, v);
        }
        if let Some(n) = svc.cfg.count_default {
            constraint(&mut w, "CountDefault", &n.to_string());
        }
        constraint(
            &mut w,
            "ResolveTimeoutDefault",
            &svc.cfg.resolve_timeout.to_string(),
        );
        constraint(&mut w, "ResolveLocalScope", "*");
        w.open("ows:Constraint", &[("name", "QueryExpressions")])
            .open("ows:AllowedValues", &[])
            .elem("ows:Value", "wfs:Query")
            .elem("ows:Value", "wfs:StoredQuery")
            .close("ows:AllowedValues")
            .close("ows:Constraint");
        w.close("ows:OperationsMetadata");
    }
    if section(sections, "FeatureTypeList") {
        w.open("wfs:FeatureTypeList", &[]);
        for t in svc
            .types
            .iter()
            .filter(|t| ns_filter.is_none_or(|p| t.def.name.prefix == p))
        {
            let def = &t.def;
            w.open("wfs:FeatureType", &[])
                .elem("wfs:Name", &def.name.prefixed());
            w.elem("wfs:Title", def.title.as_deref().unwrap_or(&def.name.local));
            if let Some(a) = &def.abstract_ {
                w.elem("wfs:Abstract", a);
            }
            feature_type_keywords(&mut w, svc, def);
            let codes = crs_codes(svc, def.srid);
            w.elem(
                "wfs:DefaultCRS",
                &Crs::new(codes[0], CrsNotation::Urn).name(),
            );
            for c in &codes[1..] {
                w.elem("wfs:OtherCRS", &Crs::new(*c, CrsNotation::Urn).name());
            }
            w.open("wfs:OutputFormats", &[]);
            for f in getfeature_formats(version) {
                w.elem("wfs:Format", f);
            }
            w.close("wfs:OutputFormats");
            wgs84_bbox(&mut w, def);
            w.close("wfs:FeatureType");
        }
        w.close("wfs:FeatureTypeList");
    }
    if section(sections, "Filter_Capabilities") {
        fes_capabilities(&mut w);
    }
    w.close("wfs:WFS_Capabilities");
    w.out
}

/// Extension operators (FES 2.0 Extended_Capabilities)
pub const EXTENSION_OPERATORS: [&str; 2] = ["bboxfes:PropertyIsILike", "bboxfes:PropertyIsIn"];
pub const EXTENSION_NS: &str = "https://www.bbox.earth/fes";

fn fes_capabilities(w: &mut XmlWriter) {
    let constraint = |w: &mut XmlWriter, name: &str, value: &str| {
        w.open("fes:Constraint", &[("name", name)])
            .empty("ows:NoValues", &[])
            .elem("ows:DefaultValue", value)
            .close("fes:Constraint");
    };
    w.open(
        "fes:Filter_Capabilities",
        &[("xmlns:bboxfes", EXTENSION_NS)],
    )
    .open("fes:Conformance", &[]);
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
        constraint(w, c, "TRUE");
    }
    w.close("fes:Conformance");
    w.open("fes:Id_Capabilities", &[])
        .empty("fes:ResourceIdentifier", &[("name", "fes:ResourceId")])
        .close("fes:Id_Capabilities");
    w.open("fes:Scalar_Capabilities", &[])
        .empty("fes:LogicalOperators", &[])
        .open("fes:ComparisonOperators", &[]);
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
        w.empty("fes:ComparisonOperator", &[("name", op)]);
    }
    w.close("fes:ComparisonOperators")
        .close("fes:Scalar_Capabilities");
    w.open("fes:Spatial_Capabilities", &[])
        .open("fes:GeometryOperands", &[]);
    for g in [
        "gml:Envelope",
        "gml:Point",
        "gml:MultiPoint",
        "gml:LineString",
        "gml:Curve",
        "gml:MultiCurve",
        "gml:Polygon",
        "gml:Surface",
        "gml:MultiSurface",
        "gml:MultiGeometry",
    ] {
        w.empty("fes:GeometryOperand", &[("name", g)]);
    }
    w.close("fes:GeometryOperands")
        .open("fes:SpatialOperators", &[]);
    for op in SpatialOp::ALL {
        w.empty("fes:SpatialOperator", &[("name", op.name())]);
    }
    w.close("fes:SpatialOperators")
        .close("fes:Spatial_Capabilities");
    w.open("fes:Temporal_Capabilities", &[])
        .open("fes:TemporalOperands", &[]);
    for t in ["gml:TimeInstant", "gml:TimePeriod"] {
        w.empty("fes:TemporalOperand", &[("name", t)]);
    }
    w.close("fes:TemporalOperands")
        .open("fes:TemporalOperators", &[]);
    // AnyInteracts is not part of the FES 2.0 TemporalOperatorNameType enumeration
    for op in TemporalOp::ALL
        .iter()
        .filter(|op| **op != TemporalOp::AnyInteracts)
    {
        w.empty("fes:TemporalOperator", &[("name", op.name())]);
    }
    w.close("fes:TemporalOperators")
        .close("fes:Temporal_Capabilities");
    w.open("fes:Functions", &[]);
    for f in FUNCTIONS {
        w.open("fes:Function", &[("name", f.name)])
            .elem("fes:Returns", f.returns);
        if !f.args.is_empty() {
            w.open("fes:Arguments", &[]);
            for (name, t) in f.args {
                w.open("fes:Argument", &[("name", name)])
                    .elem("fes:Type", t)
                    .close("fes:Argument");
            }
            w.close("fes:Arguments");
        }
        w.close("fes:Function");
    }
    w.close("fes:Functions");
    w.open("fes:Extended_Capabilities", &[])
        .open("fes:AdditionalOperators", &[]);
    for op in EXTENSION_OPERATORS {
        w.empty("fes:Operator", &[("name", op)]);
    }
    w.close("fes:AdditionalOperators")
        .close("fes:Extended_Capabilities");
    w.close("fes:Filter_Capabilities");
}
