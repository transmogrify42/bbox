//! WKB and GeoPackage geometry blob codec.
//!
//! Geometries are 2D `geo` geometries. Z ordinates are carried separately as a flat list in
//! coordinate traversal order (points, line vertices, exterior then interior rings, members in
//! order); M ordinates are dropped.

use geo::{
    BoundingRect, Coord, Geometry, GeometryCollection, LineString, MultiLineString, MultiPoint,
    MultiPolygon, Point, Polygon,
};

#[derive(thiserror::Error, Debug, PartialEq)]
pub enum WkbError {
    #[error("invalid WKB: {0}")]
    Invalid(String),
}

/// Decode (ISO or EWKB) WKB
pub fn decode_wkb(wkb: &[u8]) -> Result<Geometry<f64>, WkbError> {
    decode_wkb_z(wkb).map(|(g, _)| g)
}

/// Decode (ISO or EWKB) WKB with Z ordinates (None for 2D geometries)
pub fn decode_wkb_z(wkb: &[u8]) -> Result<(Geometry<f64>, Option<Vec<f64>>), WkbError> {
    let mut reader = Reader {
        data: wkb,
        pos: 0,
        z: Vec::new(),
        has_z: false,
    };
    let g = reader.geometry()?;
    Ok((g, reader.has_z.then_some(reader.z)))
}

/// Encode as little endian ISO WKB
pub fn encode_wkb(geom: &Geometry<f64>) -> Vec<u8> {
    encode_wkb_z(geom, None)
}

/// Encode as little endian ISO WKB, with Z ordinates if given
pub fn encode_wkb_z(geom: &Geometry<f64>, z: Option<&[f64]>) -> Vec<u8> {
    let mut out = Vec::new();
    Writer {
        out: &mut out,
        z,
        zi: 0,
    }
    .geometry(geom);
    out
}

/// Decode GeoPackage geometry blob. Returns None for empty geometries.
pub fn decode_gpkg(blob: &[u8]) -> Result<Option<Geometry<f64>>, WkbError> {
    decode_gpkg_z(blob).map(|g| g.map(|(g, _)| g))
}

/// Decode GeoPackage geometry blob with Z ordinates. Returns None for empty geometries.
/// Geometry with optional Z ordinates in coordinate traversal order
pub type GeometryZ = (Geometry<f64>, Option<Vec<f64>>);

pub fn decode_gpkg_z(blob: &[u8]) -> Result<Option<GeometryZ>, WkbError> {
    if blob.len() < 8 || &blob[0..2] != b"GP" {
        return Err(WkbError::Invalid("missing GeoPackage header".into()));
    }
    let flags = blob[3];
    if flags & 0b0001_0000 != 0 {
        return Ok(None);
    }
    let envelope_len = match (flags >> 1) & 0b111 {
        0 => 0,
        1 => 32,
        2 | 3 => 48,
        4 => 64,
        e => return Err(WkbError::Invalid(format!("invalid envelope indicator {e}"))),
    };
    let start = 8 + envelope_len;
    if blob.len() < start {
        return Err(WkbError::Invalid("truncated GeoPackage header".into()));
    }
    decode_wkb_z(&blob[start..]).map(Some)
}

/// Encode GeoPackage geometry blob with envelope
pub fn encode_gpkg(geom: &Geometry<f64>, srs_id: i32) -> Vec<u8> {
    encode_gpkg_z(geom, None, srs_id)
}

