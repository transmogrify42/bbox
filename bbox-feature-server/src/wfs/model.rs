//! WFS feature model: feature types with ordered, typed properties and feature values.

use geo::{BoundingRect, Geometry, Rect};

pub const GML_NS: &str = "http://www.opengis.net/gml";
pub const GML32_NS: &str = "http://www.opengis.net/gml/3.2";
pub const XSD_NS: &str = "http://www.w3.org/2001/XMLSchema";
pub const XLINK_NS: &str = "http://www.w3.org/1999/xlink";

/// Qualified XML name
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct QName {
    pub ns: String,
    pub prefix: String,
    pub local: String,
}

impl QName {
    pub fn new(ns: &str, prefix: &str, local: &str) -> Self {
        QName {
            ns: ns.to_string(),
            prefix: prefix.to_string(),
            local: local.to_string(),
        }
    }
    /// Prefixed name like `app:Road`
    pub fn prefixed(&self) -> String {
        if self.prefix.is_empty() {
            self.local.clone()
        } else {
            format!("{}:{}", self.prefix, self.local)
        }
    }
    /// Whether the name is in any GML namespace
    pub fn is_gml(&self) -> bool {
        is_gml_ns(&self.ns)
    }
}

pub fn is_gml_ns(ns: &str) -> bool {
    ns == GML_NS || ns == GML32_NS
}

/// Geometry type of a geometry property
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeomType {
    Point,
    LineString,
    Polygon,
    MultiPoint,
    MultiLineString,
    MultiPolygon,
    MultiGeometry,
    /// Any geometry
    Geometry,
}

impl GeomType {
    /// Parse a GeoPackage/OGC SF geometry type name
    pub fn from_sf_name(name: &str) -> GeomType {
        match name.to_ascii_uppercase().as_str() {
            "POINT" => GeomType::Point,
            "LINESTRING" => GeomType::LineString,
            "POLYGON" => GeomType::Polygon,
            "MULTIPOINT" => GeomType::MultiPoint,
            "MULTILINESTRING" => GeomType::MultiLineString,
            "MULTIPOLYGON" => GeomType::MultiPolygon,
            "GEOMETRYCOLLECTION" => GeomType::MultiGeometry,
            _ => GeomType::Geometry,
        }
    }
    /// Simple features type name in CamelCase (e.g. `MultiPolygon`)
    pub fn sf_title(&self) -> &'static str {
        match self {
            GeomType::Point => "Point",
            GeomType::LineString => "LineString",
            GeomType::Polygon => "Polygon",
            GeomType::MultiPoint => "MultiPoint",
            GeomType::MultiLineString => "MultiLineString",
            GeomType::MultiPolygon => "MultiPolygon",
            GeomType::MultiGeometry => "GeometryCollection",
            GeomType::Geometry => "Geometry",
        }
    }

    pub fn sf_name(&self) -> &'static str {
        match self {
            GeomType::Point => "POINT",
            GeomType::LineString => "LINESTRING",
            GeomType::Polygon => "POLYGON",
            GeomType::MultiPoint => "MULTIPOINT",
            GeomType::MultiLineString => "MULTILINESTRING",
            GeomType::MultiPolygon => "MULTIPOLYGON",
            GeomType::MultiGeometry => "GEOMETRYCOLLECTION",
            GeomType::Geometry => "GEOMETRY",
        }
    }
}

/// Value type of a property
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValueType {
    String,
    Integer,
    Double,
    Decimal,
    Boolean,
    Date,
    DateTime,
    Time,
    Uri,
    Geometry(GeomType),
    /// Complex content stored as XML fragment of the property element(s)
    Complex,
}

