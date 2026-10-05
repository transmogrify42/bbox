//! `/wfs` HTTP endpoint: dispatch of KVP, XML and SOAP requests.

use crate::wfs::exception::WfsError;
use crate::wfs::request::{Encoding, RawRequest};
use crate::wfs::service::WfsService;
use crate::wfs::version::Version;
use actix_web::body::BodyStream;
use actix_web::http::StatusCode;
use actix_web::web::Bytes;
use actix_web::{web, HttpRequest, HttpResponse};
use futures::stream::BoxStream;
use std::sync::Arc;

/// Response body
pub enum Body {
    Text(String),
    Shared(Arc<String>),
    Bytes(Vec<u8>),
    Stream(BoxStream<'static, Result<Bytes, actix_web::Error>>),
}

/// Operation result
pub struct WfsResponse {
    pub status: StatusCode,
    pub content_type: String,
    pub body: Body,
    pub headers: Vec<(String, String)>,
}

impl WfsResponse {
    pub fn xml(body: Body) -> Self {
        WfsResponse {
            status: StatusCode::OK,
            content_type: "text/xml; charset=UTF-8".to_string(),
            body,
            headers: Vec::new(),
        }
    }
    pub fn with_type(mut self, content_type: &str) -> Self {
        self.content_type = content_type.to_string();
        self
    }
}

pub type OpResult = Result<WfsResponse, (Version, WfsError)>;

/// Shared document as response body without copying
struct SharedStr(Arc<String>);

impl AsRef<[u8]> for SharedStr {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

/// Request properties relevant for caching
struct CacheContext {
    /// Canonical request key
    key: String,
    if_none_match: Option<String>,
}

fn into_http(resp: WfsResponse) -> HttpResponse {
    let mut b = HttpResponse::build(resp.status);
    b.content_type(resp.content_type);
    for (k, v) in resp.headers {
        b.insert_header((k, v));
    }
    match resp.body {
        Body::Text(s) => b.body(s),
        Body::Shared(s) => b.body(Bytes::from_owner(SharedStr(s))),
        Body::Bytes(v) => b.body(v),
        Body::Stream(s) => b.body(BodyStream::new(s)),
    }
}

fn error_response(version: Version, err: &WfsError) -> WfsResponse {
    WfsResponse {
        status: err.status(version),
        content_type: err.content_type(version).to_string(),
        body: Body::Text(err.render(version)),
        headers: Vec::new(),
    }
}

/// Wrap response in SOAP envelope
fn soap_response(
    encoding: Encoding,
    resp: WfsResponse,
    fault: Option<(Version, &WfsError)>,
) -> WfsResponse {
    let (ns, content_type) = match encoding {
        Encoding::Soap11 => (
            "http://schemas.xmlsoap.org/soap/envelope/",
            "text/xml; charset=UTF-8",
        ),
        _ => (
            "http://www.w3.org/2003/05/soap-envelope",
            "application/soap+xml; charset=UTF-8",
        ),
    };
    let strip_decl = |s: &str| -> String {
        let s = s.trim_start();
        if s.starts_with("<?xml") {
            s.find("?>")
                .map(|i| s[i + 2..].to_string())
                .unwrap_or_default()
        } else {
            s.to_string()
        }
    };
    let content = match resp.body {
        Body::Text(s) => strip_decl(&s),
        Body::Shared(s) => strip_decl(&s),
        Body::Bytes(b) => strip_decl(&String::from_utf8_lossy(&b)),
        Body::Stream(_) => {
            // streams are collected by callers before SOAP wrapping
            String::new()
        }
    };
    let body = match fault {
        None => content,
        Some((_, err)) => {
            if encoding == Encoding::Soap11 {
                format!(
                    r#"<soap:Fault><faultcode>soap:Server</faultcode><faultstring>{}</faultstring><detail>{content}</detail></soap:Fault>"#,
                    crate::wfs::xml::escape(&err.text)
                )
            } else {
                format!(
                    r#"<soap:Fault><soap:Code><soap:Value>soap:Server</soap:Value></soap:Code><soap:Reason><soap:Text xml:lang="en">{}</soap:Text></soap:Reason><soap:Detail>{content}</soap:Detail></soap:Fault>"#,
                    crate::wfs::xml::escape(&err.text)
                )
            }
        }
    };
    WfsResponse {
        status: resp.status,
        content_type: content_type.to_string(),
        body: Body::Text(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><soap:Envelope xmlns:soap="{ns}"><soap:Body>{body}</soap:Body></soap:Envelope>"#
        )),
        headers: resp.headers,
    }
}

/// Dispatch a request to the operation handlers
pub async fn dispatch(svc: &Arc<WfsService>, raw: RawRequest) -> OpResult {
    let Some(op) = raw.operation.clone() else {
        return Err((raw.error_version(), WfsError::missing("request")));
    };
    match op.to_ascii_lowercase().as_str() {
        "getcapabilities" => {
            let (version, doc) = crate::wfs::capabilities::get_capabilities(svc, &raw)?;
            let _ = version;
            // AcceptFormats: first supported of text/xml, application/xml
            let accepted = raw
                .kvp
                .list("acceptformats")
                .unwrap_or_default()
                .into_iter()
                .find(|f| matches!(f.trim(), "text/xml" | "application/xml"));
            let resp = WfsResponse::xml(Body::Shared(doc));
            Ok(match accepted.as_deref().map(str::trim) {
                Some("application/xml") => resp.with_type("application/xml; charset=UTF-8"),
                _ => resp,
            })
        }
        "transaction" | "lockfeature" | "getfeaturewithlock" => {
            let v = raw.error_version();
            raw.check_service(svc.cfg.cite_compliant)
                .map_err(|e| (v, e))?;
            Err((v, WfsError::not_supported(&op)))
        }
        _ => {
            let v = raw.error_version();
            raw.check_service(svc.cfg.cite_compliant)
                .map_err(|e| (v, e))?;
            crate::wfs::operations::dispatch(svc, &op, &raw).await
        }
    }
}

/// Base64 encoding (RFC 4648)
pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

async fn respond(
    svc: web::Data<Arc<WfsService>>,
    raw: RawRequest,
    cx: CacheContext,
) -> HttpResponse {
    let encoding = raw.encoding;
    let op = raw.operation.as_deref().unwrap_or("").to_ascii_lowercase();
    // ETag: documents describing the service (always), cached responses
    let tagged = matches!(
        op.as_str(),
        "getcapabilities" | "describefeaturetype" | "liststoredqueries" | "describestoredqueries"
    );
    let cache = svc
        .response_cache
        .as_ref()
        .filter(|_| response_cacheable(&svc, &op, &raw));
    if let Some(cache) = cache {
        if let Some(hit) = cache.get(&cx.key) {
            return cached_http(&svc, hit, cx.if_none_match.as_deref(), Some("hit"));
        }
    }
    let mutation = matches!(op.as_str(), "createstoredquery" | "dropstoredquery");
    // GeoServer vendor parameter EXCEPTIONS=application/json | text/javascript
    let json_exceptions = match raw
        .kvp
        .value("exceptions")
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("application/json") => Some(None),
        Some("text/javascript") => Some(Some(
            raw.kvp
                .value("format_options")
                .and_then(|o| {
                    o.split(';')
                        .filter_map(|kv| kv.split_once(':'))
                        .find(|(k, _)| k.trim().eq_ignore_ascii_case("callback"))
                        .map(|(_, v)| v.trim().to_string())
                })
                .unwrap_or_else(|| "parseResponse".to_string()),
        )),
        _ => None,
    };
    let is_dft = raw
        .operation
        .as_deref()
        .map(|o| o.eq_ignore_ascii_case("DescribeFeatureType"))
        .unwrap_or(false);
    let result = dispatch(svc.get_ref(), raw).await;
    let soap = matches!(encoding, Encoding::Soap11 | Encoding::Soap12);
    let resp = match result {
        Ok(resp) if soap && is_dft => {
            // WFS 2.0 D.4: schema returned base64 encoded
            let resp = collect(resp).await;
            let schema = match &resp.body {
                Body::Text(s) => s.clone().into_bytes(),
                Body::Shared(s) => s.as_bytes().to_vec(),
                Body::Bytes(b) => b.clone(),
                Body::Stream(_) => Vec::new(),
            };
            let wrapped = WfsResponse {
                body: Body::Text(format!(
                    r#"<wfs:DescribeFeatureTypeResponse xmlns:wfs="http://www.opengis.net/wfs/2.0">{}</wfs:DescribeFeatureTypeResponse>"#,
                    base64(&schema)
                )),
                ..resp
            };
            // the body carries the base64 encoded schema
            let mut resp = soap_response(encoding, wrapped, None);
            if let Body::Text(t) = &mut resp.body {
                *t = t.replacen("<soap:Body>", r#"<soap:Body type="xsd:base64">"#, 1);
            }
            resp
        }
        Ok(resp) if soap => soap_response(encoding, collect(resp).await, None),
        Ok(resp) => resp,
        Err((version, err)) if json_exceptions.is_some() => {
            let json = serde_json::json!({
                "version": version.label(),
                "exceptions": [{"code": err.code, "locator": err.locator.clone().unwrap_or_default(), "text": err.text}],
            })
            .to_string();
            let (body, content_type) = match json_exceptions.clone().flatten() {
                Some(callback) => (
                    format!("{callback}({json})"),
                    "text/javascript; charset=UTF-8",
                ),
                None => (json, "application/json; charset=UTF-8"),
            };
            WfsResponse {
                status: err.status(version),
                content_type: content_type.to_string(),
                body: Body::Text(body),
                headers: Vec::new(),
            }
        }
        Err((version, err)) => {
            let resp = error_response(version, &err);
            if soap {
                soap_response(encoding, resp, Some((version, &err)))
            } else {
                resp
            }
        }
    };
    if mutation && resp.status == StatusCode::OK {
        svc.clear_caches();
    }
    if resp.status != StatusCode::OK || !(tagged || cache.is_some()) {
        return into_http(resp);
    }
    // buffer small responses for ETag and caching, stream larger ones
    let max = cache.map(|c| c.max_entry).unwrap_or(usize::MAX);
    let resp = match buffer_up_to(resp, max).await {
        Ok(resp) => resp,
        Err(mut streamed) => {
            if cache.is_some() {
                streamed
                    .headers
                    .push(("X-Bbox-Cache".to_string(), "miss".to_string()));
            }
            return into_http(streamed);
        }
    };
    let body = match resp.body {
        Body::Text(s) => Bytes::from(s),
        Body::Shared(s) => Bytes::from_owner(SharedStr(s)),
        Body::Bytes(b) => Bytes::from(b),
        Body::Stream(_) => unreachable!("buffered"),
    };
    let cached = crate::wfs::cache::CachedResponse {
        etag: crate::wfs::cache::etag(&body),
        content_type: resp.content_type,
        body,
    };
    if let Some(cache) = cache {
        cache.insert(cx.key, cached.clone());
    }
    cached_http(
        &svc,
        cached,
        cx.if_none_match.as_deref(),
        cache.map(|_| "miss"),
    )
}

/// Whether the response of a request may be cached
fn response_cacheable(svc: &WfsService, op: &str, raw: &RawRequest) -> bool {
    if !matches!(
        op,
        "getfeature"
            | "getpropertyvalue"
            | "describefeaturetype"
            | "getgmlobject"
            | "describestoredqueries"
            | "liststoredqueries"
    ) {
        return false;
    }
    let collections = &svc.cfg.cache.collections;
    if collections.is_empty() {
        return true;
    }
    let types = requested_types(raw);
    !types.is_empty() && types.iter().all(|t| collections.iter().any(|c| c == t))
}

/// Local names of requested feature types
fn requested_types(raw: &RawRequest) -> Vec<String> {
    let local = |t: &str| t.rsplit(':').next().unwrap_or(t).to_string();
    let mut types = Vec::new();
    if let Some(v) = raw
        .kvp
        .value("typenames")
        .or_else(|| raw.kvp.value("typename"))
    {
        types.extend(
            v.split(|c: char| c == ',' || c == '(' || c == ')' || c.is_whitespace())
                .filter(|t| !t.is_empty())
                .map(local),
        );
    }
    if let Some(xml) = &raw.xml {
        if let Ok(doc) = roxmltree::Document::parse(xml) {
            for n in doc.descendants().filter(|n| n.is_element()) {
                if let Some(v) = n.attribute("typeNames").or(n.attribute("typeName")) {
                    types.extend(v.split_whitespace().map(local));
                }
                if n.tag_name().name() == "TypeName" {
                    types.extend(n.text().map(|t| local(t.trim())));
                }
            }
        }
    }
    types
}

/// Response with ETag (304 if matching If-None-Match), Cache-Control and cache status
fn cached_http(
    svc: &WfsService,
    cached: crate::wfs::cache::CachedResponse,
    if_none_match: Option<&str>,
    cache_status: Option<&str>,
) -> HttpResponse {
    let not_modified = if_none_match
        .map(|inm| crate::wfs::cache::etag_matches(inm, &cached.etag))
        .unwrap_or(false);
    let mut b = HttpResponse::build(if not_modified {
        StatusCode::NOT_MODIFIED
    } else {
        StatusCode::OK
    });
    b.insert_header(("ETag", cached.etag.as_str()));
    if let Some(max_age) = svc.cfg.cache.max_age {
        b.insert_header(("Cache-Control", format!("max-age={max_age}")));
    }
    if let Some(status) = cache_status {
        b.insert_header(("X-Bbox-Cache", status));
    }
    if not_modified {
        return b.finish();
    }
    b.content_type(cached.content_type);
    b.body(cached.body)
}

/// Buffer a response body up to `max` bytes. Larger (or failing) streams are returned as
/// stream of the buffered prefix and the remainder.
async fn buffer_up_to(resp: WfsResponse, max: usize) -> Result<WfsResponse, WfsResponse> {
    use futures::StreamExt;
    let Body::Stream(mut s) = resp.body else {
        return Ok(resp);
    };
    let mut buf = Vec::new();
    while let Some(chunk) = s.next().await {
        match chunk {
            Ok(b) if buf.len() + b.len() <= max => buf.extend_from_slice(&b),
            other => {
                // keep the stream Send: errors are recreated from their message
                let next: Result<Bytes, String> = other.map_err(|e| e.to_string());
                let prefix = futures::stream::iter(vec![Ok(Bytes::from(buf)), next])
                    .map(|r| r.map_err(actix_web::error::ErrorInternalServerError));
                return Err(WfsResponse {
                    body: Body::Stream(prefix.chain(s).boxed()),
                    ..resp
                });
            }
        }
    }
    Ok(WfsResponse {
        body: Body::Bytes(buf),
        ..resp
    })
}

/// Collect a streamed response into memory
async fn collect(resp: WfsResponse) -> WfsResponse {
    use futures::StreamExt;
    match resp.body {
        Body::Stream(mut s) => {
            let mut buf = Vec::new();
            while let Some(chunk) = s.next().await {
                match chunk {
                    Ok(b) => buf.extend_from_slice(&b),
                    Err(_) => break,
                }
            }
            WfsResponse {
                body: Body::Bytes(buf),
                ..resp
            }
        }
        _ => resp,
    }
}

/// WFS version of an unparsable request body, from the root start tag (version attribute,
/// WFS namespace). Defaults to 2.0.
fn body_version(text: &str) -> Version {
    // root start tag: first `<` followed by a name (skipping declarations, DOCTYPE, comments)
    let mut rest = text;
    let tag = loop {
        let Some(i) = rest.find('<') else { break "" };
        rest = &rest[i + 1..];
        if rest.starts_with(|c: char| c.is_alphabetic() || c == '_') {
            break rest.split('>').next().unwrap_or("");
        }
        if let Some(stripped) = rest.strip_prefix("!DOCTYPE") {
            // skip internal subset
            rest = match stripped.find(']') {
                Some(j) if stripped[..j].contains('[') => &stripped[j..],
                _ => stripped,
            };
        }
    };
    let attr = |name: &str| {
        tag.split_whitespace()
            .find_map(|a| a.strip_prefix(&format!("{name}=")))
            .map(|v| v.trim_matches(|c| c == '"' || c == '\'' || c == '/'))
    };
    if let Some(v) = attr("version").and_then(Version::parse_lenient) {
        return v;
    }
    if tag.contains("\"http://www.opengis.net/wfs\"")
        || tag.contains("'http://www.opengis.net/wfs'")
    {
        Version::V110
    } else {
        Version::V200
    }
}

fn if_none_match(req: &HttpRequest) -> Option<String> {
    req.headers()
        .get("if-none-match")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

async fn wfs_get(svc: web::Data<Arc<WfsService>>, req: HttpRequest) -> HttpResponse {
    let raw = RawRequest::from_kvp(req.query_string());
    // canonical key: parameter names case-insensitive, any order
    let mut params: Vec<(String, &String)> = raw
        .kvp
        .iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v))
        .collect();
    params.sort();
    let mut key = String::from("GET ");
    for (k, v) in params {
        key.push_str(&k);
        key.push('=');
        key.push_str(v);
        key.push('&');
    }
    let cx = CacheContext {
        key,
        if_none_match: if_none_match(&req),
    };
    respond(svc, raw, cx).await
}

