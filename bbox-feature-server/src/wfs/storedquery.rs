//! Stored queries (WFS 2.0): built-in GetFeatureById and managed stored queries.

use crate::wfs::endpoint::{Body, OpResult, WfsResponse};
use crate::wfs::exception::{WfsError, WfsResult};
use crate::wfs::filter::{Filter, ResourceId};
use crate::wfs::gml::GmlVersion;
use crate::wfs::output::{FeatureEncoder, OutputCrs};
use crate::wfs::query::{GetFeatureRequest, QueryExpr, StoredQueryCall};
use crate::wfs::request::RawRequest;
use crate::wfs::service::WfsService;
use crate::wfs::store::StoreQuery;
use crate::wfs::version::Version;
use crate::wfs::xml::{escape, serialize_node, XmlWriter};
use futures::TryStreamExt;
use std::sync::Arc;

pub const GET_FEATURE_BY_ID_URN: &str = "urn:ogc:def:query:OGC-WFS::GetFeatureById";
pub const GET_FEATURE_BY_ID_HTTP: &str =
    "http://www.opengis.net/def/query/OGC-WFS/0/GetFeatureById";
pub const WFS_QUERY_LANGUAGE: &str = "urn:ogc:def:queryLanguage:OGC-WFS::WFSQueryExpression";

#[derive(Clone, Debug, PartialEq)]
pub struct StoredQueryParameter {
    pub name: String,
    /// Type QName, e.g. `xsd:string`
    pub type_: String,
    pub title: Option<String>,
    pub abstract_: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredQueryDef {
    pub id: String,
    pub title: String,
    pub abstract_: Option<String>,
    pub parameters: Vec<StoredQueryParameter>,
    /// Return feature types (prefixed names); empty: any
    pub return_types: Vec<String>,
    pub language: String,
    /// Query expression template (XML of the QueryExpressionText content); None for built-ins
    pub template: Option<String>,
    /// Namespace declarations in scope of the template
    pub namespaces: Vec<(String, String)>,
    /// Feature types queried by the template (prefixed names, without parametrized ones)
    pub query_types: Vec<String>,
    pub is_private: bool,
    pub builtin: bool,
}

impl StoredQueryDef {
    pub fn is_get_feature_by_id(&self) -> bool {
        self.id == GET_FEATURE_BY_ID_URN || self.id == GET_FEATURE_BY_ID_HTTP
    }
}

/// Built-in stored queries
pub fn builtin() -> Vec<StoredQueryDef> {
    [GET_FEATURE_BY_ID_URN, GET_FEATURE_BY_ID_HTTP]
        .into_iter()
        .map(|id| StoredQueryDef {
            id: id.to_string(),
            title: "Get feature by identifier".to_string(),
            abstract_: Some("Returns the single feature whose value of gml:id matches the value of the id parameter".to_string()),
            parameters: vec![StoredQueryParameter {
                // ISO 19142 7.9.3.6 (parameter values are matched case-insensitively)
                name: "ID".to_string(),
                type_: "xsd:string".to_string(),
                title: Some("Identifier".to_string()),
                abstract_: Some("The gml:id of the feature".to_string()),
            }],
            return_types: vec![],
            language: WFS_QUERY_LANGUAGE.to_string(),
            template: None,
            namespaces: vec![],
            query_types: vec![],
            is_private: true,
            builtin: true,
        })
        .collect()
}

fn ns_decls(svc: &WfsService) -> Vec<(String, String)> {
    svc.used_namespaces()
        .iter()
        .map(|n| (format!("xmlns:{}", n.prefix), n.uri.clone()))
        .collect()
}

fn all_type_names(svc: &WfsService) -> Vec<String> {
    svc.types.iter().map(|t| t.def.name.prefixed()).collect()
}

/// Return types of a stored query (declared, else queried by the template, else all)
fn return_types(svc: &WfsService, def: &StoredQueryDef) -> Vec<String> {
    if !def.return_types.is_empty() {
        def.return_types.clone()
    } else if !def.query_types.is_empty() {
        def.query_types.clone()
    } else {
        all_type_names(svc)
    }
}

fn root_attrs<'a>(
    base: &[(&'a str, &'a str)],
    nss: &'a [(String, String)],
) -> Vec<(&'a str, &'a str)> {
    let mut v = base.to_vec();
    for (k, u) in nss {
        v.push((k.as_str(), u.as_str()));
    }
    v
}

