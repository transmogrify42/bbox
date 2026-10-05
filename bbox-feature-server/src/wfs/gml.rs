//! GML geometry encoding and decoding (GML 2.1.2, 3.1.1 and 3.2).

use geo::{Coord, Geometry, Rect};

pub const GML_NS: &str = "http://www.opengis.net/gml";
pub const GML32_NS: &str = "http://www.opengis.net/gml/3.2";

/// Whether the node is an element in a GML namespace
pub fn is_gml_element(node: &roxmltree::Node) -> bool {
    is_gml(node)
}

/// Swap x and y of all coordinates
pub fn swap_xy(geom: &mut Geometry<f64>) {
    crate::wfs::crs::try_map_coords(geom, &mut |c| Ok(Coord { x: c.y, y: c.x }))
        .expect("infallible");
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GmlVersion {
    /// GML 2.1.2 (WFS 1.0)
    V2,
    /// GML 3.1.1 (WFS 1.1)
    V31,
    /// GML 3.2 (WFS 2.0)
    V32,
}

#[derive(Clone, Debug)]
pub struct GmlWriteOpts<'a> {
    pub version: GmlVersion,
    pub srs_name: Option<&'a str>,
    /// Swap x/y on output (e.g. lat/lon axis order for urn EPSG:4326)
    pub swap_xy: bool,
    /// Base for gml:id of geometry elements (mandatory in GML 3.2)
    pub id: Option<&'a str>,
    /// GML metadata elements (gml:description, gml:name) of the top-level geometry
    pub meta: Option<&'a str>,
    /// Z ordinates in coordinate traversal order (3D geometry); for envelopes [zmin, zmax]
    pub z: Option<&'a [f64]>,
}

#[derive(thiserror::Error, Debug, PartialEq)]
pub enum GmlError {
    #[error("unsupported GML geometry element `{0}`")]
    Unsupported(String),
    #[error("invalid GML geometry: {0}")]
    Invalid(String),
}

pub fn write_geometry(out: &mut String, geom: &Geometry<f64>, opts: &GmlWriteOpts) {
    GmlWriter { out, opts, zi: 0 }.geometry(geom, opts.id.map(str::to_string), true);
}

pub fn write_envelope(out: &mut String, rect: &Rect<f64>, opts: &GmlWriteOpts) {
    let mut w = GmlWriter { out, opts, zi: 0 };
    let (min, max) = (rect.min(), rect.max());
    if opts.version == GmlVersion::V2 {
        w.out.push_str("<gml:Box");
        w.srs_attr();
        w.out.push('>');
        w.coordinates(&[min, max]);
        w.out.push_str("</gml:Box>");
    } else {
        w.out.push_str("<gml:Envelope");
        w.srs_attr();
        w.out.push_str("><gml:lowerCorner>");
        w.pos(min);
        w.out.push_str("</gml:lowerCorner><gml:upperCorner>");
        w.pos(max);
        w.out.push_str("</gml:upperCorner></gml:Envelope>");
    }
}

struct GmlWriter<'w, 'a> {
    out: &'w mut String,
    opts: &'w GmlWriteOpts<'a>,
    /// Next Z ordinate
    zi: usize,
}

