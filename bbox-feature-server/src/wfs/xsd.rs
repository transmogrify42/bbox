//! GML application schema (XSD) reader.
//!
//! Reads feature type definitions with ordered property lists from a GML 2/3.1/3.2 application schema.

use crate::wfs::model::*;

#[derive(thiserror::Error, Debug)]
pub enum XsdError {
    #[error("XML parse error: {0}")]
    Xml(#[from] roxmltree::Error),
    #[error("invalid schema: {0}")]
    Invalid(String),
}

/// Read all (non-abstract) feature types declared in an application schema.
/// `prefix` is used for feature type and property QNames.
pub fn read_feature_types(xsd: &str, prefix: &str) -> Result<Vec<FeatureTypeDef>, XsdError> {
    let mut set = SchemaSet::default();
    set.add_document(xsd, None)?;
    set.feature_types(prefix)
}

/// Read feature types from a schema file, resolving xsd:include relative to the file
pub fn read_feature_types_from_file(
    path: &str,
    prefix: &str,
) -> Result<Vec<FeatureTypeDef>, XsdError> {
    let xsd = std::fs::read_to_string(path)
        .map_err(|e| XsdError::Invalid(format!("cannot read `{path}`: {e}")))?;
    let mut set = SchemaSet::default();
    set.add_document(&xsd, std::path::Path::new(path).parent())?;
    set.feature_types(prefix)
}

type Name = (String, String); // (namespace, local)

#[derive(Clone, Copy, Debug, PartialEq)]
enum GmlFlavor {
    V2,
    V31,
    V32,
}

#[derive(Debug, Clone)]
struct Particle {
    name: Option<String>,
    ref_: Option<Name>,
    type_ref: Option<Name>,
    /// anonymous simple type restriction/list base
    anon_simple_base: Option<Name>,
    anon_complex: bool,
    min: u32,
    max: Option<u32>,
    nillable: bool,
}

#[derive(Debug, Clone, Default)]
struct ComplexTypeDecl {
    base: Option<Name>,
    particles: Vec<Particle>,
    simple_content: bool,
}

#[derive(Debug, Clone)]
struct ElementDecl {
    name: String,
    type_ref: Option<Name>,
    anon_type: Option<ComplexTypeDecl>,
    anon_simple_base: Option<Name>,
    subst: Option<Name>,
    is_abstract: bool,
}

#[derive(Default)]
struct SchemaSet {
    target_ns: Option<String>,
    gml_flavor: Option<GmlFlavor>,
    /// global elements in document order
    elements: Vec<ElementDecl>,
    complex_types: std::collections::HashMap<String, ComplexTypeDecl>,
    /// simple type name -> restriction base
    simple_types: std::collections::HashMap<String, Name>,
    included: Vec<std::path::PathBuf>,
}

const XS: &str = XSD_NS;

fn is_xs(node: &roxmltree::Node, local: &str) -> bool {
    node.is_element() && node.tag_name().namespace() == Some(XS) && node.tag_name().name() == local
}

fn xs_children<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    local: &'a str,
) -> impl Iterator<Item = roxmltree::Node<'a, 'input>> + 'a {
    node.children().filter(move |c| is_xs(c, local))
}

fn xs_child<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    local: &'a str,
) -> Option<roxmltree::Node<'a, 'input>> {
    xs_children(node, local).next()
}

/// Resolve a QName attribute value in the context of a node
fn resolve_qname(node: &roxmltree::Node, value: &str) -> Name {
    let (prefix, local) = match value.split_once(':') {
        Some((p, l)) => (Some(p), l),
        None => (None, value),
    };
    let ns = node.lookup_namespace_uri(prefix).unwrap_or("").to_string();
    (ns, local.to_string())
}

fn occurs(node: &roxmltree::Node, attr: &str, default: u32) -> Option<u32> {
    match node.attribute(attr) {
        Some("unbounded") => None,
        Some(v) => Some(v.parse().unwrap_or(default)),
        None => Some(default),
    }
}

