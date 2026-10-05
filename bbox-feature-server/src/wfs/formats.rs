//! Non-GML output formats (GeoServer compatible): GeoJSON, JSONP, CSV, SHAPE-ZIP, KML.

use crate::wfs::crs;
use crate::wfs::model::*;
use crate::wfs::output::OutputCrs;
use geo::{Coord, Geometry};
use std::fmt::Write as _;

/// Output format of feature collections
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Format {
    Gml(crate::wfs::gml::GmlVersion),
    GeoJson,
    Jsonp(String),
    Csv,
    ShapeZip,
    Kml,
}

impl Format {
    pub fn content_type(&self) -> &'static str {
        match self {
            Format::Gml(crate::wfs::gml::GmlVersion::V2) => "text/xml; subtype=gml/2.1.2",
            Format::Gml(crate::wfs::gml::GmlVersion::V31) => "text/xml; subtype=gml/3.1.1",
            Format::Gml(crate::wfs::gml::GmlVersion::V32) => "application/gml+xml; version=3.2",
            Format::GeoJson => "application/json; charset=UTF-8",
            Format::Jsonp(_) => "text/javascript; charset=UTF-8",
            Format::Csv => "text/csv; charset=UTF-8",
            Format::ShapeZip => "application/zip",
            Format::Kml => "application/vnd.google-earth.kml+xml",
        }
    }
}

/// Z ordinates consumed in coordinate traversal order
pub struct ZCursor<'a> {
    z: Option<&'a [f64]>,
    i: usize,
}

impl<'a> ZCursor<'a> {
    pub fn new(z: Option<&'a [f64]>) -> Self {
        ZCursor { z, i: 0 }
    }
    fn is_3d(&self) -> bool {
        self.z.is_some()
    }
    fn next(&mut self) -> Option<f64> {
        let z = self.z?;
        let v = z.get(self.i).copied().unwrap_or(0.0);
        self.i += 1;
        Some(v)
    }
}

fn fmt_num(out: &mut String, v: f64) {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e15 {
        // doubles keep a decimal point (like Java), so that JSON readers see a floating point number
        let _ = write!(out, "{v:.1}");
    } else if v.is_finite() {
        let _ = write!(out, "{v}");
    } else {
        out.push_str("null");
    }
}

fn json_coord(out: &mut String, zc: &mut ZCursor, c: &Coord<f64>) {
    out.push('[');
    fmt_num(out, c.x);
    out.push(',');
    fmt_num(out, c.y);
    if let Some(z) = zc.next() {
        out.push(',');
        fmt_num(out, z);
    }
    out.push(']');
}

fn json_coords(out: &mut String, zc: &mut ZCursor, coords: &[Coord<f64>]) {
    out.push('[');
    for (i, c) in coords.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        json_coord(out, zc, c);
    }
    out.push(']');
}

fn json_polygon(out: &mut String, zc: &mut ZCursor, p: &geo::Polygon<f64>) {
    out.push('[');
    json_coords(out, zc, &p.exterior().0);
    for r in p.interiors() {
        out.push(',');
        json_coords(out, zc, &r.0);
    }
    out.push(']');
}

/// GeoJSON geometry object
pub fn geojson_geometry(out: &mut String, g: &Geometry<f64>) {
    geojson_geometry_z(out, g, None)
}

/// GeoJSON geometry object with Z ordinates (in coordinate traversal order)
pub fn geojson_geometry_z(out: &mut String, g: &Geometry<f64>, z: Option<&[f64]>) {
    geojson_geometry_zc(out, g, &mut ZCursor::new(z))
}