impl GmlWriter<'_, '_> {
    fn srs_attr(&mut self) {
        if let Some(srs) = self.opts.srs_name {
            self.out.push_str(" srsName=\"");
            self.out.push_str(&super::xml::escape(srs));
            self.out.push('"');
        }
        if self.opts.z.is_some() && self.opts.version != GmlVersion::V2 {
            self.out.push_str(" srsDimension=\"3\"");
        }
    }
    /// Next Z ordinate, if 3D
    fn next_z(&mut self) -> Option<f64> {
        let z = self.opts.z?;
        let v = z.get(self.zi).copied().unwrap_or(0.0);
        self.zi += 1;
        Some(v)
    }
    fn start(&mut self, name: &str, id: &Option<String>, top: bool) {
        self.out.push_str("<gml:");
        self.out.push_str(name);
        if self.opts.version != GmlVersion::V2 {
            if let Some(id) = id {
                self.out.push_str(" gml:id=\"");
                self.out.push_str(&super::xml::escape(id));
                self.out.push('"');
            }
        }
        if top {
            self.srs_attr();
        }
        self.out.push('>');
        if top && self.opts.version != GmlVersion::V2 {
            if let Some(meta) = self.opts.meta {
                self.out.push_str(meta);
            }
        }
    }
    fn end(&mut self, name: &str) {
        self.out.push_str("</gml:");
        self.out.push_str(name);
        self.out.push('>');
    }
    fn xy(&self, c: Coord<f64>) -> (f64, f64) {
        if self.opts.swap_xy {
            (c.y, c.x)
        } else {
            (c.x, c.y)
        }
    }
    /// Write a coordinate tuple with separator `cs`
    fn tuple(&mut self, c: Coord<f64>, cs: char) {
        use std::fmt::Write;
        let (a, b) = self.xy(c);
        let _ = write!(self.out, "{a}{cs}{b}");
        if let Some(z) = self.next_z() {
            let _ = write!(self.out, "{cs}{z}");
        }
    }
    fn pos(&mut self, c: Coord<f64>) {
        self.tuple(c, ' ');
    }
    fn coordinates(&mut self, coords: &[Coord<f64>]) {
        self.out
            .push_str(r#"<gml:coordinates decimal="." cs="," ts=" ">"#);
        for (i, c) in coords.iter().enumerate() {
            if i > 0 {
                self.out.push(' ');
            }
            self.tuple(*c, ',');
        }
        self.out.push_str("</gml:coordinates>");
    }
    fn pos_list(&mut self, coords: &[Coord<f64>]) {
        if self.opts.version == GmlVersion::V2 {
            self.coordinates(coords);
        } else {
            self.out.push_str("<gml:posList>");
            for (i, c) in coords.iter().enumerate() {
                if i > 0 {
                    self.out.push(' ');
                }
                self.tuple(*c, ' ');
            }
            self.out.push_str("</gml:posList>");
        }
    }
    fn ring(&mut self, ring: &geo::LineString<f64>) {
        self.out.push_str("<gml:LinearRing>");
        self.pos_list(&ring.0);
        self.out.push_str("</gml:LinearRing>");
    }
    fn member_id(id: &Option<String>, i: usize) -> Option<String> {
        id.as_ref().map(|id| format!("{id}.{}", i + 1))
    }
    fn geometry(&mut self, geom: &Geometry<f64>, id: Option<String>, top: bool) {
        let v2 = self.opts.version == GmlVersion::V2;
        match geom {
            Geometry::Point(p) => {
                self.start("Point", &id, top);
                if v2 {
                    self.coordinates(&[p.0]);
                } else {
                    self.out.push_str("<gml:pos>");
                    self.pos(p.0);
                    self.out.push_str("</gml:pos>");
                }
                self.end("Point");
            }
            Geometry::Line(l) => {
                self.geometry(&Geometry::LineString(l.into()), id, top);
            }
            Geometry::LineString(ls) => {
                self.start("LineString", &id, top);
                self.pos_list(&ls.0);
                self.end("LineString");
            }
            Geometry::Polygon(poly) => {
                self.start("Polygon", &id, top);
                let (ext, int) = if v2 {
                    ("outerBoundaryIs", "innerBoundaryIs")
                } else {
                    ("exterior", "interior")
                };
                self.out.push_str(&format!("<gml:{ext}>"));
                self.ring(poly.exterior());
                self.out.push_str(&format!("</gml:{ext}>"));
                for ring in poly.interiors() {
                    self.out.push_str(&format!("<gml:{int}>"));
                    self.ring(ring);
                    self.out.push_str(&format!("</gml:{int}>"));
                }
                self.end("Polygon");
            }
            Geometry::Rect(r) => self.geometry(&Geometry::Polygon(r.to_polygon()), id, top),
            Geometry::Triangle(t) => self.geometry(&Geometry::Polygon(t.to_polygon()), id, top),
            Geometry::MultiPoint(mp) => {
                self.start("MultiPoint", &id, top);
                for (i, p) in mp.iter().enumerate() {
                    self.out.push_str("<gml:pointMember>");
                    self.geometry(&Geometry::Point(*p), Self::member_id(&id, i), false);
                    self.out.push_str("</gml:pointMember>");
                }
                self.end("MultiPoint");
            }
            Geometry::MultiLineString(mls) => {
                let (name, member) = if v2 {
                    ("MultiLineString", "lineStringMember")
                } else {
                    ("MultiCurve", "curveMember")
                };
                self.start(name, &id, top);
                for (i, ls) in mls.iter().enumerate() {
                    self.out.push_str(&format!("<gml:{member}>"));
                    self.geometry(
                        &Geometry::LineString(ls.clone()),
                        Self::member_id(&id, i),
                        false,
                    );
                    self.out.push_str(&format!("</gml:{member}>"));
                }
                self.end(name);
            }
            Geometry::MultiPolygon(mp) => {
                let (name, member) = if v2 {
                    ("MultiPolygon", "polygonMember")
                } else {
                    ("MultiSurface", "surfaceMember")
                };
                self.start(name, &id, top);
                for (i, poly) in mp.iter().enumerate() {
                    self.out.push_str(&format!("<gml:{member}>"));
                    self.geometry(
                        &Geometry::Polygon(poly.clone()),
                        Self::member_id(&id, i),
                        false,
                    );
                    self.out.push_str(&format!("</gml:{member}>"));
                }
                self.end(name);
            }
            Geometry::GeometryCollection(gc) => {
                self.start("MultiGeometry", &id, top);
                for (i, g) in gc.iter().enumerate() {
                    self.out.push_str("<gml:geometryMember>");
                    self.geometry(g, Self::member_id(&id, i), false);
                    self.out.push_str("</gml:geometryMember>");
                }
                self.end("MultiGeometry");
            }
        }
    }
}

/// Parsed geometry with optional srsName attribute
#[derive(Debug, PartialEq)]
pub struct GmlGeometry {
    pub geometry: Geometry<f64>,
    pub srs_name: Option<String>,
    /// Z ordinates in coordinate traversal order (3D geometry)
    pub z: Option<Vec<f64>>,
}

/// Parse a GML geometry element (any supported GML version)
pub fn parse_geometry(node: roxmltree::Node) -> Result<GmlGeometry, GmlError> {
    let srs_name = node.attribute("srsName").map(str::to_string);
    let dim = node
        .attribute("srsDimension")
        .and_then(|d| d.parse().ok())
        .unwrap_or_else(|| srs_name.as_deref().map(crs_dimension).unwrap_or(2));
    let (geometry, z) = geometry(node, dim)?;
    let z = if z.iter().any(|v| !v.is_nan()) {
        Some(
            z.into_iter()
                .map(|v| if v.is_nan() { 0.0 } else { v })
                .collect(),
        )
    } else {
        None
    };
    Ok(GmlGeometry {
        geometry,
        srs_name,
        z,
    })
}

/// Parse gml:Envelope or gml:Box
pub fn parse_envelope(node: roxmltree::Node) -> Result<(Rect<f64>, Option<String>), GmlError> {
    let srs_name = node.attribute("srsName").map(str::to_string);
    let dim = srs_dimension(node);
    let coords = match node.tag_name().name() {
        "Envelope" => {
            let lower = child(node, "lowerCorner");
            let upper = child(node, "upperCorner");
            match (lower, upper) {
                (Some(l), Some(u)) => {
                    let mut c = xy(pos(l, dim)?);
                    c.extend(xy(pos(u, dim)?));
                    c
                }
                _ => {
                    // deprecated gml:pos / gml:coord / gml:coordinates variants
                    xy(point_seq(node, dim)?)
                }
            }
        }
        "Box" => xy(point_seq(node, dim)?),
        other => return Err(GmlError::Unsupported(other.to_string())),
    };
    if coords.len() != 2 {
        return Err(GmlError::Invalid("envelope requires two corners".into()));
    }
    Ok((Rect::new(coords[0], coords[1]), srs_name))
}

fn is_gml(node: &roxmltree::Node) -> bool {
    node.is_element()
        && node
            .tag_name()
            .namespace()
            .map(|ns| ns.starts_with("http://www.opengis.net/gml"))
            .unwrap_or(false)
}

fn children<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    name: &'a str,
) -> impl Iterator<Item = roxmltree::Node<'a, 'input>> + 'a {
    node.children()
        .filter(move |c| is_gml(c) && c.tag_name().name() == name)
}