impl SchemaSet {
    fn add_document(
        &mut self,
        xsd: &str,
        base_dir: Option<&std::path::Path>,
    ) -> Result<(), XsdError> {
        let doc = roxmltree::Document::parse(xsd)?;
        let root = doc.root_element();
        if !is_xs(&root, "schema") {
            return Err(XsdError::Invalid("root element is not xsd:schema".into()));
        }
        if self.target_ns.is_none() {
            self.target_ns = root.attribute("targetNamespace").map(str::to_string);
        }
        for import in xs_children(root, "import") {
            let ns = import.attribute("namespace").unwrap_or("");
            if ns == GML32_NS {
                self.gml_flavor = Some(GmlFlavor::V32);
            } else if ns == GML_NS && self.gml_flavor.is_none() {
                let loc = import.attribute("schemaLocation").unwrap_or("");
                self.gml_flavor = Some(if loc.contains("/2.") {
                    GmlFlavor::V2
                } else {
                    GmlFlavor::V31
                });
            }
        }
        if self.gml_flavor.is_none() && root.lookup_namespace_uri(Some("gml")) == Some(GML32_NS) {
            self.gml_flavor = Some(GmlFlavor::V32);
        }
        for include in xs_children(root, "include").chain(xs_children(root, "redefine")) {
            let (Some(loc), Some(dir)) = (include.attribute("schemaLocation"), base_dir) else {
                continue;
            };
            if loc.starts_with("http:") || loc.starts_with("https:") {
                continue;
            }
            let path = dir.join(loc);
            if self.included.contains(&path) {
                continue;
            }
            self.included.push(path.clone());
            let text = std::fs::read_to_string(&path).map_err(|e| {
                XsdError::Invalid(format!("cannot read include `{}`: {e}", path.display()))
            })?;
            self.add_document(&text, path.parent())?;
        }
        for child in root.children().filter(|c| c.is_element()) {
            if is_xs(&child, "element") {
                if let Some(decl) = element_decl(&child) {
                    self.elements.push(decl);
                }
            } else if is_xs(&child, "complexType") {
                if let Some(name) = child.attribute("name") {
                    self.complex_types
                        .insert(name.to_string(), complex_type_decl(&child));
                }
            } else if is_xs(&child, "simpleType") {
                if let (Some(name), Some(base)) =
                    (child.attribute("name"), simple_type_base(&child))
                {
                    self.simple_types.insert(name.to_string(), base);
                }
            }
        }
        Ok(())
    }

    fn tns(&self) -> &str {
        self.target_ns.as_deref().unwrap_or("")
    }

    fn flavor(&self) -> GmlFlavor {
        self.gml_flavor.unwrap_or(GmlFlavor::V31)
    }

    fn element(&self, local: &str) -> Option<&ElementDecl> {
        self.elements.iter().find(|e| e.name == local)
    }

    /// Substitution group chain in the target namespace and whether it ends in a GML feature head
    fn substitution_chain(&self, decl: &ElementDecl) -> (Vec<String>, bool) {
        let mut chain = Vec::new();
        let mut current = decl.subst.clone();
        while let Some((ns, local)) = current {
            if is_gml_ns(&ns) {
                let is_feature = matches!(local.as_str(), "_Feature" | "AbstractFeature");
                return (chain, is_feature);
            }
            if ns != self.tns() || chain.contains(&local) {
                break;
            }
            chain.push(local.clone());
            current = self.element(&local).and_then(|e| e.subst.clone());
        }
        (chain, false)
    }

    fn feature_types(&self, prefix: &str) -> Result<Vec<FeatureTypeDef>, XsdError> {
        let tns = self.tns().to_string();
        let mut result = Vec::new();
        for decl in &self.elements {
            if decl.is_abstract {
                continue;
            }
            let (chain, is_feature) = self.substitution_chain(decl);
            if !is_feature {
                continue;
            }
            let ctype = match (&decl.anon_type, &decl.type_ref) {
                (Some(t), _) => t.clone(),
                (None, Some((ns, local))) if *ns == tns => {
                    self.complex_types
                        .get(local)
                        .cloned()
                        .ok_or_else(|| XsdError::Invalid(format!("type `{local}` not found")))?
                }
                _ => continue,
            };
            let mut properties = Vec::new();
            self.collect_properties(&ctype, prefix, &mut properties, 0)?;
            result.push(FeatureTypeDef {
                name: QName::new(&tns, prefix, &decl.name),
                title: None,
                abstract_: None,
                keywords: vec![],
                properties,
                srid: 0,
                wgs84_bbox: None,
                schema_xsd: None,
                supertypes: chain
                    .iter()
                    .map(|local| QName::new(&tns, prefix, local))
                    .collect(),
            });
        }
        Ok(result)
    }

