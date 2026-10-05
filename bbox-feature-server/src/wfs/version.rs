//! WFS versions and version negotiation.

use crate::wfs::filter::FilterVersion;
use crate::wfs::gml::GmlVersion;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Version {
    V100,
    V110,
    V200,
    V202,
}

impl Version {
    /// Supported versions, lowest first
    pub const ALL: [Version; 4] = [Version::V100, Version::V110, Version::V200, Version::V202];

    pub fn label(&self) -> &'static str {
        match self {
            Version::V100 => "1.0.0",
            Version::V110 => "1.1.0",
            Version::V200 => "2.0.0",
            Version::V202 => "2.0.2",
        }
    }

    pub fn parse(s: &str) -> Option<Version> {
        Version::ALL.into_iter().find(|v| v.label() == s.trim())
    }

    /// Parse a version, accepting the short forms `1.0`, `1.1` and `2.0`
    pub fn parse_lenient(s: &str) -> Option<Version> {
        Version::parse(s).or(match s.trim() {
            "1.0" => Some(Version::V100),
            "1.1" => Some(Version::V110),
            "2.0" => Some(Version::V200),
            _ => None,
        })
    }

    pub fn is_v2(&self) -> bool {
        matches!(self, Version::V200 | Version::V202)
    }

    pub fn gml(&self) -> GmlVersion {
        match self {
            Version::V100 => GmlVersion::V2,
            Version::V110 => GmlVersion::V31,
            _ => GmlVersion::V32,
        }
    }

    pub fn filter(&self) -> FilterVersion {
        match self {
            Version::V100 => FilterVersion::V100,
            Version::V110 => FilterVersion::V110,
            _ => FilterVersion::V200,
        }
    }

    /// WFS namespace
    pub fn wfs_ns(&self) -> &'static str {
        if self.is_v2() {
            "http://www.opengis.net/wfs/2.0"
        } else {
            "http://www.opengis.net/wfs"
        }
    }

    pub fn gml_ns(&self) -> &'static str {
        if self.is_v2() {
            "http://www.opengis.net/gml/3.2"
        } else {
            "http://www.opengis.net/gml"
        }
    }

    pub fn highest() -> Version {
        Version::V202
    }
}

fn version_tuple(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.trim().split('.').map(|p| p.parse::<u32>());
    let a = parts.next()?.ok()?;
    let b = parts.next().unwrap_or(Ok(0)).ok()?;
    let c = parts.next().unwrap_or(Ok(0)).ok()?;
    Some((a, b, c))
}

/// Negotiate from a VERSION parameter (WFS 1.0 / OWS rules): exact match, else the next lower
/// supported version, else the lowest supported version.
pub fn negotiate_version_param(requested: &str) -> Version {
    if let Some(v) = Version::parse(requested) {
        return v;
    }
    let Some(req) = version_tuple(requested) else {
        return Version::highest();
    };
    Version::ALL
        .iter()
        .rev()
        .find(|v| version_tuple(v.label()).map(|t| t <= req).unwrap_or(false))
        .copied()
        .unwrap_or(Version::V100)
}

/// Negotiate from AcceptVersions: first supported version in client preference order
pub fn negotiate_accept_versions(list: &[String]) -> Option<Version> {
    list.iter().find_map(|v| Version::parse(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiation() {
        assert_eq!(negotiate_version_param("1.1.0"), Version::V110);
        assert_eq!(negotiate_version_param("99.99.99"), Version::V202);
        assert_eq!(negotiate_version_param("0.99.99"), Version::V100);
        assert_eq!(negotiate_version_param("1.5.0"), Version::V110);
        assert_eq!(negotiate_version_param("2.0.1"), Version::V200);
        assert_eq!(negotiate_version_param("2.0"), Version::V200);
        assert_eq!(
            negotiate_accept_versions(&["10.0.0".into(), "2.0.0".into(), "1.1.0".into()]),
            Some(Version::V200)
        );
        assert_eq!(negotiate_accept_versions(&["0.0.1".into()]), None);
    }
}
