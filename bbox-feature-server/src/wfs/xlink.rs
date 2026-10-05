//! XLink resolution (WFS 2.0 resolve / WFS 1.1 traverseXlinkDepth) and GetGmlObject.
//!
//! Link elements (`<p xlink:href="#id"/>`) are replaced by the referenced object, prefixed with a
//! comment holding the original href. Resolution is depth limited, stops at cycles and never
//! inlines an object twice along a path.

use crate::wfs::crs::{Crs, CrsNotation};
use crate::wfs::endpoint::{Body, OpResult, WfsResponse};
use crate::wfs::exception::{WfsError, WfsResult};
use crate::wfs::filter::{Filter, ResourceId};
use crate::wfs::gml::GmlVersion;
use crate::wfs::model::{QName, XLINK_NS};
use crate::wfs::output::{FeatureEncoder, OutputCrs};
use crate::wfs::request::RawRequest;
use crate::wfs::service::WfsService;
use crate::wfs::store::StoreQuery;
use crate::wfs::version::Version;
use crate::wfs::xml::{escape, serialize_node};
use futures::future::BoxFuture;
use futures::TryStreamExt;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Resolution settings
#[derive(Clone)]
pub struct XlinkCtx {
    pub svc: Arc<WfsService>,
    pub gml: GmlVersion,
    pub local: bool,
    pub remote: bool,
    /// Report unresolvable local links and unsupported schemes as exceptions (WFS 1.1)
    pub strict: bool,
    pub deadline: Option<Instant>,
    /// Namespace declarations for parsing encoded fragments
    pub ns_decls: String,
}

impl XlinkCtx {
    pub fn new(
        svc: &Arc<WfsService>,
        version: Version,
        gml: GmlVersion,
        local: bool,
        remote: bool,
        timeout: Option<Duration>,
    ) -> Self {
        let mut ns_decls = format!(
            r#" xmlns:gml="{}" xmlns:xlink="{XLINK_NS}" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance""#,
            if gml == GmlVersion::V32 {
                "http://www.opengis.net/gml/3.2"
            } else {
                "http://www.opengis.net/gml"
            }
        );
        for ns in &svc.namespaces {
            ns_decls.push_str(&format!(r#" xmlns:{}="{}""#, ns.prefix, escape(&ns.uri)));
        }
        XlinkCtx {
            svc: svc.clone(),
            gml,
            local,
            remote,
            strict: version == Version::V110,
            deadline: timeout.map(|t| Instant::now() + t),
            ns_decls,
        }
    }

    fn expired(&self) -> bool {
        self.deadline.map(|d| Instant::now() > d).unwrap_or(false)
    }

    /// Encoded object (feature or geometry) with the given gml:id
    async fn find_object(&self, id: &str) -> WfsResult<Option<String>> {
        let filter = Filter::ResourceIds(vec![ResourceId {
            rid: id.to_string(),
            version: None,
            start_date: None,
            end_date: None,
        }]);
        for t in &self.svc.types {
            let q = StoreQuery {
                filter: Some(filter.clone()),
                limit: Some(1),
                ..Default::default()
            };
            let found: Vec<_> = t
                .store
                .query(q)
                .try_collect()
                .await
                .map_err(|e| WfsError::no_applicable(e.to_string()))?;
            if let Some(f) = found.first() {
                let encoder = self.encoder(&t.def);
                let mut out = String::new();
                encoder.write_feature(&mut out, f);
                return Ok(Some(out));
            }
        }
        for t in &self.svc.types {
            if let Some(g) = t
                .store
                .find_geometry(id)
                .await
                .map_err(|e| WfsError::no_applicable(e.to_string()))?
            {
                let encoder = self.encoder(&t.def);
                if let Some(geom) = encoder.transform(&g.geometry) {
                    let mut out = String::new();
                    encoder.write_geometry_meta(&mut out, &geom, Some(id), g.meta.as_deref());
                    return Ok(Some(out));
                }
            }
        }
        Ok(None)
    }

    fn encoder<'a>(&self, def: &'a crate::wfs::model::FeatureTypeDef) -> FeatureEncoder<'a> {
        let notation = if self.gml == GmlVersion::V2 {
            CrsNotation::Epsg
        } else {
            CrsNotation::Urn
        };
        FeatureEncoder {
            def,
            gml: self.gml,
            crs: OutputCrs::new(Crs::new(def.srid, notation), None),
            properties: None,
            feature_bounding: false,
        }
    }