fn child<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    name: &'a str,
) -> Option<roxmltree::Node<'a, 'input>> {
    children(node, name).next()
}

fn first_element<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
) -> Result<roxmltree::Node<'a, 'input>, GmlError> {
    node.children()
        .find(|c| c.is_element())
        .ok_or_else(|| GmlError::Invalid(format!("empty `{}`", node.tag_name().name())))
}

/// Coordinate dimension implied by a CRS identifier (3D geographic/geocentric CRSs)
fn crs_dimension(srs_name: &str) -> usize {
    match crate::wfs::crs::Crs::parse(srs_name) {
        Ok(crs)
            if matches!(
                crs.epsg,
                4978 | 4979 | 4936 | 4937 | 7409 | 7415 | 7423 | 9518 | 9705
            ) =>
        {
            3
        }
        _ => 2,
    }
}

fn srs_dimension(node: roxmltree::Node) -> usize {
    node.attribute("srsDimension")
        .and_then(|d| d.parse().ok())
        .unwrap_or(2)
}

fn parse_num(s: &str) -> Result<f64, GmlError> {
    s.trim()
        .parse()
        .map_err(|_| GmlError::Invalid(format!("invalid number `{s}`")))
}

/// Coordinate with Z ordinate (NaN if 2D)
type C3 = (Coord<f64>, f64);