    fn collect_properties(
        &self,
        ctype: &ComplexTypeDecl,
        prefix: &str,
        props: &mut Vec<PropertyDef>,
        depth: usize,
    ) -> Result<(), XsdError> {
        if depth > 20 {
            return Err(XsdError::Invalid("type derivation too deep".into()));
        }
        match &ctype.base {
            Some((ns, local)) if is_gml_ns(ns) => {
                if local == "AbstractFeatureType" {
                    props.extend(self.std_gml_props());
                }
            }
            Some((ns, local)) if ns == self.tns() => {
                let base = self
                    .complex_types
                    .get(local)
                    .ok_or_else(|| XsdError::Invalid(format!("base type `{local}` not found")))?;
                self.collect_properties(base, prefix, props, depth + 1)?;
            }
            _ => {}
        }
        for particle in &ctype.particles {
            props.push(self.property(particle, prefix)?);
        }
        Ok(())
    }

    fn std_gml_props(&self) -> Vec<PropertyDef> {
        let flavor = self.flavor();
        let gml_ns = if flavor == GmlFlavor::V32 {
            GML32_NS
        } else {
            GML_NS
        };
        let prop = |local: &str, value_type: ValueType, max: Option<u32>, xsd_type: &str| {
            let name = QName::new(gml_ns, "gml", local);
            PropertyDef {
                column: column_name(&name),
                name,
                value_type,
                min_occurs: 0,
                max_occurs: max,
                nillable: false,
                xsd_type: Some(xsd_type.to_string()),
            }
        };
        let name_max = if flavor == GmlFlavor::V2 {
            Some(1)
        } else {
            None
        };
        let mut props = vec![prop(
            "description",
            ValueType::String,
            Some(1),
            "gml:StringOrRefType",
        )];
        if flavor == GmlFlavor::V32 {
            props.push(prop(
                "identifier",
                ValueType::String,
                Some(1),
                "gml:CodeWithAuthorityType",
            ));
        }
        props.push(prop("name", ValueType::String, name_max, "gml:CodeType"));
        props.push(prop(
            "boundedBy",
            ValueType::Geometry(GeomType::Polygon),
            Some(1),
            "gml:BoundingShapeType",
        ));
        props
    }

    fn property(&self, particle: &Particle, prefix: &str) -> Result<PropertyDef, XsdError> {
        let tns = self.tns();
        let (name, value_type, xsd_type, nillable) = if let Some((ns, local)) = &particle.ref_ {
            let name = if is_gml_ns(ns) {
                QName::new(ns, "gml", local)
            } else {
                QName::new(ns, prefix, local)
            };
            if is_gml_ns(ns) {
                let vt = gml_property_element_type(local);
                (name, vt, Some(format!("gml:{local}")), false)
            } else if ns == tns {
                let decl = self
                    .element(local)
                    .ok_or_else(|| XsdError::Invalid(format!("element `{local}` not found")))?;
                let vt = if decl.anon_type.is_some() {
                    ValueType::Complex
                } else if let Some(base) = &decl.anon_simple_base {
                    self.map_type(base)
                } else if let Some(t) = &decl.type_ref {
                    self.map_type(t)
                } else {
                    ValueType::String
                };
                (name, vt, None, false)
            } else {
                (name, ValueType::Complex, None, false)
            }
        } else {
            let local = particle
                .name
                .as_deref()
                .ok_or_else(|| XsdError::Invalid("element without name or ref".into()))?;
            let name = QName::new(tns, prefix, local);
            let (vt, xsd_type) = if particle.anon_complex {
                (ValueType::Complex, None)
            } else if let Some(base) = &particle.anon_simple_base {
                (self.map_type(base), None)
            } else if let Some(t) = &particle.type_ref {
                (self.map_type(t), Some(type_label(t)))
            } else {
                (ValueType::String, None)
            };
            (name, vt, xsd_type, particle.nillable)
        };
        Ok(PropertyDef {
            column: column_name(&name),
            name,
            value_type,
            min_occurs: particle.min,
            max_occurs: particle.max,
            nillable,
            xsd_type,
        })
    }

    fn map_type(&self, (ns, local): &Name) -> ValueType {
        if ns == XS {
            ValueType::from_xsd_name(local).unwrap_or(ValueType::String)
        } else if is_gml_ns(ns) {
            gml_property_type(local)
        } else if ns == self.tns() {
            if let Some(base) = self.simple_types.get(local) {
                self.map_type(&base.clone())
            } else {
                ValueType::Complex
            }
        } else {
            ValueType::Complex
        }
    }
}

