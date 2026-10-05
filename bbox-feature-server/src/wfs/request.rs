//! Request intake: KVP (GET / form POST), XML POST and SOAP envelopes.

use crate::wfs::exception::{WfsError, WfsResult};
use crate::wfs::version::Version;
use crate::wfs::xml::serialize_node;
use std::collections::HashMap;

pub const SOAP11_NS: &str = "http://schemas.xmlsoap.org/soap/envelope/";
pub const SOAP12_NS: &str = "http://www.w3.org/2003/05/soap-envelope";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Kvp,
    Xml,
    Soap11,
    Soap12,
}

/// KVP parameters with lower case names
#[derive(Clone, Debug, Default)]
pub struct Kvp {
    params: HashMap<String, String>,
}

impl Kvp {
    /// Parse a (form encoded) query string; TEAM Engine sends values unencoded
    pub fn parse(query: &str) -> Self {
        let mut params = HashMap::new();
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let key = decode(k).to_lowercase();
            params.entry(key).or_insert_with(|| decode(v));
        }
        Kvp { params }
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.params.get(key).map(String::as_str)
    }
    /// Non-empty parameter value
    pub fn value(&self, key: &str) -> Option<&str> {
        self.get(key).filter(|v| !v.trim().is_empty())
    }
    pub fn contains(&self, key: &str) -> bool {
        self.params.contains_key(key)
    }
    /// Comma separated list
    pub fn list(&self, key: &str) -> Option<Vec<String>> {
        self.value(key).map(|v| {
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
    }
    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.params.iter()
    }
}

/// Percent-decode with `+` as space; invalid escapes are kept literally
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Incoming request before operation specific parsing
#[derive(Clone, Debug)]
pub struct RawRequest {
    pub encoding: Encoding,
    pub kvp: Kvp,
    /// XML request body (SOAP body content unwrapped)
    pub xml: Option<String>,
    /// Operation name as sent
    pub operation: Option<String>,
    pub service: Option<String>,
    /// Version parameter / attribute as sent
    pub version: Option<String>,
    /// Namespace of the XML root element
    pub xml_ns: Option<String>,
}

impl RawRequest {
    pub fn from_kvp(query: &str) -> Self {
        let kvp = Kvp::parse(query);
        RawRequest {
            encoding: Encoding::Kvp,
            operation: kvp.value("request").map(str::to_string),
            service: kvp.get("service").map(str::to_string),
            version: kvp.value("version").map(str::to_string),
            kvp,
            xml: None,
            xml_ns: None,
        }
    }

    /// Parse POST body. Query parameters of POST requests are ignored.
    pub fn from_body(body: &str, content_type: &str) -> WfsResult<Self> {
        if content_type.starts_with("application/x-www-form-urlencoded") {
            return Ok(Self::from_kvp(body));
        }
        let doc = roxmltree::Document::parse(body.trim_start_matches('\u{feff}'))
            .map_err(|e| WfsError::parsing("request", format!("Malformed XML request: {e}")))?;
        let mut root = doc.root_element();
        let mut encoding = Encoding::Xml;
        let xml;
        if root.tag_name().name() == "Envelope"
            && matches!(
                root.tag_name().namespace(),
                Some(SOAP11_NS) | Some(SOAP12_NS)
            )
        {
            encoding = if root.tag_name().namespace() == Some(SOAP11_NS) {
                Encoding::Soap11
            } else {
                Encoding::Soap12
            };
            let body_el = root
                .children()
                .find(|c| c.is_element() && c.tag_name().name() == "Body")
                .ok_or_else(|| WfsError::parsing("request", "SOAP envelope without Body"))?;
            root = body_el
                .children()
                .find(|c| c.is_element())
                .ok_or_else(|| WfsError::parsing("request", "empty SOAP Body"))?;
            xml = serialize_node(root);
        } else {
            xml = body.to_string();
        }
        Ok(RawRequest {
            encoding,
            kvp: Kvp::default(),
            operation: Some(root.tag_name().name().to_string()),
            service: root.attribute("service").map(str::to_string),
            version: root.attribute("version").map(str::to_string),
            xml_ns: root.tag_name().namespace().map(str::to_string),
            xml: Some(xml),
        })
    }