impl ValueType {
    pub fn is_geometry(&self) -> bool {
        matches!(self, ValueType::Geometry(_))
    }
    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            ValueType::Integer | ValueType::Double | ValueType::Decimal
        )
    }
    pub fn is_temporal(&self) -> bool {
        matches!(
            self,
            ValueType::Date | ValueType::DateTime | ValueType::Time
        )
    }
    /// XML schema built-in type name (without prefix) for simple types
    pub fn xsd_name(&self) -> Option<&'static str> {
        match self {
            ValueType::String => Some("string"),
            ValueType::Integer => Some("integer"),
            ValueType::Double => Some("double"),
            ValueType::Decimal => Some("decimal"),
            ValueType::Boolean => Some("boolean"),
            ValueType::Date => Some("date"),
            ValueType::DateTime => Some("dateTime"),
            ValueType::Time => Some("time"),
            ValueType::Uri => Some("anyURI"),
            ValueType::Geometry(_) | ValueType::Complex => None,
        }
    }
    /// Value type of an XML schema built-in type (local name)
    pub fn from_xsd_name(name: &str) -> Option<ValueType> {
        Some(match name {
            "string" | "normalizedString" | "token" | "NCName" | "Name" | "ID" | "IDREF"
            | "language" | "QName" | "hexBinary" | "base64Binary" => ValueType::String,
            "integer" | "int" | "long" | "short" | "byte" | "nonNegativeInteger"
            | "positiveInteger" | "negativeInteger" | "nonPositiveInteger" | "unsignedLong"
            | "unsignedInt" | "unsignedShort" | "unsignedByte" => ValueType::Integer,
            "double" | "float" => ValueType::Double,
            "decimal" => ValueType::Decimal,
            "boolean" => ValueType::Boolean,
            "date" => ValueType::Date,
            "dateTime" => ValueType::DateTime,
            "time" => ValueType::Time,
            "anyURI" => ValueType::Uri,
            "gYear" | "gYearMonth" | "duration" => ValueType::String,
            _ => return None,
        })
    }
}

/// Property definition of a feature type
#[derive(Clone, Debug, PartialEq)]
pub struct PropertyDef {
    pub name: QName,
    /// Storage column name
    pub column: String,
    pub value_type: ValueType,
    pub min_occurs: u32,
    /// None: unbounded
    pub max_occurs: Option<u32>,
    pub nillable: bool,
    /// XSD type QName used in DescribeFeatureType (e.g. `gml:PointPropertyType`, `xsd:string`)
    pub xsd_type: Option<String>,
}

impl PropertyDef {
    /// Simple, single valued property
    pub fn simple(name: QName, column: &str, value_type: ValueType) -> Self {
        PropertyDef {
            name,
            column: column.to_string(),
            value_type,
            min_occurs: 0,
            max_occurs: Some(1),
            nillable: true,
            xsd_type: None,
        }
    }
    /// Standard GML property stored in a column of the same name (e.g. column `name` as
    /// gml:name). GML 2 output keeps such columns in the application namespace.
    pub fn is_mapped_gml_column(&self) -> bool {
        self.name.is_gml()
            && matches!(
                self.name.local.as_str(),
                "name" | "description" | "boundedBy"
            )
            && self.column == self.name.local
    }

    pub fn is_multi(&self) -> bool {
        self.max_occurs != Some(1)
    }
    pub fn is_geometry(&self) -> bool {
        self.value_type.is_geometry()
    }
    /// Values stored as XML fragment
    pub fn is_xml(&self) -> bool {
        self.value_type == ValueType::Complex || (self.is_multi() && !self.is_geometry())
    }
}

/// Map a column named like a gml:AbstractFeatureType property (`name`, `description`,
/// geometry `boundedBy`) to that standard property, as GeoServer does. Returns true if mapped.
pub fn map_gml_column(props: &mut [PropertyDef], column: &str, value_type: &ValueType) -> bool {
    let mapped = match column {
        "name" | "description" => !value_type.is_geometry(),
        "boundedBy" => value_type.is_geometry(),
        _ => false,
    };
    if !mapped {
        return false;
    }
    match props
        .iter_mut()
        .find(|p| p.name.is_gml() && p.name.local == column)
    {
        Some(p) => {
            p.column = column.to_string();
            true
        }
        None => false,
    }
}