fn type_label((ns, local): &Name) -> String {
    if ns == XS {
        format!("xsd:{local}")
    } else if is_gml_ns(ns) {
        format!("gml:{local}")
    } else {
        local.clone()
    }
}

/// Value type of a GML property type
fn gml_property_type(local: &str) -> ValueType {
    let geom = match local {
        "PointPropertyType" => GeomType::Point,
        "LineStringPropertyType" | "CurvePropertyType" => GeomType::LineString,
        "PolygonPropertyType" | "SurfacePropertyType" => GeomType::Polygon,
        "MultiPointPropertyType" => GeomType::MultiPoint,
        "MultiLineStringPropertyType" | "MultiCurvePropertyType" => GeomType::MultiLineString,
        "MultiPolygonPropertyType" | "MultiSurfacePropertyType" => GeomType::MultiPolygon,
        "MultiGeometryPropertyType" | "GeometryCollectionPropertyType" => GeomType::MultiGeometry,
        "GeometryPropertyType" | "GeometricPrimitivePropertyType" | "GeometryAssociationType" => {
            GeomType::Geometry
        }
        _ => return ValueType::Complex,
    };
    ValueType::Geometry(geom)
}

/// Value type of a global GML property element referenced with `ref`
fn gml_property_element_type(local: &str) -> ValueType {
    let geom = match local {
        "pointProperty" | "centerOf" | "position" => GeomType::Point,
        "lineStringProperty" | "curveProperty" | "centerLineOf" | "edgeOf" => GeomType::LineString,
        "polygonProperty" | "surfaceProperty" | "extentOf" | "coverage" => GeomType::Polygon,
        "multiPointProperty" | "multiPosition" | "multiCenterOf" => GeomType::MultiPoint,
        "multiLineStringProperty" | "multiCurveProperty" | "multiCenterLineOf" | "multiEdgeOf" => {
            GeomType::MultiLineString
        }
        "multiPolygonProperty" | "multiSurfaceProperty" | "multiExtentOf" | "multiCoverage" => {
            GeomType::MultiPolygon
        }
        "multiGeometryProperty" => GeomType::MultiGeometry,
        "geometryProperty" | "location" => GeomType::Geometry,
        "name" | "description" => return ValueType::String,
        _ => return ValueType::Complex,
    };
    ValueType::Geometry(geom)
}

fn simple_type_base(node: &roxmltree::Node) -> Option<Name> {
    if let Some(r) = xs_child(*node, "restriction") {
        if let Some(base) = r.attribute("base") {
            return Some(resolve_qname(&r, base));
        }
        if let Some(st) = xs_child(r, "simpleType") {
            return simple_type_base(&st);
        }
    }
    if xs_child(*node, "list").is_some() || xs_child(*node, "union").is_some() {
        return Some((XS.to_string(), "string".to_string()));
    }
    None
}

fn element_decl(node: &roxmltree::Node) -> Option<ElementDecl> {
    let name = node.attribute("name")?.to_string();
    Some(ElementDecl {
        name,
        type_ref: node.attribute("type").map(|t| resolve_qname(node, t)),
        anon_type: xs_child(*node, "complexType").map(|ct| complex_type_decl(&ct)),
        anon_simple_base: xs_child(*node, "simpleType").and_then(|st| simple_type_base(&st)),
        subst: node
            .attribute("substitutionGroup")
            .map(|t| resolve_qname(node, t)),
        is_abstract: node.attribute("abstract") == Some("true"),
    })
}

fn complex_type_decl(node: &roxmltree::Node) -> ComplexTypeDecl {
    let mut decl = ComplexTypeDecl::default();
    let content = if let Some(cc) = xs_child(*node, "complexContent") {
        cc.children()
            .find(|c| is_xs(c, "extension") || is_xs(c, "restriction"))
    } else if let Some(sc) = xs_child(*node, "simpleContent") {
        decl.simple_content = true;
        sc.children()
            .find(|c| is_xs(c, "extension") || is_xs(c, "restriction"))
    } else {
        None
    };
    let model_parent = if let Some(content) = content {
        decl.base = content
            .attribute("base")
            .map(|b| resolve_qname(&content, b));
        content
    } else {
        *node
    };
    collect_particles(&model_parent, &mut decl.particles, false);
    decl
}