    pub fn is_kvp(&self) -> bool {
        self.encoding == Encoding::Kvp
    }

    /// Version used for exception reports before the request is fully parsed
    pub fn error_version(&self) -> Version {
        if let Some(v) = self.version.as_deref().and_then(Version::parse_lenient) {
            return v;
        }
        match self.xml_ns.as_deref() {
            Some("http://www.opengis.net/wfs") => Version::V110,
            _ => Version::V200,
        }
    }

    /// Version for an operation other than GetCapabilities
    pub fn operation_version(&self) -> WfsResult<Version> {
        match self.version.as_deref() {
            Some(v) => Version::parse_lenient(v)
                .ok_or_else(|| WfsError::invalid("version", format!("Unsupported version `{v}`"))),
            None => Ok(match self.xml_ns.as_deref() {
                Some("http://www.opengis.net/wfs") => Version::V110,
                _ => Version::V200,
            }),
        }
    }

    /// Check SERVICE parameter. Without `strict`, a missing SERVICE defaults to WFS.
    pub fn check_service(&self, strict: bool) -> WfsResult<()> {
        match self.service.as_deref() {
            None if self.is_kvp() && strict => Err(WfsError::missing("service")),
            None => Ok(()),
            Some(s) if s.trim().is_empty() => Err(WfsError::missing("service")),
            Some(s) if !s.trim().eq_ignore_ascii_case("WFS") => Err(WfsError::invalid(
                "service",
                format!("Unsupported service `{s}`"),
            )),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kvp_decoding() {
        let kvp = Kvp::parse("SERVICE=WFS&typeName=cdf:Other,cdf:Seven&filter=%3Cogc%3AFilter+xmlns%3Aogc%3D%22x%22%2F%3E&bad=%ZZ&empty=");
        assert_eq!(kvp.get("service"), Some("WFS"));
        assert_eq!(
            kvp.list("typename").unwrap(),
            vec!["cdf:Other", "cdf:Seven"]
        );
        assert_eq!(kvp.get("filter"), Some(r#"<ogc:Filter xmlns:ogc="x"/>"#));
        assert_eq!(kvp.get("bad"), Some("%ZZ"));
        assert_eq!(kvp.value("empty"), None);
        assert!(kvp.contains("empty"));
        assert_eq!(decode("a%2"), "a%2");
        assert_eq!(decode("%C3%A9"), "é");
    }

    #[test]
    fn soap_unwrapping() {
        let body = r#"<soap:Envelope xmlns:soap="http://www.w3.org/2003/05/soap-envelope"><soap:Body><wfs:GetCapabilities xmlns:wfs="http://www.opengis.net/wfs/2.0" service="WFS"/></soap:Body></soap:Envelope>"#;
        let raw = RawRequest::from_body(body, "application/soap+xml").unwrap();
        assert_eq!(raw.encoding, Encoding::Soap12);
        assert_eq!(raw.operation.as_deref(), Some("GetCapabilities"));
        assert!(raw.xml.unwrap().starts_with("<wfs:GetCapabilities"));
    }

    #[test]
    fn xml_root() {
        let raw = RawRequest::from_body(
            r#"<GetFeature xmlns="http://www.opengis.net/wfs" service="WFS" version="1.0.0"/>"#,
            "text/xml",
        )
        .unwrap();
        assert_eq!(raw.operation.as_deref(), Some("GetFeature"));
        assert_eq!(raw.operation_version().unwrap(), Version::V100);
        assert!(RawRequest::from_body("<a", "text/xml").is_err());
        let raw = RawRequest::from_body(
            "service=WFS&request=GetCapabilities",
            "application/x-www-form-urlencoded",
        )
        .unwrap();
        assert!(raw.is_kvp());
    }
}