/// Standard properties of gml:AbstractFeatureType for auto-mapped feature types
pub fn standard_gml_properties() -> Vec<PropertyDef> {
    let prop = |local: &str, vt: ValueType, xsd: &str| PropertyDef {
        name: QName::new(GML_NS, "gml", local),
        column: format!("gml_{local}"),
        value_type: vt,
        min_occurs: 0,
        max_occurs: Some(1),
        nillable: false,
        xsd_type: Some(xsd.to_string()),
    };
    vec![
        prop("description", ValueType::String, "gml:StringOrRefType"),
        prop("name", ValueType::String, "gml:CodeType"),
        prop(
            "boundedBy",
            ValueType::Geometry(GeomType::Polygon),
            "gml:BoundingShapeType",
        ),
    ]
}

/// Feature type with ordered properties
#[derive(Clone, Debug)]
pub struct FeatureTypeDef {
    pub name: QName,
    pub title: Option<String>,
    pub abstract_: Option<String>,
    pub keywords: Vec<String>,
    pub properties: Vec<PropertyDef>,
    /// Native EPSG code of geometries
    pub srid: u16,
    /// Extent in WGS84 (lon/lat)
    pub wgs84_bbox: Option<Rect<f64>>,
    /// Configured application schema (XSD document) instead of a generated one
    pub schema_xsd: Option<String>,
    /// Substitution group heads in the application namespace (supertypes, nearest first)
    pub supertypes: Vec<QName>,
}

impl FeatureTypeDef {
    pub fn property(&self, name: &QName) -> Option<(usize, &PropertyDef)> {
        self.properties.iter().enumerate().find(|(_, p)| {
            p.name.local == name.local
                && (p.name.ns == name.ns || (p.name.is_gml() && name.is_gml()))
        })
    }
    /// Lookup by local name only (unqualified property names)
    pub fn property_by_local(&self, local: &str) -> Option<(usize, &PropertyDef)> {
        self.properties
            .iter()
            .enumerate()
            .find(|(_, p)| p.name.local == local)
    }
    pub fn geometry_properties(&self) -> impl Iterator<Item = (usize, &PropertyDef)> {
        self.properties
            .iter()
            .enumerate()
            .filter(|(_, p)| p.is_geometry())
    }
    /// Default geometry property (first geometry, excluding gml:boundedBy/location)
    pub fn default_geometry(&self) -> Option<(usize, &PropertyDef)> {
        self.geometry_properties()
            .find(|(_, p)| {
                !(p.name.is_gml() && (p.name.local == "boundedBy" || p.name.local == "location"))
            })
            .or_else(|| self.geometry_properties().next())
    }
}

/// Geometry value in native CRS (x/y axis order)
#[derive(Clone, Debug, PartialEq)]
pub struct GeomValue {
    pub geometry: Geometry<f64>,
    /// gml:id of geometry, if stored
    pub gml_id: Option<String>,
    /// Stored GML metadata elements of the geometry (gml:description, gml:name)
    pub meta: Option<String>,
    /// Z ordinates in coordinate traversal order (3D geometry)
    pub z: Option<Vec<f64>>,
}

impl GeomValue {
    pub fn new(geometry: Geometry<f64>) -> Self {
        GeomValue {
            geometry,
            gml_id: None,
            meta: None,
            z: None,
        }
    }
    /// Range of Z ordinates (3D geometry)
    pub fn z_range(&self) -> Option<[f64; 2]> {
        let z = self.z.as_ref()?;
        let min = z.iter().copied().fold(f64::INFINITY, f64::min);
        let max = z.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        (min <= max).then_some([min, max])
    }
    pub fn bbox(&self) -> Option<Rect<f64>> {
        self.geometry.bounding_rect()
    }
}