fn xy(coords: Vec<C3>) -> Vec<Coord<f64>> {
    coords.into_iter().map(|(c, _)| c).collect()
}

/// Split into 2D coordinates and Z ordinates
fn split(coords: Vec<C3>) -> (Vec<Coord<f64>>, Vec<f64>) {
    coords.into_iter().unzip()
}

fn numbers_to_coords(nums: Vec<f64>, dim: usize) -> Result<Vec<C3>, GmlError> {
    if dim < 2 || nums.len() % dim != 0 {
        return Err(GmlError::Invalid(format!(
            "{} ordinates do not match dimension {dim}",
            nums.len()
        )));
    }
    Ok(nums
        .chunks(dim)
        .map(|c| {
            (
                Coord { x: c[0], y: c[1] },
                c.get(2).copied().unwrap_or(f64::NAN),
            )
        })
        .collect())
}

/// gml:pos or gml:posList content
fn pos(node: roxmltree::Node, dim: usize) -> Result<Vec<C3>, GmlError> {
    let dim = node
        .attribute("srsDimension")
        .and_then(|d| d.parse().ok())
        .unwrap_or(dim);
    let nums = node
        .text()
        .unwrap_or("")
        .split_whitespace()
        .map(parse_num)
        .collect::<Result<Vec<_>, _>>()?;
    numbers_to_coords(nums, dim)
}

/// GML2 gml:coordinates content
fn coordinates(node: roxmltree::Node) -> Result<Vec<C3>, GmlError> {
    let cs = node.attribute("cs").unwrap_or(",");
    let ts = node.attribute("ts").unwrap_or(" ");
    let decimal = node.attribute("decimal").unwrap_or(".");
    let text = node.text().unwrap_or("").trim();
    let tuples: Vec<&str> = if ts.trim().is_empty() {
        text.split_whitespace().collect()
    } else {
        text.split(ts)
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect()
    };
    tuples
        .iter()
        .map(|t| {
            let parts: Vec<f64> = if cs.trim().is_empty() {
                t.split_whitespace().collect::<Vec<_>>()
            } else {
                t.split(cs).collect()
            }
            .iter()
            .map(|v| parse_num(&v.replace(decimal, ".")))
            .collect::<Result<_, _>>()?;
            if parts.len() < 2 {
                return Err(GmlError::Invalid(format!("invalid coordinate tuple `{t}`")));
            }
            Ok((
                Coord {
                    x: parts[0],
                    y: parts[1],
                },
                parts.get(2).copied().unwrap_or(f64::NAN),
            ))
        })
        .collect()
}

/// GML2 gml:coord with gml:X / gml:Y / gml:Z
fn coord(node: roxmltree::Node) -> Result<C3, GmlError> {
    let x = child(node, "X").and_then(|n| n.text());
    let y = child(node, "Y").and_then(|n| n.text());
    let z = match child(node, "Z").and_then(|n| n.text()) {
        Some(z) => parse_num(z)?,
        None => f64::NAN,
    };
    match (x, y) {
        (Some(x), Some(y)) => Ok((
            Coord {
                x: parse_num(x)?,
                y: parse_num(y)?,
            },
            z,
        )),
        _ => Err(GmlError::Invalid("gml:coord requires X and Y".into())),
    }
}

/// Coordinate sequence from any of gml:posList, gml:pos*, gml:coordinates, gml:coord*,
/// gml:pointProperty/gml:Point
fn point_seq(node: roxmltree::Node, dim: usize) -> Result<Vec<C3>, GmlError> {
    let mut coords = Vec::new();
    for c in node.children().filter(is_gml) {
        match c.tag_name().name() {
            "posList" | "pos" => coords.extend(pos(c, dim)?),
            "coordinates" => coords.extend(coordinates(c)?),
            "coord" => coords.push(coord(c)?),
            "pointProperty" | "pointRep" => {
                if let (Geometry::Point(p), z) = geometry(first_element(c)?, dim)? {
                    coords.push((p.0, z.first().copied().unwrap_or(f64::NAN)));
                }
            }
            _ => {}
        }
    }
    Ok(coords)
}

/// Append coordinates, skipping a coordinate equal (in x/y) to the last one (joined segments)
fn append_joined(coords: &mut Vec<C3>, more: Vec<C3>) {
    for c in more {
        let dup = coords.last().map(|l| l.0 == c.0).unwrap_or(false);
        if !dup {
            coords.push(c);
        }
    }
}