fn geojson_geometry_zc(out: &mut String, g: &Geometry<f64>, zc: &mut ZCursor) {
    match g {
        Geometry::Point(p) => {
            out.push_str(r#"{"type":"Point","coordinates":"#);
            json_coord(out, zc, &p.0);
        }
        Geometry::Line(l) => return geojson_geometry_zc(out, &Geometry::LineString(l.into()), zc),
        Geometry::LineString(ls) => {
            out.push_str(r#"{"type":"LineString","coordinates":"#);
            json_coords(out, zc, &ls.0);
        }
        Geometry::Polygon(p) => {
            out.push_str(r#"{"type":"Polygon","coordinates":"#);
            json_polygon(out, zc, p);
        }
        Geometry::Rect(r) => {
            return geojson_geometry_zc(out, &Geometry::Polygon(r.to_polygon()), zc)
        }
        Geometry::Triangle(t) => {
            return geojson_geometry_zc(out, &Geometry::Polygon(t.to_polygon()), zc)
        }
        Geometry::MultiPoint(mp) => {
            out.push_str(r#"{"type":"MultiPoint","coordinates":"#);
            let coords: Vec<Coord<f64>> = mp.0.iter().map(|p| p.0).collect();
            json_coords(out, zc, &coords);
        }
        Geometry::MultiLineString(m) => {
            out.push_str(r#"{"type":"MultiLineString","coordinates":["#);
            for (i, ls) in m.0.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                json_coords(out, zc, &ls.0);
            }
            out.push(']');
        }
        Geometry::MultiPolygon(m) => {
            out.push_str(r#"{"type":"MultiPolygon","coordinates":["#);
            for (i, p) in m.0.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                json_polygon(out, zc, p);
            }
            out.push(']');
        }
        Geometry::GeometryCollection(gc) => {
            out.push_str(r#"{"type":"GeometryCollection","geometries":["#);
            for (i, g) in gc.0.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                geojson_geometry_zc(out, g, zc);
            }
            out.push(']');
        }
    }
    out.push('}');
}

fn json_string(out: &mut String, s: &str) {
    out.push_str(&serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string()));
}

fn transform(def: &FeatureTypeDef, crs: &OutputCrs, g: &Geometry<f64>) -> Option<Geometry<f64>> {
    let mut g = g.clone();
    if crs.epsg != def.srid {
        crs::transform(&mut g, def.srid, crs.epsg).ok()?;
    }
    Some(g)
}

/// Properties emitted in non-GML formats (standard GML properties only when set)
fn emitted(def: &FeatureTypeDef, properties: &Option<Vec<usize>>) -> Vec<usize> {
    def.properties
        .iter()
        .enumerate()
        .filter(|(i, p)| {
            // standard GML properties only if backed by a column of the same name
            let std_gml =
                p.name.is_gml() && crate::wfs::xsd::STD_GML_PROPS.contains(&p.name.local.as_str());
            (!std_gml || (p.column == p.name.local && p.name.local != "boundedBy"))
                && properties
                    .as_ref()
                    .map(|ps| ps.contains(i) || p.min_occurs > 0)
                    .unwrap_or(true)
        })
        .map(|(i, _)| i)
        .collect()
}

/// GeoJSON feature id policy (GeoServer format option `id_policy`)
#[derive(Clone, Debug, PartialEq, Default)]
pub enum IdPolicy {
    /// Feature identifier
    #[default]
    Default,
    /// No id member
    Omit,
    /// Value of an attribute (removed from the properties)
    Attribute(String),
}

impl IdPolicy {
    pub fn from_options(options: &std::collections::HashMap<String, String>) -> Self {
        match options.get("id_policy").map(|v| v.trim()) {
            None | Some("") | Some("true") => IdPolicy::Default,
            Some("false") => IdPolicy::Omit,
            Some(attr) => IdPolicy::Attribute(attr.to_string()),
        }
    }
}

/// CSV separator (GeoServer format option `csvSeparator`, names `comma`, `semicolon`, `tab`, `space`)
pub fn csv_separator(options: &std::collections::HashMap<String, String>) -> Result<char, String> {
    let Some(v) = options.get("csvseparator") else {
        return Ok(',');
    };
    let sep = match v.to_ascii_lowercase().as_str() {
        "comma" => ',',
        "semicolon" => ';',
        "tab" => '\t',
        "space" => ' ',
        _ => {
            let mut chars = v.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => c,
                _ => return Err(format!("Invalid CSV separator `{v}`")),
            }
        }
    };
    if sep == '"' {
        return Err("The CSV separator cannot be a double quote".to_string());
    }
    Ok(sep)
}

/// GeoJSON feature
pub fn geojson_feature(
    out: &mut String,
    def: &FeatureTypeDef,
    crs: &OutputCrs,
    f: &Feature,
    properties: &Option<Vec<usize>>,
    id_policy: &IdPolicy,
) {
    let geom_idx = def.default_geometry().map(|(i, _)| i);
    let id_attr = match id_policy {
        IdPolicy::Attribute(a) => def.properties.iter().position(|p| p.name.local == *a),
        _ => None,
    };
    out.push_str(r#"{"type":"Feature""#);
    match (id_policy, id_attr) {
        (IdPolicy::Default, _) => {
            out.push_str(r#","id":"#);
            json_string(out, &f.id);
        }
        (IdPolicy::Attribute(_), Some(i)) => {
            if let Some(v) = f.values[i].lexical() {
                out.push_str(r#","id":"#);
                json_string(out, &v);
            }
        }
        _ => {}
    }
    out.push_str(r#","geometry":"#);
    match geom_idx.map(|i| &f.values[i]) {
        Some(Value::Geometry(g)) => match transform(def, crs, &g.geometry) {
            Some(geom) => geojson_geometry_z(out, &geom, g.z.as_deref()),
            None => out.push_str("null"),
        },
        _ => out.push_str("null"),
    }
    if let Some(i) = geom_idx {
        out.push_str(r#","geometry_name":"#);
        json_string(out, &def.properties[i].name.local);
    }
    out.push_str(r#","properties":{"#);
    let mut first = true;
    for i in emitted(def, properties) {
        if Some(i) == geom_idx || Some(i) == id_attr {
            continue;
        }
        if !first {
            out.push(',');
        }
        first = false;
        json_string(out, &def.properties[i].name.local);
        out.push(':');
        match &f.values[i] {
            Value::Null => out.push_str("null"),
            Value::Integer(v) => {
                let _ = write!(out, "{v}");
            }
            Value::Double(v) => fmt_num(out, *v),
            Value::Decimal(s) => match s.parse::<f64>() {
                Ok(_) => out.push_str(s),
                Err(_) => json_string(out, s),
            },
            Value::Boolean(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Geometry(g) => match transform(def, crs, &g.geometry) {
                Some(geom) => geojson_geometry_z(out, &geom, g.z.as_deref()),
                None => out.push_str("null"),
            },
            Value::Xml(x) => json_string(out, &xml_text(x)),
            v => json_string(out, &v.lexical().unwrap_or_default()),
        }
    }
    out.push_str("}}");
}

fn xml_text(x: &str) -> String {
    xml_fragment_texts(x).join(" ")
}

/// End of a GeoJSON feature collection with counts, collection bbox (east/north) and
/// paging links (rel, href)
pub fn geojson_footer(
    out: &mut String,
    matched: Option<u64>,
    returned: u64,
    crs: Option<&OutputCrs>,
    bbox: Option<[f64; 4]>,
    links: &[(&str, String)],
) {
    out.push_str("],");
    if !links.is_empty() {
        out.push_str(r#""links":["#);
        for (i, (rel, href)) in links.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let title = if *rel == "next" {
                "next page"
            } else {
                "previous page"
            };
            let _ = write!(
                out,
                r#"{{"title":"{title}","type":"application/json","rel":"{rel}","href":"#
            );
            json_string(out, href);
            out.push('}');
        }
        out.push_str("],");
    }
    if let Some(b) = bbox {
        out.push_str(r#""bbox":["#);
        for (i, v) in b.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            fmt_num(out, *v);
        }
        out.push_str("],");
    }
    if let Some(m) = matched {
        let _ = write!(out, r#""totalFeatures":{m},"numberMatched":{m},"#);
    }
    let _ = write!(
        out,
        r#""numberReturned":{returned},"timeStamp":"{}""#,
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ")
    );
    if let Some(crs) = crs {
        let _ = write!(
            out,
            r#","crs":{{"type":"name","properties":{{"name":"urn:ogc:def:crs:EPSG::{}"}}}}"#,
            crs.epsg
        );
    }
    out.push('}');
}

// ---------------------------------------------------------------- WKT / CSV

fn wkt_coords(out: &mut String, zc: &mut ZCursor, coords: &[Coord<f64>]) {
    out.push('(');
    for (i, c) in coords.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        let _ = write!(out, "{} {}", c.x, c.y);
        if let Some(z) = zc.next() {
            let _ = write!(out, " {z}");
        }
    }
    out.push(')');
}

fn wkt_polygon(out: &mut String, zc: &mut ZCursor, p: &geo::Polygon<f64>) {
    out.push('(');
    wkt_coords(out, zc, &p.exterior().0);
    for r in p.interiors() {
        out.push_str(", ");
        wkt_coords(out, zc, &r.0);
    }
    out.push(')');
}

pub fn wkt(out: &mut String, g: &Geometry<f64>) {
    wkt_z(out, g, None)
}

/// WKT with Z ordinates (in coordinate traversal order)
pub fn wkt_z(out: &mut String, g: &Geometry<f64>, z: Option<&[f64]>) {
    wkt_zc(out, g, &mut ZCursor::new(z))
}

fn wkt_zc(out: &mut String, g: &Geometry<f64>, zc: &mut ZCursor) {
    // geometry tag with Z suffix for 3D
    let tag = |out: &mut String, name: &str, zc: &ZCursor| {
        out.push_str(name);
        out.push_str(if zc.is_3d() { " Z " } else { " " });
    };
    match g {
        Geometry::Point(p) => {
            tag(out, "POINT", zc);
            wkt_coords(out, zc, &[p.0]);
        }
        Geometry::Line(l) => wkt_zc(out, &Geometry::LineString(l.into()), zc),
        Geometry::LineString(ls) => {
            tag(out, "LINESTRING", zc);
            wkt_coords(out, zc, &ls.0);
        }
        Geometry::Polygon(p) => {
            tag(out, "POLYGON", zc);
            wkt_polygon(out, zc, p);
        }
        Geometry::Rect(r) => wkt_zc(out, &Geometry::Polygon(r.to_polygon()), zc),
        Geometry::Triangle(t) => wkt_zc(out, &Geometry::Polygon(t.to_polygon()), zc),
        Geometry::MultiPoint(mp) => {
            tag(out, "MULTIPOINT", zc);
            let c: Vec<Coord<f64>> = mp.0.iter().map(|p| p.0).collect();
            wkt_coords(out, zc, &c);
        }
        Geometry::MultiLineString(m) => {
            tag(out, "MULTILINESTRING", zc);
            out.push('(');
            for (i, ls) in m.0.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                wkt_coords(out, zc, &ls.0);
            }
            out.push(')');
        }
        Geometry::MultiPolygon(m) => {
            tag(out, "MULTIPOLYGON", zc);
            out.push('(');
            for (i, p) in m.0.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                wkt_polygon(out, zc, p);
            }
            out.push(')');
        }
        Geometry::GeometryCollection(gc) => {
            tag(out, "GEOMETRYCOLLECTION", zc);
            out.push('(');
            for (i, g) in gc.0.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                wkt_zc(out, g, zc);
            }
            out.push(')');
        }
    }
}

fn csv_field(out: &mut String, s: &str, sep: char) {
    if s.contains([sep, '"', '\n', '\r']) {
        out.push('"');
        out.push_str(&s.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(s);
    }
}

pub fn csv_header(
    out: &mut String,
    def: &FeatureTypeDef,
    properties: &Option<Vec<usize>>,
    sep: char,
) {
    out.push_str("FID");
    for i in emitted(def, properties) {
        out.push(sep);
        csv_field(out, &def.properties[i].name.local, sep);
    }
    out.push('\n');
}

/// CSV header of join tuples: FID and the columns of each member, prefixed with
/// the member name
pub fn csv_join_header(out: &mut String, members: &[(&FeatureTypeDef, &str)], sep: char) {
    out.push_str("FID");
    for (def, prefix) in members {
        for i in emitted(def, &None) {
            out.push(sep);
            csv_field(
                out,
                &format!("{prefix}.{}", def.properties[i].name.local),
                sep,
            );
        }
    }
    out.push('\n');
}

/// CSV row of a join tuple
pub fn csv_join_row(
    out: &mut String,
    id: &str,
    members: &[(&FeatureTypeDef, &OutputCrs, &Feature)],
    sep: char,
) {
    csv_field(out, id, sep);
    for (def, crs, f) in members {
        let mut row = String::new();
        csv_row(&mut row, def, crs, f, &None, sep);
        // drop the member id and the line end
        let row = row.trim_end_matches('\n');
        let rest = match row.find(sep) {
            Some(i) if !row.starts_with('"') => &row[i..],
            _ => {
                // quoted id: skip the quoted field
                let mut in_quotes = false;
                let mut cut = row.len();
                for (j, c) in row.char_indices() {
                    match c {
                        '"' => in_quotes = !in_quotes,
                        c if c == sep && !in_quotes => {
                            cut = j;
                            break;
                        }
                        _ => {}
                    }
                }
                &row[cut..]
            }
        };
        out.push_str(rest);
    }
    out.push('\n');
}

pub fn csv_row(
    out: &mut String,
    def: &FeatureTypeDef,
    crs: &OutputCrs,
    f: &Feature,
    properties: &Option<Vec<usize>>,
    sep: char,
) {
    csv_field(out, &f.id, sep);
    for i in emitted(def, properties) {
        out.push(sep);
        let text = match &f.values[i] {
            Value::Null => String::new(),
            Value::Geometry(g) => match transform(def, crs, &g.geometry) {
                Some(geom) => {
                    let mut s = String::new();
                    wkt_z(&mut s, &geom, g.z.as_deref());
                    s
                }
                None => String::new(),
            },
            Value::Xml(x) => xml_text(x),
            v => v.lexical().unwrap_or_default(),
        };
        csv_field(out, &text, sep);
    }
    out.push('\n');
}

// ---------------------------------------------------------------- KML

fn kml_coords(out: &mut String, zc: &mut ZCursor, coords: &[Coord<f64>]) {
    out.push_str("<coordinates>");
    for (i, c) in coords.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let _ = write!(out, "{},{}", c.x, c.y);
        if let Some(z) = zc.next() {
            let _ = write!(out, ",{z}");
        }
    }
    out.push_str("</coordinates>");
}

fn kml_polygon(out: &mut String, zc: &mut ZCursor, p: &geo::Polygon<f64>) {
    out.push_str("<Polygon><outerBoundaryIs><LinearRing>");
    kml_coords(out, zc, &p.exterior().0);
    out.push_str("</LinearRing></outerBoundaryIs>");
    for r in p.interiors() {
        out.push_str("<innerBoundaryIs><LinearRing>");
        kml_coords(out, zc, &r.0);
        out.push_str("</LinearRing></innerBoundaryIs>");
    }
    out.push_str("</Polygon>");
}

fn kml_geometry(out: &mut String, zc: &mut ZCursor, g: &Geometry<f64>) {
    match g {
        Geometry::Point(p) => {
            out.push_str("<Point>");
            kml_coords(out, zc, &[p.0]);
            out.push_str("</Point>");
        }
        Geometry::LineString(ls) => {
            out.push_str("<LineString>");
            kml_coords(out, zc, &ls.0);
            out.push_str("</LineString>");
        }
        Geometry::Polygon(p) => kml_polygon(out, zc, p),
        Geometry::MultiPoint(mp) => {
            out.push_str("<MultiGeometry>");
            for p in &mp.0 {
                kml_geometry(out, zc, &Geometry::Point(*p));
            }
            out.push_str("</MultiGeometry>");
        }
        Geometry::MultiLineString(m) => {
            out.push_str("<MultiGeometry>");
            for ls in &m.0 {
                kml_geometry(out, zc, &Geometry::LineString(ls.clone()));
            }
            out.push_str("</MultiGeometry>");
        }
        Geometry::MultiPolygon(m) => {
            out.push_str("<MultiGeometry>");
            for p in &m.0 {
                kml_polygon(out, zc, p);
            }
            out.push_str("</MultiGeometry>");
        }
        Geometry::GeometryCollection(gc) => {
            out.push_str("<MultiGeometry>");
            for g in &gc.0 {
                kml_geometry(out, zc, g);
            }
            out.push_str("</MultiGeometry>");
        }
        Geometry::Line(l) => kml_geometry(out, zc, &Geometry::LineString(l.into())),
        Geometry::Rect(r) => kml_geometry(out, zc, &Geometry::Polygon(r.to_polygon())),
        Geometry::Triangle(t) => kml_geometry(out, zc, &Geometry::Polygon(t.to_polygon())),
    }
}

pub const KML_HEADER: &str = r#"<?xml version="1.0" encoding="UTF-8"?><kml xmlns="http://www.opengis.net/kml/2.2"><Document>"#;
pub const KML_FOOTER: &str = "</Document></kml>";

pub fn kml_placemark(
    out: &mut String,
    def: &FeatureTypeDef,
    f: &Feature,
    properties: &Option<Vec<usize>>,
) {
    use crate::wfs::xml::escape;
    let _ = write!(
        out,
        r#"<Placemark id="{}"><name>{}</name><ExtendedData>"#,
        escape(&f.id),
        escape(&f.id)
    );
    let geom_idx = def.default_geometry().map(|(i, _)| i);
    for i in emitted(def, properties) {
        if Some(i) == geom_idx {
            continue;
        }
        let text = match &f.values[i] {
            Value::Null | Value::Geometry(_) => continue,
            Value::Xml(x) => xml_text(x),
            v => v.lexical().unwrap_or_default(),
        };
        let _ = write!(
            out,
            r#"<Data name="{}"><value>{}</value></Data>"#,
            escape(&def.properties[i].name.local),
            escape(&text)
        );
    }
    out.push_str("</ExtendedData>");
    if let Some(Value::Geometry(g)) = geom_idx.map(|i| &f.values[i]) {
        let mut geom = g.geometry.clone();
        if def.srid != 4326 && crs::transform(&mut geom, def.srid, 4326).is_err() {
            out.push_str("</Placemark>");
            return;
        }
        kml_geometry(out, &mut ZCursor::new(g.z.as_deref()), &geom);
    }
    out.push_str("</Placemark>");
}

// ---------------------------------------------------------------- SHAPE-ZIP

/// Write features of one type as zipped shapefiles (one per geometry type)
/// SHAPE-ZIP options (GeoServer format options CHARSET, PRJFILEFORMAT)
#[derive(Clone, Debug)]
pub struct ShapeZipOptions {
    /// DBF character set name (UTF-8, ISO-8859-1, ISO-8859-15, windows-1252)
    pub charset: String,
    /// .prj in ESRI WKT dialect
    pub esri_prj: bool,
}

impl ShapeZipOptions {
    pub fn from_options(
        options: &std::collections::HashMap<String, String>,
    ) -> Result<Self, String> {
        let charset = options
            .get("charset")
            .cloned()
            .unwrap_or_else(|| "UTF-8".to_string());
        if encode_text("", &charset).is_none() {
            return Err(format!("Unsupported charset `{charset}`"));
        }
        Ok(ShapeZipOptions {
            charset,
            esri_prj: options
                .get("prjfileformat")
                .is_some_and(|v| v.eq_ignore_ascii_case("ESRI")),
        })
    }
}

/// Encode text in a character set (None: unsupported character set). Unmappable characters
/// become `?`.
fn encode_text(text: &str, charset: &str) -> Option<Vec<u8>> {
    let cs = charset.to_ascii_uppercase().replace('_', "-");
    match cs.as_str() {
        "UTF-8" | "UTF8" => Some(text.as_bytes().to_vec()),
        "ISO-8859-1" | "LATIN1" | "ISO-8859-15" | "LATIN9" | "WINDOWS-1252" | "CP1252" => {
            let latin9 = matches!(cs.as_str(), "ISO-8859-15" | "LATIN9");
            let cp1252 = matches!(cs.as_str(), "WINDOWS-1252" | "CP1252");
            Some(
                text.chars()
                    .map(|c| {
                        let special: Option<u8> = match (c, latin9, cp1252) {
                            ('€', true, _) => Some(0xA4),
                            ('Š', true, _) => Some(0xA6),
                            ('š', true, _) => Some(0xA8),
                            ('Ž', true, _) => Some(0xB4),
                            ('ž', true, _) => Some(0xB8),
                            ('Œ', true, _) => Some(0xBC),
                            ('œ', true, _) => Some(0xBD),
                            ('Ÿ', true, _) => Some(0xBE),
                            ('€', _, true) => Some(0x80),
                            ('Š', _, true) => Some(0x8A),
                            ('š', _, true) => Some(0x9A),
                            ('Ž', _, true) => Some(0x8E),
                            ('ž', _, true) => Some(0x9E),
                            ('Œ', _, true) => Some(0x8C),
                            ('œ', _, true) => Some(0x9C),
                            ('Ÿ', _, true) => Some(0x9F),
                            _ => None,
                        };
                        let latin9_slot = latin9
                            && matches!(
                                c as u32,
                                0xA4 | 0xA6 | 0xA8 | 0xB4 | 0xB8 | 0xBC | 0xBD | 0xBE
                            );
                        special.unwrap_or(if (c as u32) < 0x100 && !latin9_slot {
                            c as u8
                        } else {
                            b'?'
                        })
                    })
                    .collect(),
            )
        }
        _ => None,
    }
}

/// dBASE III field
struct DbfField {
    name: String,
    kind: u8,
    width: u8,
    decimals: u8,
}

/// Minimal dBASE III table writer
fn write_dbf(fields: &[DbfField], records: &[Vec<Vec<u8>>]) -> Vec<u8> {
    let header_len = 32 + 32 * fields.len() + 1;
    let record_len = 1 + fields.iter().map(|f| f.width as usize).sum::<usize>();
    let mut out = Vec::with_capacity(header_len + record_len * records.len() + 1);
    let today = chrono::Utc::now();
    out.push(0x03);
    out.push((today.format("%y").to_string().parse::<u8>().unwrap_or(0)) + 100);
    out.push(today.format("%m").to_string().parse::<u8>().unwrap_or(1));
    out.push(today.format("%d").to_string().parse::<u8>().unwrap_or(1));
    out.extend((records.len() as u32).to_le_bytes());
    out.extend((header_len as u16).to_le_bytes());
    out.extend((record_len as u16).to_le_bytes());
    out.extend([0u8; 20]);
    for f in fields {
        let mut name = [0u8; 11];
        for (i, b) in f.name.bytes().take(10).enumerate() {
            name[i] = b;
        }
        out.extend(name);
        out.push(f.kind);
        out.extend([0u8; 4]);
        out.push(f.width);
        out.push(f.decimals);
        out.extend([0u8; 14]);
    }
    out.push(0x0D);
    for rec in records {
        out.push(b' ');
        for (f, v) in fields.iter().zip(rec) {
            let w = f.width as usize;
            let mut cell = vec![b' '; w];
            let v = &v[..v.len().min(w)];
            if f.kind == b'N' {
                // right aligned
                cell[w - v.len()..].copy_from_slice(v);
            } else {
                cell[..v.len()].copy_from_slice(v);
            }
            out.extend(cell);
        }
    }
    out.push(0x1A);
    out
}

/// Empty .shp / .shx (100 byte header) of a shape type
fn empty_shp(shape_type: i32) -> Vec<u8> {
    let mut h = Vec::with_capacity(100);
    h.extend(9994i32.to_be_bytes());
    h.extend([0u8; 20]);
    h.extend(50i32.to_be_bytes());
    h.extend(1000i32.to_le_bytes());
    h.extend(shape_type.to_le_bytes());
    h.extend([0u8; 64]);
    h
}

/// .prj content: OGC WKT, or the ESRI dialect
fn prj_wkt(epsg: u16, esri: bool) -> Option<String> {
    if esri {
        match epsg {
            4326 => return Some(r#"GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]]"#.to_string()),
            3857 => return Some(r#"PROJCS["WGS_1984_Web_Mercator_Auxiliary_Sphere",GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Mercator_Auxiliary_Sphere"],PARAMETER["False_Easting",0.0],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",0.0],PARAMETER["Standard_Parallel_1",0.0],PARAMETER["Auxiliary_Sphere_Type",0.0],UNIT["Meter",1.0]]"#.to_string()),
            _ => {}
        }
    }
    let wkt = crs_definitions::from_code(epsg).map(|d| d.wkt.to_string())?;
    if !esri {
        return Some(wkt);
    }
    // ESRI readers ignore authority codes: strip AUTHORITY[...] clauses
    let mut out = String::new();
    let mut rest = wkt.as_str();
    while let Some(i) = rest.find(",AUTHORITY[") {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        let mut depth = 0;
        let mut end = rest.len();
        for (j, c) in rest.char_indices() {
            match c {
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        end = j + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    Some(out)
}

fn shape_type_code(g: Option<GeomType>) -> Option<i32> {
    match g? {
        GeomType::Point => Some(1),
        GeomType::LineString | GeomType::MultiLineString => Some(3),
        GeomType::Polygon | GeomType::MultiPolygon => Some(5),
        GeomType::MultiPoint => Some(8),
        GeomType::Geometry | GeomType::MultiGeometry => None,
    }
}

/// Write the features of one feature type as shapefile(s) into a zip archive: one shapefile per
/// geometry column (several columns) or per shape type (generic geometry column)
pub fn shape_zip(
    zip: &mut zip::ZipWriter<std::io::Cursor<Vec<u8>>>,
    def: &FeatureTypeDef,
    crs: &OutputCrs,
    features: &[Feature],
    opts: &ShapeZipOptions,
) -> Result<(), String> {
    use std::collections::BTreeMap;
    use std::io::{Cursor, Write};
    let geoms: Vec<usize> = def
        .properties
        .iter()
        .enumerate()
        .filter(|(_, p)| p.is_geometry() && !p.name.is_gml())
        .map(|(i, _)| i)
        .collect();
    let attrs: Vec<usize> = emitted(def, &None)
        .into_iter()
        .filter(|i| !def.properties[*i].is_geometry())
        .collect();
    // dbf field names: max 10 characters, unique
    let mut names: Vec<String> = Vec::new();
    for &i in &attrs {
        let base: String = def.properties[i]
            .name
            .local
            .chars()
            .filter(|c| c.is_ascii())
            .take(10)
            .collect();
        let mut name = if base.is_empty() {
            format!("F{i}")
        } else {
            base
        };
        let mut n = 1;
        while names.contains(&name) {
            name = format!("{}{n}", &name[..name.len().min(8)]);
            n += 1;
        }
        names.push(name);
    }
    let fields: Vec<DbfField> = attrs
        .iter()
        .zip(&names)
        .map(|(&i, name)| {
            let (kind, width, decimals) = match def.properties[i].value_type {
                ValueType::Integer => (b'N', 18, 0),
                ValueType::Double | ValueType::Decimal => (b'N', 24, 15),
                ValueType::Boolean => (b'L', 1, 0),
                ValueType::Date => (b'D', 8, 0),
                _ => (b'C', 254, 0),
            };
            DbfField {
                name: name.clone(),
                kind,
                width,
                decimals,
            }
        })
        .collect();
    let record = |f: &Feature| -> Vec<Vec<u8>> {
        attrs
            .iter()
            .map(|&i| {
                let v = &f.values[i];
                match def.properties[i].value_type {
                    ValueType::Integer | ValueType::Double | ValueType::Decimal => v
                        .lexical()
                        .and_then(|s| s.parse::<f64>().ok().map(|_| s.into_bytes()))
                        .unwrap_or_default(),
                    ValueType::Boolean => match v {
                        Value::Boolean(true) => b"T".to_vec(),
                        Value::Boolean(false) => b"F".to_vec(),
                        _ => b"?".to_vec(),
                    },
                    ValueType::Date => v
                        .lexical()
                        .map(|s| {
                            s.chars()
                                .filter(|c| c.is_ascii_digit())
                                .take(8)
                                .collect::<String>()
                                .into_bytes()
                        })
                        .unwrap_or_default(),
                    _ => {
                        let text = match v {
                            Value::Null => String::new(),
                            Value::Xml(x) => xml_text(x),
                            v => v.lexical().unwrap_or_default(),
                        };
                        let mut bytes = encode_text(&text, &opts.charset).unwrap_or_default();
                        bytes.truncate(254);
                        bytes
                    }
                }
            })
            .collect()
    };
    // shapefiles: (base name, declared shape type, shapes with feature)
    type ShapeFile<'f> = (String, Option<i32>, Vec<(shapefile::Shape, &'f Feature)>);
    let mut files: Vec<ShapeFile> = Vec::new();
    let several = geoms.len() > 1;
    for &gi in &geoms {
        let declared = match def.properties[gi].value_type {
            ValueType::Geometry(g) => Some(g),
            _ => None,
        };
        let mut groups: BTreeMap<&'static str, Vec<(shapefile::Shape, &Feature)>> = BTreeMap::new();
        for f in features {
            let Value::Geometry(g) = &f.values[gi] else {
                continue;
            };
            let Some(geom) = transform(def, crs, &g.geometry) else {
                continue;
            };
            let shape = shapefile::Shape::try_from(geom).map_err(|e| format!("{e:?}"))?;
            let key = match &shape {
                shapefile::Shape::Point(_) => "Point",
                shapefile::Shape::Multipoint(_) => "MultiPoint",
                shapefile::Shape::Polyline(_) => "Line",
                shapefile::Shape::Polygon(_) => "Polygon",
                _ => continue,
            };
            groups.entry(key).or_default().push((shape, f));
        }
        let prefix = if several {
            format!("{}{}", def.name.local, def.properties[gi].name.local)
        } else {
            def.name.local.clone()
        };
        let code = shape_type_code(declared);
        if groups.is_empty() {
            files.push((prefix, code, Vec::new()));
        } else if groups.len() == 1 {
            files.push((
                prefix,
                code,
                groups.into_values().next().unwrap_or_default(),
            ));
        } else {
            for (key, items) in groups {
                files.push((format!("{prefix}{key}"), None, items));
            }
        }
    }
    let opts_zip = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut put = |name: String, data: &[u8]| -> Result<(), String> {
        zip.start_file(name, opts_zip).map_err(|e| e.to_string())?;
        zip.write_all(data).map_err(|e| e.to_string())
    };
    let prj = prj_wkt(crs.epsg, opts.esri_prj);
    for (base, code, items) in files {
        let (shp, shx) = if items.is_empty() {
            match code {
                Some(c) => (empty_shp(c), empty_shp(c)),
                None => {
                    // generic geometry type without features: no shapefile type can be chosen
                    put(
                        "README.TXT".to_string(),
                        format!("The {base} feature type has a generic geometry type and the result is empty: no shapefile was written.\n").as_bytes(),
                    )?;
                    continue;
                }
            }
        } else {
            let mut shp = Cursor::new(Vec::new());
            let mut shx = Cursor::new(Vec::new());
            {
                let mut writer = shapefile::ShapeWriter::with_shx(&mut shp, &mut shx);
                for (shape, _) in &items {
                    let r = match shape {
                        shapefile::Shape::Point(p) => writer.write_shape(p),
                        shapefile::Shape::Multipoint(p) => writer.write_shape(p),
                        shapefile::Shape::Polyline(p) => writer.write_shape(p),
                        shapefile::Shape::Polygon(p) => writer.write_shape(p),
                        _ => Ok(()),
                    };
                    r.map_err(|e| format!("{e:?}"))?;
                }
                writer.finalize().map_err(|e| format!("{e:?}"))?;
            }
            (shp.into_inner(), shx.into_inner())
        };
        let records: Vec<Vec<Vec<u8>>> = items.iter().map(|(_, f)| record(f)).collect();
        put(format!("{base}.shp"), &shp)?;
        put(format!("{base}.shx"), &shx)?;
        put(format!("{base}.dbf"), &write_dbf(&fields, &records))?;
        if let Some(wkt) = &prj {
            put(format!("{base}.prj"), wkt.as_bytes())?;
        }
        put(format!("{base}.cst"), opts.charset.as_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod dbf_tests {
    use super::*;

    #[test]
    fn dbf_readable_by_dbase() {
        let fields = vec![
            DbfField {
                name: "NAME".into(),
                kind: b'C',
                width: 20,
                decimals: 0,
            },
            DbfField {
                name: "COUNT".into(),
                kind: b'N',
                width: 18,
                decimals: 0,
            },
            DbfField {
                name: "RATIO".into(),
                kind: b'N',
                width: 24,
                decimals: 15,
            },
            DbfField {
                name: "FLAG".into(),
                kind: b'L',
                width: 1,
                decimals: 0,
            },
        ];
        let records = vec![
            vec![
                encode_text("Zürich €", "ISO-8859-15").unwrap(),
                b"42".to_vec(),
                b"0.5".to_vec(),
                b"T".to_vec(),
            ],
            vec![Vec::new(), Vec::new(), Vec::new(), b"?".to_vec()],
        ];
        let data = write_dbf(&fields, &records);
        let mut reader = shapefile::dbase::Reader::new(std::io::Cursor::new(data)).unwrap();
        let recs = reader.read().unwrap();
        assert_eq!(recs.len(), 2);
        use shapefile::dbase::FieldValue;
        assert_eq!(recs[0].get("COUNT"), Some(&FieldValue::Numeric(Some(42.0))));
        assert_eq!(recs[0].get("RATIO"), Some(&FieldValue::Numeric(Some(0.5))));
        assert_eq!(recs[0].get("FLAG"), Some(&FieldValue::Logical(Some(true))));
        assert_eq!(recs[1].get("COUNT"), Some(&FieldValue::Numeric(None)));
        // Latin-9 bytes
        assert_eq!(encode_text("€ü", "ISO-8859-15").unwrap(), vec![0xA4, 0xFC]);
        assert_eq!(encode_text("€", "windows-1252").unwrap(), vec![0x80]);
        assert!(encode_text("x", "EBCDIC").is_none());
        // empty shapefile header
        let shp = empty_shp(5);
        assert_eq!(shp.len(), 100);
        assert_eq!(&shp[0..4], &9994i32.to_be_bytes());
        assert_eq!(&shp[32..36], &5i32.to_le_bytes());
        // ESRI prj
        assert!(prj_wkt(4326, true)
            .unwrap()
            .starts_with("GEOGCS[\"GCS_WGS_1984\""));
        assert!(!prj_wkt(32632, true).unwrap().contains("AUTHORITY"));
    }
}