/// Collect element particles of sequence/choice/all groups (flattened)
fn collect_particles(node: &roxmltree::Node, particles: &mut Vec<Particle>, optional: bool) {
    for child in node.children().filter(|c| c.is_element()) {
        if is_xs(&child, "sequence") || is_xs(&child, "all") {
            let opt = optional || child.attribute("minOccurs") == Some("0");
            collect_particles(&child, particles, opt);
        } else if is_xs(&child, "choice") {
            collect_particles(&child, particles, true);
        } else if is_xs(&child, "element") {
            let min = if optional {
                0
            } else {
                occurs(&child, "minOccurs", 1).unwrap_or(1)
            };
            particles.push(Particle {
                name: child.attribute("name").map(str::to_string),
                ref_: child.attribute("ref").map(|r| resolve_qname(&child, r)),
                type_ref: child.attribute("type").map(|t| resolve_qname(&child, t)),
                anon_simple_base: xs_child(child, "simpleType")
                    .and_then(|st| simple_type_base(&st)),
                anon_complex: xs_child(child, "complexType").is_some(),
                min,
                max: occurs(&child, "maxOccurs", 1),
                nillable: child.attribute("nillable") == Some("true"),
            });
        }
    }
}

/// Storage column name for a property
pub fn column_name(name: &QName) -> String {
    if name.is_gml() && STD_GML_PROPS.contains(&name.local.as_str()) {
        format!("gml_{}", name.local)
    } else {
        name.local.clone()
    }
}

/// Standard properties of gml:AbstractFeatureType
pub const STD_GML_PROPS: &[&str] = &["description", "identifier", "name", "boundedBy"];

#[cfg(test)]
mod tests {
    use super::*;

    fn read(file: &str, prefix: &str) -> Vec<FeatureTypeDef> {
        let path = format!("{}/tests/data/cite/{file}", env!("CARGO_MANIFEST_DIR"));
        let xsd = std::fs::read_to_string(path).unwrap();
        read_feature_types(&xsd, prefix).unwrap()
    }

    fn prop_names(ft: &FeatureTypeDef) -> Vec<String> {
        ft.properties.iter().map(|p| p.name.prefixed()).collect()
    }