fn line_coords(node: roxmltree::Node, dim: usize) -> Result<Vec<C3>, GmlError> {
    match node.tag_name().name() {
        "LineString" | "LinearRing" | "LineStringSegment" => point_seq(node, dim),
        "Curve" => {
            let segments = child(node, "segments")
                .ok_or_else(|| GmlError::Invalid("gml:Curve without segments".into()))?;
            let mut coords = Vec::new();
            for seg in segments.children().filter(is_gml) {
                append_joined(&mut coords, line_coords(seg, dim)?);
            }
            Ok(coords)
        }
        "Ring" => {
            let mut coords = Vec::new();
            for member in children(node, "curveMember") {
                append_joined(&mut coords, line_coords(first_element(member)?, dim)?);
            }
            Ok(coords)
        }
        other => Err(GmlError::Unsupported(other.to_string())),
    }
}

fn line_string(
    node: roxmltree::Node,
    dim: usize,
) -> Result<(geo::LineString<f64>, Vec<f64>), GmlError> {
    let (coords, z) = split(line_coords(node, dim)?);
    Ok((geo::LineString(coords), z))
}

fn ring(node: roxmltree::Node, dim: usize) -> Result<(geo::LineString<f64>, Vec<f64>), GmlError> {
    line_string(first_element(node)?, dim)
}

fn polygon(node: roxmltree::Node, dim: usize) -> Result<(geo::Polygon<f64>, Vec<f64>), GmlError> {
    let ext = child(node, "exterior")
        .or_else(|| child(node, "outerBoundaryIs"))
        .ok_or_else(|| GmlError::Invalid("polygon without exterior".into()))?;
    // Z in traversal order: exterior, then interiors
    let (exterior, mut z) = ring(ext, dim)?;
    let mut interiors = Vec::new();
    for n in children(node, "interior").chain(children(node, "innerBoundaryIs")) {
        let (r, rz) = ring(n, dim)?;
        interiors.push(r);
        z.extend(rz);
    }
    Ok((geo::Polygon::new(exterior, interiors), z))
}

fn polygons(
    node: roxmltree::Node,
    dim: usize,
) -> Result<(Vec<geo::Polygon<f64>>, Vec<f64>), GmlError> {
    match node.tag_name().name() {
        "Polygon" | "PolygonPatch" => {
            let (p, z) = polygon(node, dim)?;
            Ok((vec![p], z))
        }
        "Surface" => {
            let patches = child(node, "patches")
                .ok_or_else(|| GmlError::Invalid("gml:Surface without patches".into()))?;
            let mut polys = Vec::new();
            let mut z = Vec::new();
            for p in patches.children().filter(is_gml) {
                let (poly, pz) = polygon(p, dim)?;
                polys.push(poly);
                z.extend(pz);
            }
            Ok((polys, z))
        }
        _ => match geometry(node, dim)? {
            (Geometry::Polygon(p), z) => Ok((vec![p], z)),
            (Geometry::MultiPolygon(mp), z) => Ok((mp.0, z)),
            _ => Err(GmlError::Invalid("surface member is not a surface".into())),
        },
    }
}

/// Member geometries of multi geometry (xxxMember and xxxMembers)
fn members<'a, 'input>(node: roxmltree::Node<'a, 'input>) -> Vec<roxmltree::Node<'a, 'input>> {
    let mut nodes = Vec::new();
    for c in node.children().filter(is_gml) {
        let name = c.tag_name().name();
        if name.ends_with("Members") {
            nodes.extend(c.children().filter(|n| n.is_element()));
        } else if name.ends_with("Member") {
            if let Some(g) = c.children().find(|n| n.is_element()) {
                nodes.push(g);
            }
        }
    }
    nodes
}