    async fn fetch_remote(&self, href: &str) -> Option<String> {
        let (url, fragment) = match href.split_once('#') {
            Some((u, f)) => (u, Some(f)),
            None => (href, None),
        };
        let timeout = self
            .deadline
            .map(|d| d.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_secs(self.svc.cfg.resolve_timeout));
        let client = reqwest::Client::builder().timeout(timeout).build().ok()?;
        let text = client
            .get(url)
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .text()
            .await
            .ok()?;
        let doc = roxmltree::Document::parse(&text).ok()?;
        let node = match fragment {
            Some(f) => doc.descendants().find(|n| {
                n.is_element()
                    && n.attributes().any(|a| {
                        a.name() == "id"
                            && a.namespace()
                                .map(|ns| ns.starts_with("http://www.opengis.net/gml"))
                                .unwrap_or(false)
                            && a.value() == f
                    })
            })?,
            None => doc.root_element(),
        };
        Some(serialize_node(node))
    }
}

/// Resolve links in an encoded fragment. `depth` None: unlimited.
pub fn resolve<'a>(
    xml: String,
    depth: Option<u32>,
    ancestors: Vec<String>,
    ctx: &'a XlinkCtx,
) -> BoxFuture<'a, WfsResult<String>> {
    Box::pin(async move {
        if depth == Some(0) || !(ctx.local || ctx.remote) || !xml.contains("href=") {
            return Ok(xml);
        }
        let wrapped = format!("<r{}>{xml}</r>", ctx.ns_decls);
        let offset = wrapped.len() - xml.len() - 4;
        let doc = match roxmltree::Document::parse(&wrapped) {
            Ok(d) => d,
            Err(_) => return Ok(xml),
        };
        // link elements: xlink:href without element content
        let links: Vec<(std::ops::Range<usize>, String)> = doc
            .descendants()
            .filter(|n| {
                n.is_element()
                    && n.attribute((XLINK_NS, "href")).is_some()
                    && !n.children().any(|c| c.is_element())
            })
            .map(|n| {
                (
                    n.range(),
                    n.attribute((XLINK_NS, "href")).unwrap_or("").to_string(),
                )
            })
            .collect();
        let mut out = xml.clone();
        // replace from the end to keep earlier ranges valid
        for (range, href) in links.into_iter().rev() {
            if ctx.expired() {
                return Err(WfsError::no_applicable(
                    "XLink resolution timed out (traverseXlinkExpiry)",
                ));
            }
            let start = range.start - offset;
            let end = range.end - offset;
            let element = &xml[start..end];
            let referent = if let Some(id) = href.strip_prefix('#') {
                if !ctx.local || ancestors.iter().any(|a| a == id) {
                    None
                } else {
                    match ctx.find_object(id).await? {
                        Some(obj) => {
                            let mut anc = ancestors.clone();
                            anc.push(id.to_string());
                            Some(resolve(obj, depth.map(|d| d - 1), anc, ctx).await?)
                        }
                        None if ctx.strict => {
                            return Err(WfsError::no_applicable(format!(
                                "Unresolvable local link `{href}`"
                            )))
                        }
                        None => None,
                    }
                }
            } else if href.starts_with("http://") || href.starts_with("https://") {
                if !ctx.remote || ancestors.contains(&href) {
                    None
                } else {
                    match ctx.fetch_remote(&href).await {
                        Some(obj) => {
                            let mut anc = ancestors.clone();
                            anc.push(href.clone());
                            Some(resolve(obj, depth.map(|d| d - 1), anc, ctx).await?)
                        }
                        None => None,
                    }
                }
            } else if ctx.strict {
                return Err(WfsError::no_applicable(format!(
                    "Unsupported link domain `{href}`"
                )));
            } else {
                None
            };
            if let Some(obj) = referent {
                out.replace_range(start..end, &replace_link(element, &href, &obj));
            }
        }
        Ok(out)
    })
}