async fn wfs_post(
    svc: web::Data<Arc<WfsService>>,
    req: HttpRequest,
    body: web::Bytes,
) -> HttpResponse {
    let content_type = req
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    let text = String::from_utf8_lossy(&body);
    match RawRequest::from_body(&text, &content_type) {
        Ok(raw) => {
            let cx = CacheContext {
                key: crate::wfs::cache::body_key(&content_type, &body),
                if_none_match: if_none_match(&req),
            };
            respond(svc, raw, cx).await
        }
        Err(err) => into_http(error_response(body_version(&text), &err)),
    }
}

pub fn register(cfg: &mut web::ServiceConfig, svc: Arc<WfsService>) {
    cfg.app_data(web::Data::new(svc)).service(
        web::resource(["/wfs", "/wfs/"])
            // Camel-Case header names on the wire (HTTP/1.x), as many OGC clients and the
            // TEAM Engine test harness match header names case sensitively.
            .wrap_fn(|req, srv| {
                let fut = actix_web::dev::Service::call(srv, req);
                async move {
                    let mut res = fut.await?;
                    res.response_mut().head_mut().set_camel_case_headers(true);
                    Ok(res)
                }
            })
            .route(web::get().to(wfs_get))
            .route(web::post().to(wfs_post)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_of_unparsable_body() {
        let doctype = r#"<?xml version="1.0"?><!DOCTYPE wfs:GetFeature [<!ATTLIST wfs:GetFeature xmlns:wfs CDATA "http://www.opengis.net/wfs"><!ENTITY c SYSTEM "file:///x">]>"#;
        assert_eq!(
            body_version(&format!(
                r#"{doctype}<wfs:GetFeature service="WFS" version="1.0.0" xmlns:wfs="http://www.opengis.net/wfs">&c;"#
            )),
            Version::V100
        );
        assert_eq!(
            body_version(&format!(
                r#"{doctype}<wfs:GetFeature xmlns:wfs="http://www.opengis.net/wfs/2.0">"#
            )),
            Version::V200
        );
        assert_eq!(
            body_version(r#"<wfs:GetFeature xmlns:wfs="http://www.opengis.net/wfs"><broken"#),
            Version::V110
        );
        assert_eq!(body_version("garbage"), Version::V200);
    }
}