const WFS2_NS: &str = "http://www.opengis.net/wfs/2.0";
const SCHEMA_LOC: &str =
    "http://www.opengis.net/wfs/2.0 http://schemas.opengis.net/wfs/2.0/wfs.xsd";

pub fn list_stored_queries(svc: &WfsService, raw: &RawRequest) -> OpResult {
    let version = raw
        .operation_version()
        .map_err(|e| (raw.error_version(), e))?;
    let queries = svc.stored_queries.read().unwrap().clone();
    let nss = ns_decls(svc);
    let mut w = XmlWriter::new();
    w.decl().open(
        "wfs:ListStoredQueriesResponse",
        &root_attrs(
            &[
                ("xmlns:wfs", WFS2_NS),
                ("xmlns:xsi", "http://www.w3.org/2001/XMLSchema-instance"),
                ("xsi:schemaLocation", SCHEMA_LOC),
            ],
            &nss,
        ),
    );
    for q in &queries {
        w.open("wfs:StoredQuery", &[("id", &q.id)])
            .elem("wfs:Title", &q.title);
        for t in return_types(svc, q) {
            w.elem("wfs:ReturnFeatureType", &t);
        }
        w.close("wfs:StoredQuery");
    }
    w.close("wfs:ListStoredQueriesResponse");
    let _ = version;
    Ok(WfsResponse::xml(Body::Text(w.out)))
}

pub fn describe_stored_queries(svc: &WfsService, raw: &RawRequest) -> OpResult {
    let version = raw
        .operation_version()
        .map_err(|e| (raw.error_version(), e))?;
    let ids: Vec<String> = if raw.is_kvp() {
        raw.kvp
            .list("storedquery_id")
            .or_else(|| raw.kvp.list("storedqueryid"))
            .unwrap_or_default()
    } else {
        let doc = roxmltree::Document::parse(raw.xml.as_deref().unwrap_or(""))
            .map_err(|e| (version, WfsError::parsing("request", e.to_string())))?;
        doc.root_element()
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "StoredQueryId")
            .filter_map(|c| c.text().map(|t| t.trim().to_string()))
            .collect()
    };
    let all = svc.stored_queries.read().unwrap().clone();
    let selected: Vec<&StoredQueryDef> = if ids.is_empty() {
        all.iter().collect()
    } else {
        let mut sel = Vec::new();
        for id in &ids {
            match all.iter().find(|q| q.id == *id) {
                Some(q) => sel.push(q),
                None => {
                    return Err((
                        version,
                        WfsError::invalid("StoredQuery_id", format!("Unknown stored query `{id}`")),
                    ))
                }
            }
        }
        sel
    };
    let nss = ns_decls(svc);
    let mut w = XmlWriter::new();
    w.decl().open(
        "wfs:DescribeStoredQueriesResponse",
        &root_attrs(
            &[
                ("xmlns:wfs", WFS2_NS),
                ("xmlns:fes", "http://www.opengis.net/fes/2.0"),
                ("xmlns:xs", "http://www.w3.org/2001/XMLSchema"),
                ("xmlns:xsd", "http://www.w3.org/2001/XMLSchema"),
                ("xmlns:xsi", "http://www.w3.org/2001/XMLSchema-instance"),
                ("xsi:schemaLocation", SCHEMA_LOC),
            ],
            &nss,
        ),
    );
    for q in selected {
        w.open("wfs:StoredQueryDescription", &[("id", &q.id)])
            .elem("wfs:Title", &q.title);
        if let Some(a) = &q.abstract_ {
            w.elem("wfs:Abstract", a);
        }
        for p in &q.parameters {
            let title = p.title.clone().unwrap_or_else(|| p.name.clone());
            w.open("wfs:Parameter", &[("name", &p.name), ("type", &p.type_)])
                .elem("wfs:Title", &title);
            if let Some(a) = &p.abstract_ {
                w.elem("wfs:Abstract", a);
            }
            w.close("wfs:Parameter");
        }
        // user defined queries are described as created (returnFeatureTypes may be empty)
        let types = if q.builtin {
            return_types(svc, q).join(" ")
        } else {
            q.return_types.join(" ")
        };
        let private = if q.is_private { "true" } else { "false" };
        let mut attrs: Vec<(&str, &str)> = vec![
            ("returnFeatureTypes", &types),
            ("language", &q.language),
            ("isPrivate", private),
        ];
        let decls: Vec<(String, String)> = q
            .namespaces
            .iter()
            .filter(|(p, _)| !p.is_empty() && p != "wfs" && p != "xml")
            .map(|(p, u)| (format!("xmlns:{p}"), u.clone()))
            .collect();
        for (k, v) in &decls {
            attrs.push((k.as_str(), v.as_str()));
        }
        match (&q.template, q.is_private) {
            (Some(t), false) => {
                w.open("wfs:QueryExpressionText", &attrs)
                    .raw(t)
                    .close("wfs:QueryExpressionText");
            }
            _ => {
                w.empty("wfs:QueryExpressionText", &attrs);
            }
        }
        w.close("wfs:StoredQueryDescription");
    }
    w.close("wfs:DescribeStoredQueriesResponse");
    Ok(WfsResponse::xml(Body::Text(w.out)))
}

