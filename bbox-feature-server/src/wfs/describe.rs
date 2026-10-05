//! DescribeFeatureType: GML application schemas generated from feature type definitions.

use crate::wfs::endpoint::{Body, OpResult, WfsResponse};
use crate::wfs::exception::WfsError;
use crate::wfs::model::*;
use crate::wfs::query::{parse_describe, TypeName};
use crate::wfs::request::RawRequest;
use crate::wfs::service::{FeatureTypeEntry, WfsService};
use crate::wfs::version::Version;
use crate::wfs::xml::{escape, XmlWriter};

pub fn content_type(version: Version) -> &'static str {
    match version {
        Version::V100 => "text/xml; charset=UTF-8",
        Version::V110 => "text/xml; subtype=gml/3.1.1",
        _ => "application/gml+xml; version=3.2",
    }
}

/// Resolve requested type names
pub fn resolve_types<'a>(
    svc: &'a WfsService,
    names: &[TypeName],
    version: Version,
) -> Result<Vec<&'a FeatureTypeEntry>, WfsError> {
    if names.is_empty() {
        return Ok(svc.types.iter().collect());
    }
    let mut result: Vec<&FeatureTypeEntry> = Vec::new();
    for n in names {
        let found = svc.find_type(n.ns.as_deref(), n.prefix.as_deref(), &n.local);
        match found {
            Some(t) => {
                if !result.iter().any(|r| std::ptr::eq(*r, t)) {
                    result.push(t)
                }
            }
            None => {
                let subtypes = svc.find_subtypes(n.ns.as_deref(), n.prefix.as_deref(), &n.local);
                if subtypes.is_empty() {
                    return Err(WfsError::invalid(
                        if version.is_v2() {
                            "typeNames"
                        } else {
                            "typeName"
                        },
                        format!("Unknown feature type `{}`", n.display()),
                    ));
                }
                for t in subtypes {
                    if !result.iter().any(|r| std::ptr::eq(*r, t)) {
                        result.push(t)
                    }
                }
            }
        }
    }
    Ok(result)
}

pub fn describe_feature_type(svc: &WfsService, raw: &RawRequest) -> OpResult {
    let ev = raw.error_version();
    let req = parse_describe(raw, &svc.prefix_map()).map_err(|e| (ev, e))?;
    let version = req.version;
    let types = resolve_types(svc, &req.type_names, version).map_err(|e| (version, e))?;
    let mut namespaces: Vec<&str> = Vec::new();
    for t in &types {
        if !namespaces.contains(&t.def.name.ns.as_str()) {
            namespaces.push(&t.def.name.ns);
        }
    }
    // GeoServer JSON schema description (JSONP: text/javascript)
    if let Some(f) = req.output_format.as_deref() {
        let f = f.to_ascii_lowercase();
        if matches!(f.as_str(), "application/json" | "json" | "text/javascript") {
            let json = json_description(&types);
            if f == "text/javascript" {
                let callback = raw
                    .kvp
                    .value("format_options")
                    .and_then(|o| {
                        o.split(';')
                            .filter_map(|kv| kv.split_once(':'))
                            .find(|(k, _)| k.trim().eq_ignore_ascii_case("callback"))
                            .map(|(_, v)| v.trim().to_string())
                    })
                    .unwrap_or_else(|| "parseResponse".to_string());
                return Ok(WfsResponse::xml(Body::Text(format!("{callback}({json})")))
                    .with_type("text/javascript; charset=UTF-8"));
            }
            return Ok(
                WfsResponse::xml(Body::Text(json)).with_type("application/json; charset=UTF-8")
            );
        }
    }
    let doc = if namespaces.len() <= 1 {
        schema_document(svc, version, &types)
    } else {
        wrapper_schema(svc, version, &types, &namespaces)
    };
    Ok(WfsResponse::xml(Body::Text(doc)).with_type(content_type(version)))
}