    fn find<'a>(fts: &'a [FeatureTypeDef], name: &str) -> &'a FeatureTypeDef {
        fts.iter()
            .find(|ft| ft.name.local == name)
            .unwrap_or_else(|| panic!("feature type {name} missing"))
    }

    #[test]
    fn cite_data_features() {
        let fts = read("wfs10/dataFeatures.xsd", "cdf");
        let mut names: Vec<_> = fts.iter().map(|ft| ft.name.prefixed()).collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "cdf:Deletes",
                "cdf:Fifteen",
                "cdf:Inserts",
                "cdf:Locks",
                "cdf:Nulls",
                "cdf:Other",
                "cdf:Seven",
                "cdf:Updates"
            ]
        );
        let other = find(&fts, "Other");
        assert_eq!(other.name.ns, "http://www.opengis.net/cite/data");
        assert_eq!(
            prop_names(other),
            vec![
                "gml:description",
                "gml:name",
                "gml:boundedBy",
                "gml:pointProperty",
                "cdf:string1",
                "cdf:string2",
                "cdf:integers",
                "cdf:dates"
            ]
        );
        let (_, p) = other.property_by_local("string1").unwrap();
        assert_eq!((p.min_occurs, p.max_occurs), (1, Some(1)));
        assert_eq!(p.value_type, ValueType::String);
        assert_eq!(p.column, "string1");
        let (_, p) = other.property_by_local("integers").unwrap();
        assert_eq!(p.value_type, ValueType::Integer);
        assert_eq!(p.min_occurs, 0);
        let (_, p) = other.property_by_local("dates").unwrap();
        assert_eq!(p.value_type, ValueType::Date);
        let (_, p) = other.property_by_local("pointProperty").unwrap();
        assert_eq!(p.value_type, ValueType::Geometry(GeomType::Point));
        assert_eq!(p.name.ns, GML_NS);
        assert_eq!(p.column, "pointProperty");
        let (_, p) = other.property_by_local("boundedBy").unwrap();
        assert_eq!(p.column, "gml_boundedBy");
        let (_, p) = other.property_by_local("name").unwrap();
        assert_eq!(p.column, "gml_name");
    }

    #[test]
    fn cite_geometry_features_inherit_base_type() {
        let fts = read("wfs10/geometryFeatures.xsd", "cgf");
        assert_eq!(fts.len(), 6, "abstract _SimpleFeature excluded");
        let lines = find(&fts, "Lines");
        assert_eq!(
            prop_names(lines),
            vec![
                "gml:description",
                "gml:name",
                "gml:boundedBy",
                "cgf:id",
                "gml:lineStringProperty"
            ]
        );
        let (_, p) = lines.property_by_local("id").unwrap();
        assert_eq!((p.min_occurs, p.value_type.clone()), (1, ValueType::String));
        let mp = find(&fts, "MPolygons");
        let (_, p) = mp.default_geometry().unwrap();
        assert_eq!(p.value_type, ValueType::Geometry(GeomType::MultiPolygon));
    }

    #[test]
    fn cite_complex_feature() {
        let fts = read("wfs10/complexFeatures.xsd", "ccf");
        assert_eq!(fts.len(), 1);
        let c = &fts[0];
        assert_eq!(c.name.prefixed(), "ccf:Complex");
        assert_eq!(
            prop_names(c),
            vec![
                "gml:description",
                "gml:name",
                "gml:boundedBy",
                "gml:pointProperty",
                "ccf:resident",
                "ccf:address"
            ]
        );
        let (_, resident) = c.property_by_local("resident").unwrap();
        assert_eq!(resident.max_occurs, None);
        assert!(resident.is_xml());
        let (_, address) = c.property_by_local("address").unwrap();
        assert_eq!(address.value_type, ValueType::Complex);
    }

    #[test]
    fn gmlsf0_types() {
        let fts = read("wfs11/cite-gmlsf0.xsd", "sf");
        let mut names: Vec<_> = fts.iter().map(|ft| ft.name.local.clone()).collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "AggregateGeoFeature",
                "EntitéGénérique",
                "PrimitiveGeoFeature"
            ]
        );
        let p = find(&fts, "PrimitiveGeoFeature");
        assert_eq!(
            prop_names(p)[3..].to_vec(),
            vec![
                "sf:surfaceProperty",
                "sf:pointProperty",
                "sf:curveProperty",
                "sf:intProperty",
                "sf:uriProperty",
                "sf:measurand",
                "sf:dateTimeProperty",
                "sf:dateProperty",
                "sf:decimalProperty",
                "sf:relatedFeature"
            ]
        );
        // GML 3: gml:name may repeat
        let (_, name) = p.property_by_local("name").unwrap();
        assert_eq!(name.max_occurs, None);
        let types: Vec<_> = p.properties[3..]
            .iter()
            .map(|p| p.value_type.clone())
            .collect();
        assert_eq!(
            types,
            vec![
                ValueType::Geometry(GeomType::Polygon),
                ValueType::Geometry(GeomType::Point),
                ValueType::Geometry(GeomType::LineString),
                ValueType::Integer,
                ValueType::Uri,
                ValueType::Double,
                ValueType::DateTime,
                ValueType::Date,
                ValueType::Decimal,
                ValueType::Complex,
            ]
        );
        let a = find(&fts, "AggregateGeoFeature");
        let (_, p) = a.property_by_local("intRangeProperty").unwrap();
        assert_eq!(p.value_type, ValueType::Integer);
        let (_, p) = a.property_by_local("multiGeomProperty").unwrap();
        assert_eq!(p.value_type, ValueType::Geometry(GeomType::MultiGeometry));
        let e = find(&fts, "EntitéGénérique");
        let (_, p) = e.property_by_local("attribut.Géométrie").unwrap();
        assert_eq!(p.value_type, ValueType::Geometry(GeomType::Geometry));
        assert_eq!(p.min_occurs, 1);
        assert_eq!(p.column, "attribut.Géométrie");
    }

    #[test]
    fn gmlsf1_includes_level0() {
        // cite-gmlsf1.xsd includes cite-gmlsf0.xsd; includes are resolved relative to the schema
        let path = format!(
            "{}/tests/data/cite/wfs11/cite-gmlsf1.xsd",
            env!("CARGO_MANIFEST_DIR")
        );
        let fts = read_feature_types_from_file(&path, "sf").unwrap();
        let names: Vec<_> = fts.iter().map(|ft| ft.name.local.as_str()).collect();
        assert!(names.contains(&"ComplexGeoFeature"), "{names:?}");
        assert!(names.contains(&"PrimitiveGeoFeature"), "{names:?}");
    }
}