/// Encode GeoPackage geometry blob with envelope, with Z ordinates if given
pub fn encode_gpkg_z(geom: &Geometry<f64>, z: Option<&[f64]>, srs_id: i32) -> Vec<u8> {
    let mut out = vec![b'G', b'P', 0];
    let rect = geom.bounding_rect();
    // little endian, envelope [minx, maxx, miny, maxy] if non-empty
    let flags: u8 = if rect.is_some() {
        0b0000_0011
    } else {
        0b0001_0001
    };
    out.push(flags);
    out.extend(srs_id.to_le_bytes());
    if let Some(r) = rect {
        for v in [r.min().x, r.max().x, r.min().y, r.max().y] {
            out.extend(v.to_le_bytes());
        }
    }
    Writer {
        out: &mut out,
        z,
        zi: 0,
    }
    .geometry(geom);
    out
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    z: Vec<f64>,
    has_z: bool,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], WkbError> {
        let bytes = self
            .data
            .get(self.pos..self.pos + N)
            .ok_or_else(|| WkbError::Invalid("unexpected end of data".into()))?;
        self.pos += N;
        Ok(bytes.try_into().expect("slice length"))
    }
    fn u32(&mut self, le: bool) -> Result<u32, WkbError> {
        let b = self.take::<4>()?;
        Ok(if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }
    fn f64(&mut self, le: bool) -> Result<f64, WkbError> {
        let b = self.take::<8>()?;
        Ok(if le {
            f64::from_le_bytes(b)
        } else {
            f64::from_be_bytes(b)
        })
    }
    /// Read a coordinate. `dims`: number of ordinates, `z`: third ordinate is Z
    fn coord(&mut self, le: bool, dims: usize, z: bool) -> Result<Coord<f64>, WkbError> {
        let x = self.f64(le)?;
        let y = self.f64(le)?;
        for i in 2..dims {
            let v = self.f64(le)?;
            if i == 2 && z {
                self.z.push(v);
            }
        }
        Ok(Coord { x, y })
    }
    fn coords(&mut self, le: bool, dims: usize, z: bool) -> Result<Vec<Coord<f64>>, WkbError> {
        let n = self.u32(le)? as usize;
        if n > self.data.len() {
            return Err(WkbError::Invalid("invalid coordinate count".into()));
        }
        (0..n).map(|_| self.coord(le, dims, z)).collect()
    }
    fn polygon(&mut self, le: bool, dims: usize, z: bool) -> Result<Polygon<f64>, WkbError> {
        let n = self.u32(le)? as usize;
        if n == 0 {
            return Ok(Polygon::new(LineString(vec![]), vec![]));
        }
        let exterior = LineString(self.coords(le, dims, z)?);
        let interiors = (1..n)
            .map(|_| self.coords(le, dims, z).map(LineString))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Polygon::new(exterior, interiors))
    }
    fn geometry(&mut self) -> Result<Geometry<f64>, WkbError> {
        let le = match self.take::<1>()?[0] {
            0 => false,
            1 => true,
            b => return Err(WkbError::Invalid(format!("invalid byte order {b}"))),
        };
        let raw = self.u32(le)?;
        // EWKB flags
        let ewkb_z = raw & 0x8000_0000 != 0;
        let ewkb_m = raw & 0x4000_0000 != 0;
        if raw & 0x2000_0000 != 0 {
            self.u32(le)?; // SRID
        }
        let code = raw & 0x0FFF_FFFF;
        let (base, iso_dims) = (code % 1000, code / 1000);
        let (dims, z) = match iso_dims {
            0 => (2 + ewkb_z as usize + ewkb_m as usize, ewkb_z),
            1 => (3, true),
            2 => (3, false),
            3 => (4, true),
            _ => return Err(WkbError::Invalid(format!("invalid geometry type {raw}"))),
        };
        self.has_z |= z;
        let multi = |this: &mut Self| -> Result<Vec<Geometry<f64>>, WkbError> {
            let n = this.u32(le)? as usize;
            if n > this.data.len() {
                return Err(WkbError::Invalid("invalid member count".into()));
            }
            (0..n).map(|_| this.geometry()).collect()
        };
        Ok(match base {
            1 => Geometry::Point(Point(self.coord(le, dims, z)?)),
            2 => Geometry::LineString(LineString(self.coords(le, dims, z)?)),
            3 => Geometry::Polygon(self.polygon(le, dims, z)?),
            4 => Geometry::MultiPoint(MultiPoint(
                multi(self)?
                    .into_iter()
                    .map(|g| match g {
                        Geometry::Point(p) => Ok(p),
                        _ => Err(WkbError::Invalid("multipoint member".into())),
                    })
                    .collect::<Result<_, _>>()?,
            )),
            5 => Geometry::MultiLineString(MultiLineString(
                multi(self)?
                    .into_iter()
                    .map(|g| match g {
                        Geometry::LineString(l) => Ok(l),
                        _ => Err(WkbError::Invalid("multilinestring member".into())),
                    })
                    .collect::<Result<_, _>>()?,
            )),
            6 => Geometry::MultiPolygon(MultiPolygon(
                multi(self)?
                    .into_iter()
                    .map(|g| match g {
                        Geometry::Polygon(p) => Ok(p),
                        _ => Err(WkbError::Invalid("multipolygon member".into())),
                    })
                    .collect::<Result<_, _>>()?,
            )),
            7 => Geometry::GeometryCollection(GeometryCollection(multi(self)?)),
            // SQL/MM curves are linearized
            8 => {
                let mark = self.z.len();
                let coords = self.coords(le, dims, z)?;
                let zs = self.z.split_off(mark);
                let (coords, zs) = linearize_arcs(&coords, z.then_some(zs.as_slice()));
                self.z.extend(zs);
                Geometry::LineString(LineString(coords))
            }
            9 => {
                let n = self.u32(le)? as usize;
                if n > self.data.len() {
                    return Err(WkbError::Invalid("invalid member count".into()));
                }
                let mut coords: Vec<Coord<f64>> = Vec::new();
                let mut zs = Vec::new();
                for _ in 0..n {
                    let mark = self.z.len();
                    let Geometry::LineString(ls) = self.geometry()? else {
                        return Err(WkbError::Invalid("compound curve member".into()));
                    };
                    let mz = self.z.split_off(mark);
                    for (i, c) in ls.0.into_iter().enumerate() {
                        // joint vertex of consecutive members
                        if i == 0 && coords.last() == Some(&c) {
                            continue;
                        }
                        coords.push(c);
                        if let Some(v) = mz.get(i) {
                            zs.push(*v);
                        }
                    }
                }
                self.z.extend(zs);
                Geometry::LineString(LineString(coords))
            }
            10 => {
                let rings = multi(self)?
                    .into_iter()
                    .map(|g| match g {
                        Geometry::LineString(l) => Ok(l),
                        _ => Err(WkbError::Invalid("curve polygon ring".into())),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let mut rings = rings.into_iter();
                match rings.next() {
                    Some(ext) => Geometry::Polygon(Polygon::new(ext, rings.collect())),
                    None => Geometry::Polygon(Polygon::new(LineString(vec![]), vec![])),
                }
            }
            11 => Geometry::MultiLineString(MultiLineString(
                multi(self)?
                    .into_iter()
                    .map(|g| match g {
                        Geometry::LineString(l) => Ok(l),
                        _ => Err(WkbError::Invalid("multicurve member".into())),
                    })
                    .collect::<Result<_, _>>()?,
            )),
            // MultiSurface, PolyhedralSurface, TIN
            12 | 15 | 16 => Geometry::MultiPolygon(MultiPolygon(
                multi(self)?
                    .into_iter()
                    .map(|g| match g {
                        Geometry::Polygon(p) => Ok(p),
                        _ => Err(WkbError::Invalid("surface member".into())),
                    })
                    .collect::<Result<_, _>>()?,
            )),
            17 => Geometry::Polygon(self.polygon(le, dims, z)?),
            t => return Err(WkbError::Invalid(format!("unsupported geometry type {t}"))),
        })
    }
}

/// Maximum angle of a linearized arc segment
const ARC_STEP: f64 = std::f64::consts::TAU / 128.0;

/// Linearize a circular string (sequence of arcs p0 p1 p2, p2 p3 p4, ..). Z ordinates are
/// interpolated piecewise along each arc.
fn linearize_arcs(points: &[Coord<f64>], z: Option<&[f64]>) -> (Vec<Coord<f64>>, Vec<f64>) {
    let mut out = Vec::new();
    let mut zout = Vec::new();
    let zat = |i: usize| z.and_then(|z| z.get(i).copied());
    if let Some(p) = points.first() {
        out.push(*p);
        if let Some(v) = zat(0) {
            zout.push(v);
        }
    }
    let mut i = 0;
    while i + 2 < points.len() {
        let (p0, p1, p2) = (points[i], points[i + 1], points[i + 2]);
        let (z0, z1, z2) = (zat(i), zat(i + 1), zat(i + 2));
        let mut push = |c: Coord<f64>, zv: Option<f64>| {
            out.push(c);
            if let Some(v) = zv {
                zout.push(v);
            }
        };
        match arc_geometry(p0, p1, p2) {
            None => {
                // collinear: straight segments
                push(p1, z1);
                push(p2, z2);
            }
            Some((center, r, a0, sweep, f1)) => {
                let n = ((sweep.abs() / ARC_STEP).ceil() as usize).max(2);
                for k in 1..=n {
                    let t = k as f64 / n as f64;
                    let c = if k == n {
                        p2
                    } else {
                        let a = a0 + sweep * t;
                        Coord {
                            x: center.x + r * a.cos(),
                            y: center.y + r * a.sin(),
                        }
                    };
                    let zv = match (z0, z1, z2) {
                        (Some(z0), Some(z1), Some(z2)) => Some(if t <= f1 {
                            z0 + (z1 - z0) * (t / f1)
                        } else {
                            z1 + (z2 - z1) * ((t - f1) / (1.0 - f1))
                        }),
                        _ => None,
                    };
                    push(c, zv);
                }
            }
        }
        i += 2;
    }
    // trailing point without complete arc
    if i + 1 < points.len() {
        for (j, p) in points.iter().enumerate().skip(i + 1) {
            out.push(*p);
            if let Some(v) = zat(j) {
                zout.push(v);
            }
        }
    }
    (out, zout)
}

/// Circle through three points: (center, radius, start angle, signed sweep, fraction of the
/// sweep at the middle point). None for collinear points.
fn arc_geometry(
    p0: Coord<f64>,
    p1: Coord<f64>,
    p2: Coord<f64>,
) -> Option<(Coord<f64>, f64, f64, f64, f64)> {
    use std::f64::consts::TAU;
    let norm = |a: f64| a.rem_euclid(TAU);
    if p0 == p2 {
        // full circle, p1 diametrically opposite
        let center = Coord {
            x: (p0.x + p1.x) / 2.0,
            y: (p0.y + p1.y) / 2.0,
        };
        let r = ((p0.x - center.x).powi(2) + (p0.y - center.y).powi(2)).sqrt();
        if r == 0.0 {
            return None;
        }
        let a0 = (p0.y - center.y).atan2(p0.x - center.x);
        return Some((center, r, a0, TAU, 0.5));
    }
    let d = 2.0 * (p0.x * (p1.y - p2.y) + p1.x * (p2.y - p0.y) + p2.x * (p0.y - p1.y));
    let scale = (p0.x.abs() + p0.y.abs() + p2.x.abs() + p2.y.abs()).max(1.0);
    if d.abs() < 1e-12 * scale * scale {
        return None;
    }
    let sq = |c: Coord<f64>| c.x * c.x + c.y * c.y;
    let center = Coord {
        x: (sq(p0) * (p1.y - p2.y) + sq(p1) * (p2.y - p0.y) + sq(p2) * (p0.y - p1.y)) / d,
        y: (sq(p0) * (p2.x - p1.x) + sq(p1) * (p0.x - p2.x) + sq(p2) * (p1.x - p0.x)) / d,
    };
    let r = ((p0.x - center.x).powi(2) + (p0.y - center.y).powi(2)).sqrt();
    let angle = |p: Coord<f64>| (p.y - center.y).atan2(p.x - center.x);
    let (a0, a1, a2) = (angle(p0), angle(p1), angle(p2));
    let ccw_total = norm(a2 - a0);
    let ccw_mid = norm(a1 - a0);
    let (sweep, mid) = if ccw_mid < ccw_total {
        (ccw_total, ccw_mid)
    } else {
        (ccw_total - TAU, ccw_mid - TAU)
    };
    Some((center, r, a0, sweep, mid / sweep))
}

struct Writer<'a> {
    out: &'a mut Vec<u8>,
    z: Option<&'a [f64]>,
    zi: usize,
}