pub fn create_stored_query(svc: &WfsService, raw: &RawRequest) -> OpResult {
    let version = raw
        .operation_version()
        .map_err(|e| (raw.error_version(), e))?;
    let err = |e: WfsError| (version, e);
    let xml = raw
        .xml
        .as_deref()
        .ok_or_else(|| err(WfsError::not_supported("CreateStoredQuery (KVP)")))?;
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| err(WfsError::parsing("request", e.to_string())))?;
    let mut defs = Vec::new();
    for d in doc
        .root_element()
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == "StoredQueryDefinition")
    {
        defs.push(parse_definition(d).map_err(err)?);
    }
    if defs.is_empty() {
        return Err(err(WfsError::missing("StoredQueryDefinition")));
    }
    let mut store = svc.stored_queries.write().unwrap();
    for def in &defs {
        if store.iter().any(|q| q.id == def.id) {
            return Err(err(WfsError::new(
                "DuplicateStoredQueryIdValue",
                Some(&def.id),
                format!("Stored query `{}` already exists", def.id),
            )));
        }
    }
    store.extend(defs);
    drop(store);
    Ok(WfsResponse::xml(Body::Text(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><wfs:CreateStoredQueryResponse status="OK" xmlns:wfs="{WFS2_NS}" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:schemaLocation="{SCHEMA_LOC}"/>"#
    ))))
}

fn parse_definition(node: roxmltree::Node) -> WfsResult<StoredQueryDef> {
    let id = node
        .attribute("id")
        .ok_or_else(|| WfsError::missing("id"))?
        .to_string();
    let child_text = |n: roxmltree::Node, name: &str| -> Option<String> {
        n.children()
            .find(|c| c.is_element() && c.tag_name().name() == name)
            .and_then(|c| c.text())
            .map(|t| t.trim().to_string())
    };
    let mut parameters = Vec::new();
    for p in node
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == "Parameter")
    {
        parameters.push(StoredQueryParameter {
            name: p
                .attribute("name")
                .ok_or_else(|| WfsError::missing("Parameter/@name"))?
                .to_string(),
            type_: p.attribute("type").unwrap_or("xsd:string").to_string(),
            title: child_text(p, "Title"),
            abstract_: child_text(p, "Abstract"),
        });
    }
    let qet = node
        .children()
        .find(|c| c.is_element() && c.tag_name().name() == "QueryExpressionText")
        .ok_or_else(|| WfsError::missing("QueryExpressionText"))?;
    let language = qet.attribute("language").unwrap_or("").to_string();
    if language != WFS_QUERY_LANGUAGE
        && language != "urn:ogc:def:queryLanguage:OGC-WFS::WFS_QueryExpression"
    {
        return Err(WfsError::invalid(
            "language",
            format!("Unsupported query language `{language}`"),
        ));
    }
    let template: String = qet
        .children()
        .filter(|c| c.is_element())
        .map(serialize_node)
        .collect();
    let mut namespaces: Vec<(String, String)> = Vec::new();
    for ns in qet.namespaces() {
        if let Some(p) = ns.name() {
            namespaces.push((p.to_string(), ns.uri().to_string()));
        }
    }
    let return_types: Vec<String> = qet
        .attribute("returnFeatureTypes")
        .map(|t| t.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    // queried types must be return types (compared by namespace URI and local name)
    let expand = |n: roxmltree::Node, qname: &str| -> (Option<String>, String) {
        match qname.split_once(':') {
            Some((p, l)) => (
                n.lookup_namespace_uri(Some(p)).map(str::to_string),
                l.to_string(),
            ),
            None => (
                n.lookup_namespace_uri(None).map(str::to_string),
                qname.to_string(),
            ),
        }
    };
    let returned: Vec<(Option<String>, String)> =
        return_types.iter().map(|t| expand(qet, t)).collect();
    let mut query_types = Vec::new();
    for q in qet
        .descendants()
        .filter(|c| c.is_element() && c.tag_name().name() == "Query")
    {
        let names = q
            .attribute("typeNames")
            .or(q.attribute("typeName"))
            .unwrap_or("");
        for t in names.split_whitespace().filter(|t| !t.contains("${")) {
            if !returned.is_empty() && !returned.contains(&expand(q, t)) {
                return Err(WfsError::invalid(
                    "returnFeatureTypes",
                    format!("Query type `{t}` is not one of the returnFeatureTypes"),
                ));
            }
            if !query_types.iter().any(|x: &String| x == t) {
                query_types.push(t.to_string());
            }
        }
    }
    Ok(StoredQueryDef {
        title: child_text(node, "Title").unwrap_or_else(|| id.clone()),
        abstract_: child_text(node, "Abstract"),
        id,
        parameters,
        return_types,
        language: WFS_QUERY_LANGUAGE.to_string(),
        template: Some(template),
        namespaces,
        query_types,
        is_private: qet.attribute("isPrivate") == Some("true"),
        builtin: false,
    })
}