/// JSON description of feature types (GeoServer DescribeFeatureType outputFormat=application/json)
fn json_description(types: &[&FeatureTypeEntry]) -> String {
    let first = types.first().map(|t| &t.def.name);
    let feature_types: Vec<serde_json::Value> = types
        .iter()
        .map(|t| {
            let props: Vec<serde_json::Value> = t
                .def
                .properties
                .iter()
                // inherited GML properties only if stored in a column of the same name
                .filter(|p| !p.name.is_gml() || (p.column == p.name.local && p.name.local != "boundedBy"))
                .map(|p| {
                    let (ty, local) = match &p.value_type {
                        ValueType::Geometry(g) => {
                            let n = match g {
                                GeomType::MultiGeometry => "GeometryCollection",
                                other => other.sf_title(),
                            };
                            (format!("gml:{n}"), n.to_string())
                        }
                        vt => {
                            let n = match vt {
                                ValueType::Integer => "int",
                                other => other.xsd_name().unwrap_or("string"),
                            };
                            (format!("xsd:{n}"), n.to_string())
                        }
                    };
                    serde_json::json!({
                        "name": p.name.local,
                        "maxOccurs": p.max_occurs.map(|m| serde_json::json!(m)).unwrap_or(serde_json::json!("unbounded")),
                        "minOccurs": p.min_occurs,
                        "nillable": p.nillable || p.name.is_gml(),
                        "type": ty,
                        "localType": local,
                    })
                })
                .collect();
            serde_json::json!({"typeName": t.def.name.local, "properties": props})
        })
        .collect();
    serde_json::json!({
        "elementFormDefault": "qualified",
        "targetNamespace": first.map(|n| n.ns.as_str()).unwrap_or(""),
        "targetPrefix": first.map(|n| n.prefix.as_str()).unwrap_or(""),
        "featureTypes": feature_types,
    })
    .to_string()
}

/// Schema importing per-namespace schemas from DescribeFeatureType URLs
fn wrapper_schema(
    svc: &WfsService,
    version: Version,
    types: &[&FeatureTypeEntry],
    namespaces: &[&str],
) -> String {
    let mut w = XmlWriter::new();
    w.decl().open(
        "xs:schema",
        &[("xmlns:xs", XSD_NS), ("elementFormDefault", "qualified")],
    );
    for ns in namespaces {
        let names: Vec<String> = types
            .iter()
            .filter(|t| t.def.name.ns == *ns)
            .map(|t| t.def.name.prefixed())
            .collect();
        let url = format!(
            "{}?service=WFS&version={}&request=DescribeFeatureType&typeName={}",
            svc.endpoint_url,
            version.label(),
            names.join(",")
        );
        w.empty("xs:import", &[("namespace", ns), ("schemaLocation", &url)]);
    }
    w.close("xs:schema");
    w.out
}

/// GML import of a version
fn gml_import(version: Version) -> (&'static str, &'static str) {
    match version {
        Version::V100 => (GML_NS, "http://schemas.opengis.net/gml/2.1.2/feature.xsd"),
        Version::V110 => (GML_NS, "http://schemas.opengis.net/gml/3.1.1/base/gml.xsd"),
        _ => (GML32_NS, "http://schemas.opengis.net/gml/3.2.1/gml.xsd"),
    }
}

/// XSD type of a property for a GML version
fn property_type(p: &PropertyDef, version: Version) -> Option<String> {
    let gml2 = version == Version::V100;
    Some(match &p.value_type {
        ValueType::Geometry(g) => {
            let t = match (g, gml2) {
                (GeomType::Point, _) => "PointPropertyType",
                (GeomType::LineString, true) => "LineStringPropertyType",
                (GeomType::LineString, false) => "CurvePropertyType",
                (GeomType::Polygon, true) => "PolygonPropertyType",
                (GeomType::Polygon, false) => "SurfacePropertyType",
                (GeomType::MultiPoint, _) => "MultiPointPropertyType",
                (GeomType::MultiLineString, true) => "MultiLineStringPropertyType",
                (GeomType::MultiLineString, false) => "MultiCurvePropertyType",
                (GeomType::MultiPolygon, true) => "MultiPolygonPropertyType",
                (GeomType::MultiPolygon, false) => "MultiSurfacePropertyType",
                (GeomType::MultiGeometry, true) => "GeometryCollectionPropertyType",
                (GeomType::MultiGeometry, false) => "MultiGeometryPropertyType",
                (GeomType::Geometry, _) => "GeometryPropertyType",
            };
            format!("gml:{t}")
        }
        ValueType::Complex => match &p.xsd_type {
            // GML property types (MeasureType, CodeType, ...) exist in all GML 3 versions
            Some(t) if t.starts_with("gml:") && !gml2 => t.clone(),
            _ => return None,
        },
        ValueType::Integer => "xs:integer".to_string(),
        vt => format!("xs:{}", vt.xsd_name().unwrap_or("string")),
    })
}