impl Writer<'_> {
    fn coord(&mut self, c: &Coord<f64>) {
        self.out.extend(c.x.to_le_bytes());
        self.out.extend(c.y.to_le_bytes());
        if let Some(z) = self.z {
            let v = z.get(self.zi).copied().unwrap_or(0.0);
            self.zi += 1;
            self.out.extend(v.to_le_bytes());
        }
    }
    fn coords(&mut self, coords: &[Coord<f64>]) {
        self.out.extend((coords.len() as u32).to_le_bytes());
        for c in coords {
            self.coord(c);
        }
    }
    fn polygon_body(&mut self, p: &Polygon<f64>) {
        let rings = 1 + p.interiors().len();
        self.out.extend((rings as u32).to_le_bytes());
        self.coords(&p.exterior().0);
        for r in p.interiors() {
            self.coords(&r.0);
        }
    }
    fn header(&mut self, code: u32) {
        self.out.push(1);
        // ISO WKB Z type codes: 1000 + base type
        let code = if self.z.is_some() { code + 1000 } else { code };
        self.out.extend(code.to_le_bytes());
    }
    fn geometry(&mut self, geom: &Geometry<f64>) {
        match geom {
            Geometry::Point(p) => {
                self.header(1);
                self.coord(&p.0);
            }
            Geometry::Line(l) => self.geometry(&Geometry::LineString(l.into())),
            Geometry::LineString(ls) => {
                self.header(2);
                self.coords(&ls.0);
            }
            Geometry::Polygon(p) => {
                self.header(3);
                self.polygon_body(p);
            }
            Geometry::Rect(r) => self.geometry(&Geometry::Polygon(r.to_polygon())),
            Geometry::Triangle(t) => self.geometry(&Geometry::Polygon(t.to_polygon())),
            Geometry::MultiPoint(mp) => {
                self.header(4);
                self.out.extend((mp.0.len() as u32).to_le_bytes());
                for p in &mp.0 {
                    self.geometry(&Geometry::Point(*p));
                }
            }
            Geometry::MultiLineString(mls) => {
                self.header(5);
                self.out.extend((mls.0.len() as u32).to_le_bytes());
                for ls in &mls.0 {
                    self.header(2);
                    self.coords(&ls.0);
                }
            }
            Geometry::MultiPolygon(mp) => {
                self.header(6);
                self.out.extend((mp.0.len() as u32).to_le_bytes());
                for p in &mp.0 {
                    self.header(3);
                    self.polygon_body(p);
                }
            }
            Geometry::GeometryCollection(gc) => {
                self.header(7);
                self.out.extend((gc.0.len() as u32).to_le_bytes());
                for g in &gc.0 {
                    self.geometry(g);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{line_string, point, polygon};

    fn samples() -> Vec<Geometry<f64>> {
        let poly = polygon!(
            exterior: [(x: 0., y: 0.), (x: 10., y: 0.), (x: 10., y: 10.), (x: 0., y: 0.)],
            interiors: [[(x: 1., y: 1.), (x: 2., y: 1.), (x: 2., y: 2.), (x: 1., y: 1.)]],
        );
        vec![
            point!(x: 1.5, y: -2.25).into(),
            line_string![(x: 0., y: 0.), (x: 1., y: 1.), (x: 2., y: 0.)].into(),
            poly.clone().into(),
            MultiPoint(vec![point!(x: 1., y: 2.), point!(x: 3., y: 4.)]).into(),
            MultiLineString(vec![line_string![(x: 0., y: 0.), (x: 1., y: 1.)]]).into(),
            MultiPolygon(vec![poly.clone(), poly]).into(),
            Geometry::GeometryCollection(GeometryCollection(vec![
                point!(x: 1., y: 2.).into(),
                line_string![(x: 0., y: 0.), (x: 1., y: 1.)].into(),
            ])),
        ]
    }

    #[test]
    fn wkb_roundtrip() {
        for g in samples() {
            assert_eq!(decode_wkb(&encode_wkb(&g)).unwrap(), g);
        }
    }

    #[test]
    fn point_wkb_bytes() {
        let wkb = encode_wkb(&point!(x: 1.0, y: 2.0).into());
        assert_eq!(wkb[0], 1); // little endian
        assert_eq!(&wkb[1..5], &1u32.to_le_bytes());
        assert_eq!(&wkb[5..13], &1.0f64.to_le_bytes());
        assert_eq!(wkb.len(), 21);
    }

    #[test]
    fn decode_big_endian_and_z() {
        // big endian point
        let mut wkb = vec![0u8];
        wkb.extend(1u32.to_be_bytes());
        wkb.extend(3.0f64.to_be_bytes());
        wkb.extend(4.0f64.to_be_bytes());
        assert_eq!(decode_wkb(&wkb).unwrap(), point!(x: 3., y: 4.).into());
        // ISO Point Z (1001)
        let mut wkb = vec![1u8];
        wkb.extend(1001u32.to_le_bytes());
        for v in [1.0f64, 2.0, 3.0] {
            wkb.extend(v.to_le_bytes());
        }
        assert_eq!(decode_wkb(&wkb).unwrap(), point!(x: 1., y: 2.).into());
        // EWKB Point with SRID and Z flag
        let mut wkb = vec![1u8];
        wkb.extend((1u32 | 0x8000_0000 | 0x2000_0000).to_le_bytes());
        wkb.extend(4326u32.to_le_bytes());
        for v in [5.0f64, 6.0, 7.0] {
            wkb.extend(v.to_le_bytes());
        }
        assert_eq!(decode_wkb(&wkb).unwrap(), point!(x: 5., y: 6.).into());
        assert!(decode_wkb(&[1, 2]).is_err());
    }

    #[test]
    fn z_roundtrip() {
        for g in samples() {
            let n = geo::CoordsIter::coords_count(&g);
            let z: Vec<f64> = (0..n).map(|i| i as f64 * 1.5).collect();
            let wkb = encode_wkb_z(&g, Some(&z));
            assert_eq!(decode_wkb_z(&wkb).unwrap(), (g.clone(), Some(z.clone())));
            let blob = encode_gpkg_z(&g, Some(&z), 4979);
            assert_eq!(decode_gpkg_z(&blob).unwrap(), Some((g.clone(), Some(z))));
            // 2D stays 2D
            assert_eq!(decode_wkb_z(&encode_wkb(&g)).unwrap(), (g, None));
        }
        // ISO ZM (3001): Z kept, M dropped; ISO M (2001): no Z
        let mut wkb = vec![1u8];
        wkb.extend(3001u32.to_le_bytes());
        for v in [1.0f64, 2.0, 3.0, 4.0] {
            wkb.extend(v.to_le_bytes());
        }
        assert_eq!(
            decode_wkb_z(&wkb).unwrap(),
            (point!(x: 1., y: 2.).into(), Some(vec![3.0]))
        );
        let mut wkb = vec![1u8];
        wkb.extend(2001u32.to_le_bytes());
        for v in [1.0f64, 2.0, 4.0] {
            wkb.extend(v.to_le_bytes());
        }
        assert_eq!(
            decode_wkb_z(&wkb).unwrap(),
            (point!(x: 1., y: 2.).into(), None)
        );
    }

    /// Raw ISO WKB builder for tests
    fn raw(code: u32, body: &[u8]) -> Vec<u8> {
        let mut w = vec![1u8];
        w.extend(code.to_le_bytes());
        w.extend_from_slice(body);
        w
    }
    fn pts(coords: &[&[f64]]) -> Vec<u8> {
        let mut b = (coords.len() as u32).to_le_bytes().to_vec();
        for c in coords {
            for v in *c {
                b.extend(v.to_le_bytes());
            }
        }
        b
    }
    fn parts(members: &[Vec<u8>]) -> Vec<u8> {
        let mut b = (members.len() as u32).to_le_bytes().to_vec();
        for m in members {
            b.extend(m);
        }
        b
    }

    #[test]
    fn curves_are_linearized() {
        // half circle (0,0) -> (1,1) -> (2,0) around (1,0), radius 1
        let arc = raw(8, &pts(&[&[0., 0.], &[1., 1.], &[2., 0.]]));
        let Geometry::LineString(ls) = decode_wkb(&arc).unwrap() else {
            panic!()
        };
        assert!(ls.0.len() > 10, "{}", ls.0.len());
        assert_eq!(ls.0.first(), Some(&Coord { x: 0., y: 0. }));
        assert_eq!(ls.0.last(), Some(&Coord { x: 2., y: 0. }));
        for c in &ls.0 {
            let r = ((c.x - 1.).powi(2) + c.y.powi(2)).sqrt();
            assert!((r - 1.).abs() < 1e-9, "{c:?}");
            assert!(c.y >= -1e-12, "{c:?}");
        }
        // passes through the middle point
        assert!(ls
            .0
            .iter()
            .any(|c| (c.x - 1.).abs() < 1e-9 && (c.y - 1.).abs() < 1e-9));
        // collinear arc points: straight line
        let line = raw(8, &pts(&[&[0., 0.], &[1., 1.], &[2., 2.]]));
        assert_eq!(
            decode_wkb(&line).unwrap(),
            Geometry::LineString(line_string![(x: 0., y: 0.), (x: 1., y: 1.), (x: 2., y: 2.)])
        );
        // Z interpolated along the arc
        let arc_z = raw(
            1008,
            &pts(&[&[0., 0., 10.], &[1., 1., 20.], &[2., 0., 30.]]),
        );
        let (g, z) = decode_wkb_z(&arc_z).unwrap();
        let z = z.unwrap();
        assert_eq!(z.len(), geo::CoordsIter::coords_count(&g));
        assert_eq!((z[0], *z.last().unwrap()), (10., 30.));
        assert!(z.windows(2).all(|w| w[1] >= w[0]));
        // compound curve: line + arc, joint vertex not duplicated
        let compound = raw(
            9,
            &parts(&[raw(2, &pts(&[&[-1., 0.], &[0., 0.]])), arc.clone()]),
        );
        let Geometry::LineString(cl) = decode_wkb(&compound).unwrap() else {
            panic!()
        };
        assert_eq!(cl.0.len(), ls.0.len() + 1);
        assert_eq!(cl.0[0], Coord { x: -1., y: 0. });
        // curve polygon with a closed compound ring
        let closing = raw(2, &pts(&[&[2., 0.], &[0., 0.]]));
        let ring = raw(9, &parts(&[arc.clone(), closing]));
        let cp = raw(10, &parts(&[ring]));
        let Geometry::Polygon(p) = decode_wkb(&cp).unwrap() else {
            panic!()
        };
        assert_eq!(p.exterior().0.first(), p.exterior().0.last());
        // multicurve, multisurface, triangle, TIN
        let Geometry::MultiLineString(mc) = decode_wkb(&raw(
            11,
            &parts(&[arc.clone(), raw(2, &pts(&[&[5., 5.], &[6., 6.]]))]),
        ))
        .unwrap() else {
            panic!()
        };
        assert_eq!(mc.0.len(), 2);
        let tri = raw(
            17,
            &parts_rings(&[&[&[0., 0.], &[1., 0.], &[0., 1.], &[0., 0.]]]),
        );
        let Geometry::Polygon(_) = decode_wkb(&tri).unwrap() else {
            panic!()
        };
        let Geometry::MultiPolygon(ms) = decode_wkb(&raw(12, &parts(&[cp, tri.clone()]))).unwrap()
        else {
            panic!()
        };
        assert_eq!(ms.0.len(), 2);
        let Geometry::MultiPolygon(tin) =
            decode_wkb(&raw(16, &parts(&[tri.clone(), tri]))).unwrap()
        else {
            panic!()
        };
        assert_eq!(tin.0.len(), 2);
    }

    fn parts_rings(rings: &[&[&[f64]]]) -> Vec<u8> {
        let mut b = (rings.len() as u32).to_le_bytes().to_vec();
        for r in rings {
            b.extend(pts(r));
        }
        b
    }

    #[test]
    fn gpkg_roundtrip() {
        for g in samples() {
            let blob = encode_gpkg(&g, 32615);
            assert_eq!(&blob[0..2], b"GP");
            assert_eq!(&blob[4..8], &32615i32.to_le_bytes());
            assert_eq!(decode_gpkg(&blob).unwrap(), Some(g));
        }
    }

    #[test]
    fn gpkg_from_sample_file() {
        // blob as written by GDAL in assets/ne_extracts.gpkg (point, no envelope)
        let mut blob = vec![b'G', b'P', 0, 0b0000_0001];
        blob.extend(4326i32.to_le_bytes());
        blob.extend(encode_wkb(&point!(x: 7.0, y: 8.0).into()));
        assert_eq!(
            decode_gpkg(&blob).unwrap(),
            Some(point!(x: 7., y: 8.).into())
        );
        // empty flag
        let blob = vec![b'G', b'P', 0, 0b0001_0001, 0, 0, 0, 0];
        assert_eq!(decode_gpkg(&blob).unwrap(), None);
        assert!(decode_gpkg(b"XX").is_err());
    }
}