pub fn drop_stored_query(svc: &WfsService, raw: &RawRequest) -> OpResult {
    let version = raw
        .operation_version()
        .map_err(|e| (raw.error_version(), e))?;
    let err = |e: WfsError| (version, e);
    let id = if raw.is_kvp() {
        raw.kvp
            .value("storedquery_id")
            .or_else(|| raw.kvp.value("storedqueryid"))
            .or_else(|| raw.kvp.value("id"))
            .map(str::to_string)
    } else {
        let doc = roxmltree::Document::parse(raw.xml.as_deref().unwrap_or(""))
            .map_err(|e| err(WfsError::parsing("request", e.to_string())))?;
        doc.root_element().attribute("id").map(str::to_string)
    }
    .ok_or_else(|| err(WfsError::missing("id")))?;
    let mut store = svc.stored_queries.write().unwrap();
    match store.iter().position(|q| q.id == id && !q.builtin) {
        Some(pos) => {
            store.remove(pos);
        }
        None => {
            return Err(err(WfsError::invalid(
                "id",
                format!("Stored query `{id}` does not exist or can not be dropped"),
            )))
        }
    }
    Ok(WfsResponse::xml(Body::Text(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><wfs:DropStoredQueryResponse status="OK" xmlns:wfs="{WFS2_NS}" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:schemaLocation="{SCHEMA_LOC}"/>"#
    ))))
}