/// Property value
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    String(String),
    Integer(i64),
    Double(f64),
    /// Lexical decimal representation
    Decimal(String),
    Boolean(bool),
    /// Lexical xsd:date
    Date(String),
    /// Lexical xsd:dateTime
    DateTime(String),
    /// Lexical xsd:time
    Time(String),
    Geometry(GeomValue),
    /// XML fragment with one or more property elements (namespace declarations inline)
    Xml(String),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
    /// Lexical representation of simple values
    pub fn lexical(&self) -> Option<String> {
        match self {
            Value::Null | Value::Geometry(_) | Value::Xml(_) => None,
            Value::String(s)
            | Value::Decimal(s)
            | Value::Date(s)
            | Value::DateTime(s)
            | Value::Time(s) => Some(s.clone()),
            Value::Integer(i) => Some(i.to_string()),
            Value::Double(d) => Some(format_double(*d)),
            Value::Boolean(b) => Some(b.to_string()),
        }
    }
    /// Text values of a simple value or of all elements in an XML fragment
    pub fn text_values(&self) -> Vec<String> {
        match self {
            Value::Xml(xml) => xml_fragment_texts(xml),
            v => v.lexical().into_iter().collect(),
        }
    }
}

/// Canonical xsd:double lexical form without unnecessary exponent
pub fn format_double(d: f64) -> String {
    if d.is_finite() {
        format!("{d}")
    } else if d.is_nan() {
        "NaN".to_string()
    } else if d > 0.0 {
        "INF".to_string()
    } else {
        "-INF".to_string()
    }
}

/// Text content of top-level elements of an XML fragment
pub fn xml_fragment_texts(xml: &str) -> Vec<String> {
    let wrapped = format!("<r>{xml}</r>");
    match roxmltree::Document::parse(&wrapped) {
        Ok(doc) => doc
            .root_element()
            .children()
            .filter(|n| n.is_element())
            .map(|n| {
                n.descendants()
                    .filter(|d| d.is_text())
                    .filter_map(|d| d.text())
                    .collect::<String>()
                    .trim()
                    .to_string()
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Feature with values aligned to the feature type properties
#[derive(Clone, Debug, PartialEq)]
pub struct Feature {
    /// gml:id / fid
    pub id: String,
    pub values: Vec<Value>,
}

impl Feature {
    /// Envelope of all geometry values
    pub fn bbox(&self) -> Option<Rect<f64>> {
        let mut result: Option<Rect<f64>> = None;
        for v in &self.values {
            if let Value::Geometry(g) = v {
                if let Some(r) = g.bbox() {
                    result = Some(match result {
                        None => r,
                        Some(acc) => merge_rect(&acc, &r),
                    });
                }
            }
        }
        result
    }
}

pub fn merge_rect(a: &Rect<f64>, b: &Rect<f64>) -> Rect<f64> {
    Rect::new(
        (a.min().x.min(b.min().x), a.min().y.min(b.min().y)),
        (a.max().x.max(b.max().x), a.max().y.max(b.max().y)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qname() {
        let q = QName::new("http://example.com", "app", "Road");
        assert_eq!(q.prefixed(), "app:Road");
        assert!(!q.is_gml());
        assert!(QName::new(GML32_NS, "gml", "name").is_gml());
    }

    #[test]
    fn xml_texts() {
        let v = Value::Xml(
            r#"<gml:name xmlns:gml="http://www.opengis.net/gml" codeSpace="x">a</gml:name><gml:name xmlns:gml="http://www.opengis.net/gml"> b </gml:name>"#
                .to_string(),
        );
        assert_eq!(v.text_values(), vec!["a", "b"]);
        assert_eq!(Value::Integer(7).text_values(), vec!["7"]);
        assert!(Value::Null.text_values().is_empty());
    }

    #[test]
    fn property_lookup_matches_any_gml_namespace() {
        let ft = FeatureTypeDef {
            name: QName::new("http://example.com", "app", "T"),
            title: None,
            abstract_: None,
            keywords: vec![],
            properties: vec![PropertyDef::simple(
                QName::new(GML_NS, "gml", "name"),
                "gml_name",
                ValueType::String,
            )],
            srid: 4326,
            wgs84_bbox: None,
            schema_xsd: None,
            supertypes: vec![],
        };
        assert!(ft.property(&QName::new(GML32_NS, "gml", "name")).is_some());
        assert!(ft.property(&QName::new("urn:x", "x", "name")).is_none());
    }
}