/// Resolve links in an encoded feature, with link traversal depths for individual properties
/// (WFS 1.1 wfs:XlinkPropertyName). Other properties use `depth`.
pub async fn resolve_feature(
    xml: String,
    depth: Option<u32>,
    property_depths: &[(QName, Option<u32>)],
    ancestors: Vec<String>,
    ctx: &XlinkCtx,
) -> WfsResult<String> {
    if property_depths.is_empty() {
        return resolve(xml, depth, ancestors, ctx).await;
    }
    let wrapped = format!("<r{}>{xml}</r>", ctx.ns_decls);
    let offset = wrapped.len() - xml.len() - 4;
    let Ok(doc) = roxmltree::Document::parse(&wrapped) else {
        return Ok(xml);
    };
    let Some(feature) = doc.root_element().first_element_child() else {
        return Ok(xml);
    };
    let props: Vec<(std::ops::Range<usize>, Option<u32>)> = feature
        .children()
        .filter(|c| c.is_element())
        .map(|p| {
            let name = p.tag_name();
            let d = property_depths
                .iter()
                .find(|(q, _)| q.local == name.name() && name.namespace().unwrap_or("") == q.ns)
                .map(|(_, d)| *d)
                .unwrap_or(depth);
            (p.range().start - offset..p.range().end - offset, d)
        })
        .collect();
    let mut out = xml.clone();
    for (range, d) in props.into_iter().rev() {
        let prop = xml[range.clone()].to_string();
        let resolved = resolve(prop, d, ancestors.clone(), ctx).await?;
        out.replace_range(range, &resolved);
    }
    Ok(out)
}

/// Replace a link element by an element containing the referent
fn replace_link(element: &str, href: &str, referent: &str) -> String {
    // element name and start tag without the xlink:href attribute
    let name_end = element[1..]
        .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .map(|i| i + 1)
        .unwrap_or(element.len());
    let name = &element[1..name_end];
    let tag_end = element.find('>').unwrap_or(element.len());
    let mut start_tag = element[..tag_end].trim_end_matches('/').to_string();
    if let Some(pos) = start_tag.find(":href=\"") {
        let attr_start = start_tag[..pos].rfind(char::is_whitespace).unwrap_or(pos);
        let value_end = start_tag[pos + 7..]
            .find('"')
            .map(|i| pos + 7 + i + 1)
            .unwrap_or(start_tag.len());
        start_tag.replace_range(attr_start..value_end, "");
    }
    format!(
        "{start_tag}><!-- xlink:href=\"{}\" -->{referent}</{name}>",
        href.replace("--", "- -")
    )
}

/// GetGmlObject (WFS 1.1)
pub async fn get_gml_object(svc: &Arc<WfsService>, raw: &RawRequest) -> OpResult {
    let version = raw
        .operation_version()
        .map_err(|e| (raw.error_version(), e))?;
    let err = |e: WfsError| (version, e);
    let (id, depth) = if raw.is_kvp() {
        (
            raw.kvp.value("gmlobjectid").map(str::to_string),
            raw.kvp.value("traversexlinkdepth").map(str::to_string),
        )
    } else {
        let doc = roxmltree::Document::parse(raw.xml.as_deref().unwrap_or(""))
            .map_err(|e| err(WfsError::parsing("request", e.to_string())))?;
        let root = doc.root_element();
        let id = root
            .descendants()
            .find(|n| n.is_element() && n.tag_name().name() == "GmlObjectId")
            .and_then(|n| {
                n.attributes()
                    .find(|a| a.name() == "id")
                    .map(|a| a.value().to_string())
            });
        (id, root.attribute("traverseXlinkDepth").map(str::to_string))
    };
    let id = id.ok_or_else(|| err(WfsError::missing("gmlObjectId")))?;
    let depth = match depth.as_deref() {
        None => Some(0),
        Some("*") => None,
        Some(d) => Some(d.parse().map_err(|_| {
            err(WfsError::invalid(
                "traverseXlinkDepth",
                format!("Invalid depth `{d}`"),
            ))
        })?),
    };
    let gml = version.gml();
    let ctx = XlinkCtx::new(svc, version, gml, true, true, None);
    let obj = ctx.find_object(&id).await.map_err(err)?.ok_or_else(|| {
        err(WfsError::invalid(
            "gmlObjectId",
            format!("No object with gml:id `{id}`"),
        ))
    })?;
    let resolved = resolve(obj, depth, vec![id.clone()], &ctx)
        .await
        .map_err(err)?;
    // namespace declarations on the root element
    let first_end = resolved
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(resolved.len());
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>{}{}{}"#,
        &resolved[..first_end],
        ctx.ns_decls,
        &resolved[first_end..]
    );
    Ok(WfsResponse::xml(Body::Text(doc)).with_type("text/xml; subtype=gml/3.1.1"))
}