/// Execute a stored query
pub async fn execute(
    svc: &Arc<WfsService>,
    req: &GetFeatureRequest,
    call: &StoredQueryCall,
) -> OpResult {
    let version = req.version;
    let err = |e: WfsError| (version, e);
    let def = svc
        .stored_queries
        .read()
        .unwrap()
        .iter()
        .find(|q| q.id == call.id)
        .cloned()
        .ok_or_else(|| {
            err(WfsError::invalid(
                "STOREDQUERY_ID",
                format!("Unknown stored query `{}`", call.id),
            ))
        })?;
    let param =
        |name: &str| -> Option<&(String, String, std::collections::HashMap<String, String>)> {
            call.params
                .iter()
                .find(|(n, _, _)| n.eq_ignore_ascii_case(name))
        };
    if def.is_get_feature_by_id() {
        let id = param("id")
            .map(|(_, v, _)| v.clone())
            .ok_or_else(|| err(WfsError::missing("id")))?;
        return get_feature_by_id(svc, version, &id).await;
    }
    // expand template
    let mut body = def.template.clone().unwrap_or_default();
    for p in &def.parameters {
        let value = param(&p.name)
            .map(|(_, v, _)| v.clone())
            .ok_or_else(|| err(WfsError::missing(&p.name)))?;
        // XML values (e.g. fes:Literal, gml:Envelope) are inserted as XML, text is escaped
        let v = value.trim();
        let is_xml = v.starts_with('<') && v.ends_with('>');
        let text = if is_xml {
            value.clone()
        } else {
            escape(&value)
        };
        body = body.replace(&format!("${{{}}}", p.name), &text);
    }
    // namespace declarations: template scope, parameter scope, then defaults
    let mut nss: Vec<(String, String)> = Vec::new();
    let mut add = |prefix: &str, uri: &str| {
        if prefix != "wfs" && prefix != "xml" && !nss.iter().any(|(p, _)| p == prefix) {
            nss.push((prefix.to_string(), uri.to_string()));
        }
    };
    for (prefix, uri) in &def.namespaces {
        add(prefix, uri);
    }
    for (_, _, params_ns) in &call.params {
        for (prefix, uri) in params_ns {
            add(prefix, uri);
        }
    }
    add("fes", "http://www.opengis.net/fes/2.0");
    let decls: String = nss
        .iter()
        .map(|(p, u)| format!(r#" xmlns:{p}="{}""#, escape(u)))
        .collect();
    let xml = format!(
        r#"<wfs:GetFeature service="WFS" version="{}" xmlns:wfs="{WFS2_NS}"{decls}>{body}</wfs:GetFeature>"#,
        version.label()
    );
    let mut inner = crate::wfs::query::parse_get_feature_body(&xml, version, &svc.prefix_map())
        .map_err(|e| {
            err(WfsError::invalid(
                "STOREDQUERY_ID",
                format!("Invalid stored query expansion: {}", e.text),
            ))
        })?;
    if inner
        .queries
        .iter()
        .any(|q| matches!(q, QueryExpr::Stored(_)))
    {
        return Err(err(WfsError::invalid(
            "STOREDQUERY_ID",
            "Nested stored queries are not supported",
        )));
    }
    // type names with a prefix that is not bound (e.g. a `${typeName}` argument) are matched
    // by local name, like GeoServer
    let known = svc.prefix_map();
    for q in inner.queries.iter_mut() {
        if let QueryExpr::Adhoc(query) = q {
            for t in query.type_names.iter_mut() {
                if t.ns.is_none() && t.prefix.as_deref().is_some_and(|p| !known.contains_key(p)) {
                    t.prefix = None;
                }
            }
        }
    }
    inner.count = req.count;
    inner.start_index = req.start_index;
    inner.result_type = req.result_type;
    inner.output_format = req.output_format.clone();
    inner.resolve = req.resolve;
    inner.resolve_depth = req.resolve_depth;
    inner.resolve_timeout = req.resolve_timeout;
    inner.kvp = req.kvp;
    inner.kvp_params = req.kvp_params.clone();
    crate::wfs::getfeature::run_get_feature(svc, inner).await
}

/// GetFeatureById: the bare feature as document
pub async fn get_feature_by_id(svc: &Arc<WfsService>, version: Version, id: &str) -> OpResult {
    let err = |e: WfsError| (version, e);
    let filter = Filter::ResourceIds(vec![ResourceId {
        rid: id.to_string(),
        version: None,
        start_date: None,
        end_date: None,
    }]);
    // types whose ids can match (prefix `{type}.`), all types otherwise
    let prefixed: Vec<_> = svc
        .types
        .iter()
        .filter(|t| id.starts_with(&format!("{}.", t.def.name.local)))
        .collect();
    let candidates: Vec<_> = if prefixed.is_empty() {
        svc.types.iter().collect()
    } else {
        prefixed
    };
    for t in candidates {
        let q = StoreQuery {
            filter: Some(filter.clone()),
            limit: Some(1),
            ..Default::default()
        };
        let features: Vec<_> = t
            .store
            .query(q)
            .try_collect()
            .await
            .map_err(|e| err(WfsError::no_applicable(e.to_string())))?;
        if let Some(f) = features.first() {
            let gml = version.gml();
            let crs = crate::wfs::crs::Crs::new(t.def.srid, crate::wfs::crs::CrsNotation::Urn);
            let encoder = FeatureEncoder {
                def: &t.def,
                gml,
                crs: OutputCrs::new(crs, None),
                properties: None,
                feature_bounding: false,
            };
            let mut feature = String::new();
            encoder.write_feature(&mut feature, f);
            // declare namespaces on the feature element
            let gml_ns = if gml == GmlVersion::V32 {
                "http://www.opengis.net/gml/3.2"
            } else {
                "http://www.opengis.net/gml"
            };
            let name = t.def.name.prefixed();
            let decls = format!(
                r#" xmlns:{}="{}" xmlns:gml="{gml_ns}" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance""#,
                t.def.name.prefix,
                escape(&t.def.name.ns)
            );
            let feature = feature.replacen(&format!("<{name}"), &format!("<{name}{decls}"), 1);
            let doc = format!(r#"<?xml version="1.0" encoding="UTF-8"?>{feature}"#);
            return Ok(
                WfsResponse::xml(Body::Text(doc)).with_type("application/gml+xml; version=3.2")
            );
        }
    }
    Err(err(WfsError::not_found(
        "id",
        format!("Feature `{id}` not found"),
    )))
}
