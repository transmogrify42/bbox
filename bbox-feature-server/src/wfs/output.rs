//! Feature encoding (GML 2.1.2, 3.1.1, 3.2).

use crate::wfs::crs::{self, Crs};
use crate::wfs::gml::{write_envelope, write_geometry, GmlVersion, GmlWriteOpts};
use crate::wfs::model::*;
use crate::wfs::xml::escape;
use geo::{BoundingRect, Geometry, Rect};

/// Output CRS of geometries
#[derive(Clone, Debug)]
pub struct OutputCrs {
    pub epsg: u16,
    /// srsName attribute value
    pub name: String,
    pub swap_xy: bool,
}

impl OutputCrs {
    pub fn new(crs: Crs, name: Option<&str>) -> Self {
        OutputCrs {
            epsg: crs.epsg,
            name: name.map(str::to_string).unwrap_or_else(|| crs.name()),
            swap_xy: crs.swap_xy(),
        }
    }
}

/// Settings for encoding features of one feature type
pub struct FeatureEncoder<'a> {
    pub def: &'a FeatureTypeDef,
    pub gml: GmlVersion,
    pub crs: OutputCrs,
    /// Properties to emit (None: all)
    pub properties: Option<Vec<usize>>,
    /// Emit gml:boundedBy for each feature (computed if not stored)
    pub feature_bounding: bool,
}

impl FeatureEncoder<'_> {
    fn emit_property(&self, idx: usize) -> bool {
        match &self.properties {
            None => true,
            Some(props) => props.contains(&idx) || self.def.properties[idx].min_occurs > 0,
        }
    }

    fn element_name(&self, p: &PropertyDef) -> String {
        if self.gml == GmlVersion::V2 && p.is_mapped_gml_column() {
            format!("{}:{}", self.def.name.prefix, p.name.local)
        } else if p.name.is_gml() {
            format!("gml:{}", p.name.local)
        } else {
            p.name.prefixed()
        }
    }

    /// Transform geometry from native CRS into output CRS (x/y order)
    pub fn transform(&self, g: &Geometry<f64>) -> Option<Geometry<f64>> {
        let mut g = g.clone();
        if self.crs.epsg != self.def.srid {
            crs::transform(&mut g, self.def.srid, self.crs.epsg).ok()?;
        }
        Some(g)
    }

    fn write_envelope(&self, out: &mut String, rect: &Rect<f64>, z: Option<[f64; 2]>) {
        let rect = if self.crs.epsg != self.def.srid {
            match self
                .transform(&Geometry::Polygon(rect.to_polygon()))
                .and_then(|g| g.bounding_rect())
            {
                Some(r) => r,
                None => return,
            }
        } else {
            *rect
        };
        let opts = GmlWriteOpts {
            version: self.gml,
            srs_name: Some(&self.crs.name),
            swap_xy: self.crs.swap_xy,
            id: None,
            meta: None,
            z: z.as_ref().map(|z| z.as_slice()),
        };
        write_envelope(out, &rect, &opts);
    }

    /// Write a geometry (already in output CRS) with optional gml:id
    pub fn write_geometry(&self, out: &mut String, g: &Geometry<f64>, id: Option<&str>) {
        self.write_geometry_meta(out, g, id, None)
    }

    /// Write a geometry with gml:id and metadata elements
    pub fn write_geometry_meta(
        &self,
        out: &mut String,
        g: &Geometry<f64>,
        id: Option<&str>,
        meta: Option<&str>,
    ) {
        self.write_geometry_z(out, g, id, meta, None)
    }

    /// Write a geometry with gml:id, metadata elements and Z ordinates
    pub fn write_geometry_z(
        &self,
        out: &mut String,
        g: &Geometry<f64>,
        id: Option<&str>,
        meta: Option<&str>,
        z: Option<&[f64]>,
    ) {
        let opts = GmlWriteOpts {
            version: self.gml,
            srs_name: Some(&self.crs.name),
            swap_xy: self.crs.swap_xy,
            id,
            meta,
            z,
        };
        write_geometry(out, g, &opts);
    }

    /// Encode a feature element
    pub fn write_feature(&self, out: &mut String, f: &Feature) {
        let name = self.def.name.prefixed();
        let id = escape(&f.id);
        match self.gml {
            GmlVersion::V2 => out.push_str(&format!("<{name} fid=\"{id}\">")),
            _ => out.push_str(&format!("<{name} gml:id=\"{id}\">")),
        }
        let mut bounded_written = false;
        let gml2 = self.gml == GmlVersion::V2;
        let has_bounded_prop = self.def.properties.iter().any(|p| {
            p.name.is_gml() && p.name.local == "boundedBy" && !(gml2 && p.is_mapped_gml_column())
        });
        if !has_bounded_prop && self.feature_bounding {
            if let Some(r) = f.bbox() {
                out.push_str("<gml:boundedBy>");
                self.write_envelope(out, &r, None);
                out.push_str("</gml:boundedBy>");
            }
        }
        for (i, p) in self.def.properties.iter().enumerate() {
            let value = &f.values[i];
            let is_std = p.name.is_gml();
            if is_std && p.name.local == "identifier" && self.gml != GmlVersion::V32 {
                continue;
            }
            if is_std && p.name.local == "boundedBy" && !(gml2 && p.is_mapped_gml_column()) {
                // boundedBy follows description/name
                if let Value::Geometry(g) = value {
                    if let Some(r) = g.bbox() {
                        out.push_str("<gml:boundedBy>");
                        self.write_envelope(out, &r, g.z_range());
                        out.push_str("</gml:boundedBy>");
                        bounded_written = true;
                    }
                } else if self.feature_bounding {
                    if let Some(r) = f.bbox() {
                        out.push_str("<gml:boundedBy>");
                        self.write_envelope(out, &r, None);
                        out.push_str("</gml:boundedBy>");
                        bounded_written = true;
                    }
                }
                continue;
            }
            if !self.emit_property(i) {
                continue;
            }
            let el = self.element_name(p);
            match value {
                Value::Null => {
                    if p.nillable && p.min_occurs > 0 {
                        out.push_str(&format!("<{el} xsi:nil=\"true\"/>"));
                    }
                }
                Value::Xml(xml) => out.push_str(xml),
                Value::Geometry(g) => {
                    let Some(geom) = self.transform(&g.geometry) else {
                        continue;
                    };
                    out.push('<');
                    out.push_str(&el);
                    out.push('>');
                    let gid = match (&g.gml_id, self.gml) {
                        (Some(id), _) => Some(id.clone()),
                        (None, GmlVersion::V32) => Some(format!("{}.{}", f.id, p.name.local)),
                        _ => None,
                    };
                    self.write_geometry_z(
                        out,
                        &geom,
                        gid.as_deref(),
                        g.meta.as_deref(),
                        g.z.as_deref(),
                    );
                    out.push_str("</");
                    out.push_str(&el);
                    out.push('>');
                }
                v => {
                    let text = v.lexical().unwrap_or_default();
                    out.push('<');
                    out.push_str(&el);
                    out.push('>');
                    out.push_str(&escape(&text));
                    out.push_str("</");
                    out.push_str(&el);
                    out.push('>');
                }
            }
        }
        let _ = bounded_written;
        out.push_str(&format!("</{name}>"));
    }
}