/// Generated application schema for feature types of one namespace
pub fn schema_document(svc: &WfsService, version: Version, types: &[&FeatureTypeEntry]) -> String {
    let (ns, prefix) = match types.first() {
        Some(t) => (t.def.name.ns.clone(), t.def.name.prefix.clone()),
        None => (
            svc.cfg.namespace_uri.clone(),
            svc.cfg.namespace_prefix.clone(),
        ),
    };
    let (gml_ns, gml_loc) = gml_import(version);
    let xmlns_prefix = format!("xmlns:{prefix}");
    let mut w = XmlWriter::new();
    w.decl().open(
        "xs:schema",
        &[
            ("xmlns:xs", XSD_NS),
            ("xmlns:gml", gml_ns),
            (&xmlns_prefix, &ns),
            ("targetNamespace", &ns),
            ("elementFormDefault", "qualified"),
            ("attributeFormDefault", "unqualified"),
        ],
    );
    w.empty(
        "xs:import",
        &[("namespace", gml_ns), ("schemaLocation", gml_loc)],
    );
    let subst = if version.is_v2() {
        "gml:AbstractFeature"
    } else {
        "gml:_Feature"
    };
    for t in types {
        let def = &t.def;
        let local = &def.name.local;
        let type_name = format!("{prefix}:{local}Type");
        w.empty(
            "xs:element",
            &[
                ("name", local),
                ("type", &type_name),
                ("substitutionGroup", subst),
            ],
        );
        w.open("xs:complexType", &[("name", &format!("{local}Type"))])
            .open("xs:complexContent", &[])
            .open("xs:extension", &[("base", "gml:AbstractFeatureType")])
            .open("xs:sequence", &[]);
        for p in &def.properties {
            // GML 2: columns named like standard GML properties stay application properties
            let app_mapped = version == Version::V100 && p.is_mapped_gml_column();
            // standard properties are inherited from gml:AbstractFeatureType
            if !app_mapped
                && p.name.is_gml()
                && crate::wfs::xsd::STD_GML_PROPS.contains(&p.name.local.as_str())
            {
                continue;
            }
            let p = &if app_mapped {
                let mut q = p.clone();
                q.name = QName::new(&def.name.ns, &def.name.prefix, &p.name.local);
                q.nillable = true;
                q
            } else {
                p.clone()
            };
            let min = p.min_occurs.to_string();
            let max = p
                .max_occurs
                .map(|m| m.to_string())
                .unwrap_or_else(|| "unbounded".to_string());
            let mut attrs: Vec<(&str, &str)> = Vec::new();
            let name_ref;
            let ty;
            if p.name.is_gml() {
                name_ref = format!("gml:{}", p.name.local);
                attrs.push(("ref", &name_ref));
            } else {
                attrs.push(("name", &p.name.local));
            }
            attrs.push(("minOccurs", &min));
            attrs.push(("maxOccurs", &max));
            if p.nillable && !p.name.is_gml() {
                attrs.push(("nillable", "true"));
            }
            match property_type(p, version) {
                Some(t) if !p.name.is_gml() => {
                    ty = t;
                    attrs.push(("type", &ty));
                    w.empty("xs:element", &attrs);
                }
                Some(_) => {
                    w.empty("xs:element", &attrs);
                }
                None => {
                    // complex content stored as XML fragment
                    w.open("xs:element", &attrs)
                        .open("xs:complexType", &[("mixed", "true")])
                        .open("xs:sequence", &[])
                        .empty(
                            "xs:any",
                            &[
                                ("minOccurs", "0"),
                                ("maxOccurs", "unbounded"),
                                ("processContents", "lax"),
                            ],
                        )
                        .close("xs:sequence")
                        .empty("xs:anyAttribute", &[("processContents", "lax")])
                        .close("xs:complexType")
                        .close("xs:element");
                }
            }
        }
        w.close("xs:sequence")
            .close("xs:extension")
            .close("xs:complexContent")
            .close("xs:complexType");
    }
    w.close("xs:schema");
    w.out
}

/// DescribeFeatureType URL for schema location hints
pub fn describe_url(svc: &WfsService, version: Version, type_names: &[String]) -> String {
    let key = if version.is_v2() {
        "typeNames"
    } else {
        "typeName"
    };
    escape(&format!(
        "{}?service=WFS&version={}&request=DescribeFeatureType&{key}={}",
        svc.endpoint_url,
        version.label(),
        type_names.join(",")
    ))
}
