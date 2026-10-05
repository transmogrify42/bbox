//! CRS identifiers, axis order and reprojection.

use geo::{Coord, Geometry};

/// Notation used for a CRS identifier
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrsNotation {
    /// `EPSG:4326` (traditional x/y = lon/lat axis order)
    Epsg,
    /// `http://www.opengis.net/gml/srs/epsg.xml#4326` (x/y axis order)
    EpsgXml,
    /// `urn:ogc:def:crs:EPSG::4326` (EPSG axis order)
    Urn,
    /// `urn:x-ogc:def:crs:EPSG:4326` (EPSG axis order)
    XUrn,
    /// `http://www.opengis.net/def/crs/EPSG/0/4326` (EPSG axis order)
    Http,
    /// `urn:ogc:def:crs:OGC:1.3:CRS84` / `http://www.opengis.net/def/crs/OGC/1.3/CRS84` (lon/lat)
    Crs84,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crs {
    pub epsg: u16,
    pub notation: CrsNotation,
}

#[derive(thiserror::Error, Debug, PartialEq)]
pub enum CrsError {
    #[error("unknown CRS `{0}`")]
    Unknown(String),
    #[error("transformation error: {0}")]
    Transform(String),
}

impl Crs {
    pub fn new(epsg: u16, notation: CrsNotation) -> Self {
        Crs { epsg, notation }
    }

    /// Parse a CRS identifier in any supported notation
    /// Parse a CRS identifier (EPSG codes in the usual notations, CRS84)
    pub fn parse(name: &str) -> Result<Crs, CrsError> {
        let unknown = || CrsError::Unknown(name.to_string());
        let code = |s: &str| {
            s.trim()
                .parse::<u32>()
                .ok()
                .and_then(epsg_alias)
                .ok_or_else(unknown)
        };
        let trimmed = name.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower.ends_with("crs84") {
            return Ok(Crs::new(4326, CrsNotation::Crs84));
        }
        if let Some(c) = lower.strip_prefix("epsg:") {
            return Ok(Crs::new(code(c)?, CrsNotation::Epsg));
        }
        if let Some(c) = lower.strip_prefix("http://www.opengis.net/gml/srs/epsg.xml#") {
            return Ok(Crs::new(code(c)?, CrsNotation::EpsgXml));
        }
        if let Some(rest) = lower.strip_prefix("urn:ogc:def:crs:epsg:") {
            // urn:ogc:def:crs:EPSG:{version}:{code}
            let c = rest.rsplit(':').next().ok_or_else(unknown)?;
            return Ok(Crs::new(code(c)?, CrsNotation::Urn));
        }
        if let Some(rest) = lower.strip_prefix("urn:x-ogc:def:crs:epsg:") {
            let c = rest.rsplit(':').next().ok_or_else(unknown)?;
            return Ok(Crs::new(code(c)?, CrsNotation::XUrn));
        }
        for prefix in [
            "http://www.opengis.net/def/crs/epsg/",
            "https://www.opengis.net/def/crs/epsg/",
        ] {
            if let Some(rest) = lower.strip_prefix(prefix) {
                // {version}/{code}
                let c = rest.rsplit('/').next().ok_or_else(unknown)?;
                return Ok(Crs::new(code(c)?, CrsNotation::Http));
            }
        }
        Err(unknown())
    }

    /// Identifier in this CRS' notation
    pub fn name(&self) -> String {
        let epsg = self.epsg;
        match self.notation {
            CrsNotation::Epsg => format!("EPSG:{epsg}"),
            CrsNotation::EpsgXml => format!("http://www.opengis.net/gml/srs/epsg.xml#{epsg}"),
            CrsNotation::Urn => format!("urn:ogc:def:crs:EPSG::{epsg}"),
            CrsNotation::XUrn => format!("urn:x-ogc:def:crs:EPSG:{epsg}"),
            CrsNotation::Http => format!("http://www.opengis.net/def/crs/EPSG/0/{epsg}"),
            CrsNotation::Crs84 => "http://www.opengis.net/def/crs/OGC/1.3/CRS84".to_string(),
        }
    }

    /// Whether coordinates in this notation are written in y/x (lat/lon, northing/easting) order
    pub fn swap_xy(&self) -> bool {
        match self.notation {
            CrsNotation::Epsg | CrsNotation::EpsgXml | CrsNotation::Crs84 => false,
            CrsNotation::Urn | CrsNotation::XUrn | CrsNotation::Http => is_geographic(self.epsg),
        }
    }

    /// Whether the EPSG code is supported for reprojection
    pub fn is_known(epsg: u16) -> bool {
        proj(epsg).is_ok()
    }
}

pub fn is_geographic(epsg: u16) -> bool {
    proj(epsg).map(|p| p.is_latlong()).unwrap_or(false)
}

fn proj(epsg: u16) -> Result<proj4rs::Proj, CrsError> {
    proj4rs::Proj::from_epsg_code(epsg).map_err(|e| CrsError::Unknown(format!("EPSG:{epsg} ({e})")))
}

/// Reproject a geometry (in x/y axis order) between EPSG codes
pub fn transform(geom: &mut Geometry<f64>, from: u16, to: u16) -> Result<(), CrsError> {
    if from == to {
        return Ok(());
    }
    let src = proj(from)?;
    let dst = proj(to)?;
    try_map_coords(geom, &mut |c| {
        let (mut x, mut y) = (c.x, c.y);
        if src.is_latlong() {
            x = x.to_radians();
            y = y.to_radians();
        }
        let (mut x, mut y) = proj4rs::adaptors::transform_xy(&src, &dst, x, y)
            .map_err(|e| CrsError::Transform(e.to_string()))?;
        if dst.is_latlong() {
            x = x.to_degrees();
            y = y.to_degrees();
        }
        Ok(Coord { x, y })
    })
}

type CoordFn<'a> = dyn FnMut(Coord<f64>) -> Result<Coord<f64>, CrsError> + 'a;

/// Apply a fallible function to all coordinates of a geometry
pub fn try_map_coords(geom: &mut Geometry<f64>, f: &mut CoordFn) -> Result<(), CrsError> {
    fn line(ls: &mut geo::LineString<f64>, f: &mut CoordFn) -> Result<(), CrsError> {
        for c in ls.0.iter_mut() {
            *c = f(*c)?;
        }
        Ok(())
    }
    fn poly(p: &mut geo::Polygon<f64>, f: &mut CoordFn) -> Result<(), CrsError> {
        let mut err = None;
        p.exterior_mut(|ext| err = line(ext, f).err());
        if let Some(e) = err {
            return Err(e);
        }
        p.interiors_mut(|ints| {
            for ring in ints {
                if err.is_none() {
                    err = line(ring, f).err();
                }
            }
        });
        err.map_or(Ok(()), Err)
    }
    match geom {
        Geometry::Point(p) => p.0 = f(p.0)?,
        Geometry::Line(l) => {
            l.start = f(l.start)?;
            l.end = f(l.end)?;
        }
        Geometry::LineString(ls) => line(ls, f)?,
        Geometry::Polygon(p) => poly(p, f)?,
        Geometry::MultiPoint(mp) => {
            for p in mp.0.iter_mut() {
                p.0 = f(p.0)?;
            }
        }
        Geometry::MultiLineString(mls) => {
            for ls in mls.0.iter_mut() {
                line(ls, f)?;
            }
        }
        Geometry::MultiPolygon(mp) => {
            for p in mp.0.iter_mut() {
                poly(p, f)?;
            }
        }
        Geometry::GeometryCollection(gc) => {
            for g in gc.0.iter_mut() {
                try_map_coords(g, f)?;
            }
        }
        Geometry::Rect(r) => {
            let mut p = Geometry::Polygon(r.to_polygon());
            try_map_coords(&mut p, f)?;
            *geom = p;
        }
        Geometry::Triangle(t) => {
            let mut p = Geometry::Polygon(t.to_polygon());
            try_map_coords(&mut p, f)?;
            *geom = p;
        }
    }
    Ok(())
}

/// EPSG code with deprecated or vendor aliases resolved (Web Mercator: 900913, 3785, 102100, ...)
fn epsg_alias(code: u32) -> Option<u16> {
    match code {
        900913 | 3785 | 102100 | 102113 | 41001 => Some(3857),
        c => u16::try_from(c).ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{point, Point};

    #[test]
    fn parse_notations() {
        use CrsNotation::*;
        for (name, epsg, notation) in [
            ("EPSG:4326", 4326, Epsg),
            ("epsg:3857", 3857, Epsg),
            (
                "http://www.opengis.net/gml/srs/epsg.xml#4326",
                4326,
                EpsgXml,
            ),
            ("urn:ogc:def:crs:EPSG::4326", 4326, Urn),
            ("urn:ogc:def:crs:EPSG:6.9:4326", 4326, Urn),
            ("urn:x-ogc:def:crs:EPSG:4326", 4326, XUrn),
            ("urn:x-ogc:def:crs:EPSG:6.11.2:4326", 4326, XUrn),
            ("http://www.opengis.net/def/crs/EPSG/0/32615", 32615, Http),
            ("https://www.opengis.net/def/crs/EPSG/0/2056", 2056, Http),
            ("urn:ogc:def:crs:OGC:1.3:CRS84", 4326, Crs84),
            ("urn:ogc:def:crs:OGC::CRS84", 4326, Crs84),
            ("http://www.opengis.net/def/crs/OGC/1.3/CRS84", 4326, Crs84),
            // Web Mercator aliases
            ("EPSG:900913", 3857, Epsg),
            ("EPSG:102100", 3857, Epsg),
            ("urn:ogc:def:crs:EPSG::3785", 3857, Urn),
        ] {
            assert_eq!(Crs::parse(name), Ok(Crs::new(epsg, notation)), "{name}");
        }
        assert!(matches!(Crs::parse("EPSG:abc"), Err(CrsError::Unknown(_))));
        assert!(matches!(
            Crs::parse("EPSG:999999"),
            Err(CrsError::Unknown(_))
        ));
        assert!(matches!(Crs::parse("foo"), Err(CrsError::Unknown(_))));
    }

    #[test]
    fn names() {
        use CrsNotation::*;
        assert_eq!(Crs::new(4326, Epsg).name(), "EPSG:4326");
        assert_eq!(
            Crs::new(4326, EpsgXml).name(),
            "http://www.opengis.net/gml/srs/epsg.xml#4326"
        );
        assert_eq!(Crs::new(4326, Urn).name(), "urn:ogc:def:crs:EPSG::4326");
        assert_eq!(Crs::new(4326, XUrn).name(), "urn:x-ogc:def:crs:EPSG:4326");
        assert_eq!(
            Crs::new(4326, Http).name(),
            "http://www.opengis.net/def/crs/EPSG/0/4326"
        );
        assert_eq!(
            Crs::new(4326, Crs84).name(),
            "http://www.opengis.net/def/crs/OGC/1.3/CRS84"
        );
    }

    #[test]
    fn axis_order() {
        use CrsNotation::*;
        // geographic CRS: lat/lon in URN/HTTP notations
        assert!(!Crs::new(4326, Epsg).swap_xy());
        assert!(!Crs::new(4326, EpsgXml).swap_xy());
        assert!(Crs::new(4326, Urn).swap_xy());
        assert!(Crs::new(4326, XUrn).swap_xy());
        assert!(Crs::new(4326, Http).swap_xy());
        assert!(!Crs::new(4326, Crs84).swap_xy());
        assert!(Crs::new(4258, Urn).swap_xy());
        // projected CRS with easting/northing
        assert!(!Crs::new(3857, Urn).swap_xy());
        assert!(!Crs::new(32615, Http).swap_xy());
        assert!(!Crs::new(3395, Urn).swap_xy());
    }

    #[test]
    fn known_codes() {
        assert!(Crs::is_known(4326));
        assert!(Crs::is_known(3857));
        assert!(Crs::is_known(32615));
        assert!(!Crs::is_known(1));
    }

    fn assert_close(a: Point<f64>, b: Point<f64>, eps: f64) {
        assert!(
            (a.x() - b.x()).abs() < eps && (a.y() - b.y()).abs() < eps,
            "{a:?} != {b:?}"
        );
    }

    #[test]
    fn reproject() {
        let mut g: Geometry<f64> = point!(x: 8.5, y: 47.0).into();
        transform(&mut g, 4326, 3857).unwrap();
        let Geometry::Point(p) = g else { panic!() };
        assert_close(p, point!(x: 946215.67, y: 5942074.07), 0.1);

        let mut g: Geometry<f64> = p.into();
        transform(&mut g, 3857, 4326).unwrap();
        let Geometry::Point(p) = g else { panic!() };
        assert_close(p, point!(x: 8.5, y: 47.0), 1e-7);

        // identity
        let mut g: Geometry<f64> = point!(x: 1.0, y: 2.0).into();
        transform(&mut g, 4326, 4326).unwrap();
        assert_eq!(g, point!(x: 1.0, y: 2.0).into());

        // UTM 15N
        let mut g: Geometry<f64> = point!(x: -93.0, y: 45.0).into();
        transform(&mut g, 4326, 32615).unwrap();
        let Geometry::Point(p) = g else { panic!() };
        assert_close(p, point!(x: 500000.0, y: 4982950.4), 0.1);

        assert!(transform(&mut g, 4326, 1).is_err());
    }
}