/// Parse a geometry with its Z ordinates in traversal order (NaN for 2D coordinates)
fn geometry(node: roxmltree::Node, dim: usize) -> Result<(Geometry<f64>, Vec<f64>), GmlError> {
    let dim = node
        .attribute("srsDimension")
        .and_then(|d| d.parse().ok())
        .unwrap_or(dim);
    if !is_gml(&node) {
        return Err(GmlError::Unsupported(node.tag_name().name().to_string()));
    }
    let name = node.tag_name().name();
    match name {
        "Point" => {
            let coords = point_seq(node, dim)?;
            let (c, z) = coords
                .first()
                .ok_or_else(|| GmlError::Invalid("empty point".into()))?;
            Ok((Geometry::Point((*c).into()), vec![*z]))
        }
        "LineString" | "LinearRing" | "Curve" | "Ring" => {
            let (ls, z) = line_string(node, dim)?;
            Ok((Geometry::LineString(ls), z))
        }
        "Polygon" | "PolygonPatch" => {
            let (p, z) = polygon(node, dim)?;
            Ok((Geometry::Polygon(p), z))
        }
        "Surface" => {
            let (mut polys, z) = polygons(node, dim)?;
            if polys.len() == 1 {
                Ok((Geometry::Polygon(polys.remove(0)), z))
            } else {
                Ok((Geometry::MultiPolygon(geo::MultiPolygon(polys)), z))
            }
        }
        "Envelope" | "Box" => {
            let (rect, _) = parse_envelope(node)?;
            let p = rect.to_polygon();
            let n = p.exterior().0.len();
            Ok((Geometry::Polygon(p), vec![f64::NAN; n]))
        }
        "MultiPoint" => {
            let mut points = Vec::new();
            let mut z = Vec::new();
            for m in members(node) {
                match geometry(m, dim)? {
                    (Geometry::Point(p), pz) => {
                        points.push(p);
                        z.extend(pz);
                    }
                    _ => return Err(GmlError::Invalid("point member is not a point".into())),
                }
            }
            Ok((Geometry::MultiPoint(geo::MultiPoint(points)), z))
        }
        "MultiLineString" | "MultiCurve" => {
            let mut lines = Vec::new();
            let mut z = Vec::new();
            for m in members(node) {
                let (ls, lz) = line_string(m, dim)?;
                lines.push(ls);
                z.extend(lz);
            }
            Ok((Geometry::MultiLineString(geo::MultiLineString(lines)), z))
        }
        "MultiPolygon" | "MultiSurface" => {
            let mut polys = Vec::new();
            let mut z = Vec::new();
            for m in members(node) {
                let (p, pz) = polygons(m, dim)?;
                polys.extend(p);
                z.extend(pz);
            }
            Ok((Geometry::MultiPolygon(geo::MultiPolygon(polys)), z))
        }
        "MultiGeometry" => {
            let mut geoms = Vec::new();
            let mut z = Vec::new();
            for m in members(node) {
                let (g, gz) = geometry(m, dim)?;
                geoms.push(g);
                z.extend(gz);
            }
            Ok((
                Geometry::GeometryCollection(geo::GeometryCollection(geoms)),
                z,
            ))
        }
        other => Err(GmlError::Unsupported(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{line_string, point, polygon, MultiPoint, MultiPolygon};

    fn opts(version: GmlVersion) -> GmlWriteOpts<'static> {
        GmlWriteOpts {
            version,
            srs_name: Some("EPSG:4326"),
            swap_xy: false,
            id: Some("f1.geom"),
            meta: None,
            z: None,
        }
    }

    fn write(geom: impl Into<Geometry<f64>>, opts: &GmlWriteOpts) -> String {
        let mut s = String::new();
        write_geometry(&mut s, &geom.into(), opts);
        s
    }

    fn roundtrip(geom: Geometry<f64>, version: GmlVersion) {
        let xml = write(geom.clone(), &opts(version));
        let wrapped = format!(
            r#"<r xmlns:gml="{}">{xml}</r>"#,
            if version == GmlVersion::V32 {
                "http://www.opengis.net/gml/3.2"
            } else {
                "http://www.opengis.net/gml"
            }
        );
        let doc = roxmltree::Document::parse(&wrapped).unwrap();
        let parsed = parse_geometry(doc.root_element().first_element_child().unwrap()).unwrap();
        assert_eq!(parsed.geometry, geom, "{version:?}: {xml}");
        assert_eq!(parsed.srs_name.as_deref(), Some("EPSG:4326"));
    }

    #[test]
    fn point_gml2() {
        assert_eq!(
            write(point!(x: 1.5, y: -2.0), &opts(GmlVersion::V2)),
            r#"<gml:Point srsName="EPSG:4326"><gml:coordinates decimal="." cs="," ts=" ">1.5,-2</gml:coordinates></gml:Point>"#
        );
    }

    #[test]
    fn point_gml31() {
        let mut o = opts(GmlVersion::V31);
        o.id = None;
        assert_eq!(
            write(point!(x: 1.5, y: -2.0), &o),
            r#"<gml:Point srsName="EPSG:4326"><gml:pos>1.5 -2</gml:pos></gml:Point>"#
        );
    }

    #[test]
    fn point_gml32_has_id() {
        assert_eq!(
            write(point!(x: 1.5, y: -2.0), &opts(GmlVersion::V32)),
            r#"<gml:Point gml:id="f1.geom" srsName="EPSG:4326"><gml:pos>1.5 -2</gml:pos></gml:Point>"#
        );
    }

    #[test]
    fn swap_axis_order() {
        let mut o = opts(GmlVersion::V32);
        o.swap_xy = true;
        o.srs_name = Some("urn:ogc:def:crs:EPSG::4326");
        assert_eq!(
            write(point!(x: 8.5, y: 47.0), &o),
            r#"<gml:Point gml:id="f1.geom" srsName="urn:ogc:def:crs:EPSG::4326"><gml:pos>47 8.5</gml:pos></gml:Point>"#
        );
    }

    #[test]
    fn linestring_versions() {
        let ls = line_string![(x: 0., y: 0.), (x: 1., y: 1.)];
        assert_eq!(
            write(ls.clone(), &opts(GmlVersion::V2)),
            r#"<gml:LineString srsName="EPSG:4326"><gml:coordinates decimal="." cs="," ts=" ">0,0 1,1</gml:coordinates></gml:LineString>"#
        );
        let mut o = opts(GmlVersion::V31);
        o.id = None;
        assert_eq!(
            write(ls, &o),
            r#"<gml:LineString srsName="EPSG:4326"><gml:posList>0 0 1 1</gml:posList></gml:LineString>"#
        );
    }

    #[test]
    fn polygon_versions() {
        let poly = polygon![(x: 0., y: 0.), (x: 1., y: 0.), (x: 1., y: 1.), (x: 0., y: 0.)];
        assert_eq!(
            write(poly.clone(), &opts(GmlVersion::V2)),
            r#"<gml:Polygon srsName="EPSG:4326"><gml:outerBoundaryIs><gml:LinearRing><gml:coordinates decimal="." cs="," ts=" ">0,0 1,0 1,1 0,0</gml:coordinates></gml:LinearRing></gml:outerBoundaryIs></gml:Polygon>"#
        );
        let mut o = opts(GmlVersion::V31);
        o.id = None;
        assert_eq!(
            write(poly, &o),
            r#"<gml:Polygon srsName="EPSG:4326"><gml:exterior><gml:LinearRing><gml:posList>0 0 1 0 1 1 0 0</gml:posList></gml:LinearRing></gml:exterior></gml:Polygon>"#
        );
    }

    #[test]
    fn multi_geometries_use_gml3_names() {
        let poly = polygon![(x: 0., y: 0.), (x: 1., y: 0.), (x: 1., y: 1.), (x: 0., y: 0.)];
        let mp = MultiPolygon(vec![poly]);
        assert!(write(mp.clone(), &opts(GmlVersion::V2)).starts_with(
            r#"<gml:MultiPolygon srsName="EPSG:4326"><gml:polygonMember><gml:Polygon>"#
        ));
        let gml32 = write(mp, &opts(GmlVersion::V32));
        assert!(
            gml32.starts_with(r#"<gml:MultiSurface gml:id="f1.geom" srsName="EPSG:4326"><gml:surfaceMember><gml:Polygon gml:id="f1.geom.1">"#),
            "{gml32}"
        );
    }

    #[test]
    fn roundtrips() {
        let poly = polygon!(
            exterior: [(x: 0., y: 0.), (x: 10., y: 0.), (x: 10., y: 10.), (x: 0., y: 0.)],
            interiors: [[(x: 1., y: 1.), (x: 2., y: 1.), (x: 2., y: 2.), (x: 1., y: 1.)]],
        );
        let geoms: Vec<Geometry<f64>> = vec![
            point!(x: 1.25, y: 2.5).into(),
            line_string![(x: 0., y: 0.), (x: 1., y: 1.)].into(),
            poly.clone().into(),
            MultiPoint(vec![point!(x: 1., y: 2.), point!(x: 3., y: 4.)]).into(),
            geo::MultiLineString(vec![line_string![(x: 0., y: 0.), (x: 1., y: 1.)]]).into(),
            MultiPolygon(vec![poly]).into(),
        ];
        for version in [GmlVersion::V2, GmlVersion::V31, GmlVersion::V32] {
            for g in &geoms {
                roundtrip(g.clone(), version);
            }
        }
    }

    fn parse_str(xml: &str) -> Result<GmlGeometry, GmlError> {
        let doc = roxmltree::Document::parse(xml).unwrap();
        parse_geometry(doc.root_element())
    }

    #[test]
    fn parse_variants() {
        // GML2 coordinates with custom separators
        let g = parse_str(r#"<gml:Point xmlns:gml="http://www.opengis.net/gml"><gml:coordinates cs=" " ts=";">1 2</gml:coordinates></gml:Point>"#).unwrap();
        assert_eq!(g.geometry, point!(x: 1., y: 2.).into());
        // GML2 coord/X/Y
        let g = parse_str(r#"<gml:Point xmlns:gml="http://www.opengis.net/gml"><gml:coord><gml:X>3</gml:X><gml:Y>4</gml:Y></gml:coord></gml:Point>"#).unwrap();
        assert_eq!(g.geometry, point!(x: 3., y: 4.).into());
        // GML3 pos list with srsDimension 3 drops z
        let g = parse_str(r#"<gml:LineString xmlns:gml="http://www.opengis.net/gml/3.2" gml:id="a"><gml:posList srsDimension="3">0 0 5 1 1 5</gml:posList></gml:LineString>"#).unwrap();
        assert_eq!(
            g.geometry,
            line_string![(x: 0., y: 0.), (x: 1., y: 1.)].into()
        );
        // Sequence of gml:pos in LineString
        let g = parse_str(r#"<gml:LineString xmlns:gml="http://www.opengis.net/gml"><gml:pos>0 0</gml:pos><gml:pos>2 2</gml:pos></gml:LineString>"#).unwrap();
        assert_eq!(
            g.geometry,
            line_string![(x: 0., y: 0.), (x: 2., y: 2.)].into()
        );
        // Surface with PolygonPatch, MultiCurve, Curve with LineStringSegment
        let g = parse_str(r#"<gml:Surface xmlns:gml="http://www.opengis.net/gml/3.2" gml:id="s"><gml:patches><gml:PolygonPatch><gml:exterior><gml:LinearRing><gml:posList>0 0 1 0 1 1 0 0</gml:posList></gml:LinearRing></gml:exterior></gml:PolygonPatch></gml:patches></gml:Surface>"#).unwrap();
        assert_eq!(
            g.geometry,
            polygon![(x: 0., y: 0.), (x: 1., y: 0.), (x: 1., y: 1.), (x: 0., y: 0.)].into()
        );
        let g = parse_str(r#"<gml:Curve xmlns:gml="http://www.opengis.net/gml"><gml:segments><gml:LineStringSegment><gml:posList>0 0 3 3</gml:posList></gml:LineStringSegment></gml:segments></gml:Curve>"#).unwrap();
        assert_eq!(
            g.geometry,
            line_string![(x: 0., y: 0.), (x: 3., y: 3.)].into()
        );
        // 3D CRS without srsDimension
        let g = parse_str(r#"<gml:LineString xmlns:gml="http://www.opengis.net/gml" srsName="urn:ogc:def:crs:EPSG::4979"><gml:posList>46.074 9.799 600.2 46.652 10.466 781.4</gml:posList></gml:LineString>"#).unwrap();
        assert_eq!(
            g.geometry,
            line_string![(x: 46.074, y: 9.799), (x: 46.652, y: 10.466)].into()
        );
        // Envelope as geometry -> polygon
        let g = parse_str(r#"<gml:Envelope xmlns:gml="http://www.opengis.net/gml" srsName="EPSG:4326"><gml:lowerCorner>0 0</gml:lowerCorner><gml:upperCorner>1 2</gml:upperCorner></gml:Envelope>"#).unwrap();
        assert_eq!(
            g.geometry,
            Geometry::Polygon(Rect::new((0., 0.), (1., 2.)).to_polygon())
        );
        assert!(matches!(
            parse_str(r#"<gml:Foo xmlns:gml="http://www.opengis.net/gml"/>"#),
            Err(GmlError::Unsupported(_))
        ));
    }

    #[test]
    fn envelope_box() {
        let doc = roxmltree::Document::parse(r#"<gml:Box xmlns:gml="http://www.opengis.net/gml" srsName="EPSG:4326"><gml:coordinates>-10,-20 10,20</gml:coordinates></gml:Box>"#).unwrap();
        let (rect, srs) = parse_envelope(doc.root_element()).unwrap();
        assert_eq!(
            rect,
            Rect::new(Coord { x: -10., y: -20. }, Coord { x: 10., y: 20. })
        );
        assert_eq!(srs.as_deref(), Some("EPSG:4326"));
    }

    #[test]
    fn write_envelope_versions() {
        let rect = Rect::new((0., 1.), (2., 3.));
        let mut s = String::new();
        write_envelope(&mut s, &rect, &opts(GmlVersion::V2));
        assert_eq!(
            s,
            r#"<gml:Box srsName="EPSG:4326"><gml:coordinates decimal="." cs="," ts=" ">0,1 2,3</gml:coordinates></gml:Box>"#
        );
        let mut s = String::new();
        write_envelope(&mut s, &rect, &opts(GmlVersion::V32));
        assert_eq!(
            s,
            r#"<gml:Envelope srsName="EPSG:4326"><gml:lowerCorner>0 1</gml:lowerCorner><gml:upperCorner>2 3</gml:upperCorner></gml:Envelope>"#
        );
    }
}
