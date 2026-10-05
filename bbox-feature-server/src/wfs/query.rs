//! Query request model and parsing (DescribeFeatureType, GetFeature, GetPropertyValue).

use crate::wfs::exception::{WfsError, WfsResult};
use crate::wfs::request::{Kvp, RawRequest};
use crate::wfs::version::Version;
use crate::wfs::xml::serialize_node;
use std::collections::HashMap;

/// Feature type reference as given in a request
#[derive(Clone, Debug, PartialEq)]
pub struct TypeName {
    pub ns: Option<String>,
    pub prefix: Option<String>,
    pub local: String,
}

impl TypeName {
    /// Parse a (prefixed) name with a namespace resolver
    pub fn parse(
        name: &str,
        namespaces: &HashMap<String, String>,
        default_ns: Option<&str>,
    ) -> Self {
        let name = name.trim();
        // Clark notation {ns}local
        if let Some(rest) = name.strip_prefix('{') {
            if let Some((ns, local)) = rest.split_once('}') {
                return TypeName {
                    ns: Some(ns.to_string()),
                    prefix: None,
                    local: local.to_string(),
                };
            }
        }
        match name.split_once(':') {
            Some((prefix, local)) => TypeName {
                ns: namespaces.get(prefix).cloned(),
                prefix: Some(prefix.to_string()),
                local: local.to_string(),
            },
            None => TypeName {
                ns: default_ns.map(str::to_string),
                prefix: None,
                local: name.to_string(),
            },
        }
    }
    pub fn display(&self) -> String {
        match &self.prefix {
            Some(p) => format!("{p}:{}", self.local),
            None => self.local.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ResultType {
    #[default]
    Results,
    Hits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Resolve {
    #[default]
    None,
    Local,
    Remote,
    All,
}

/// Filter given in a request (parsed later, once the feature type is known)
#[derive(Clone, Debug, PartialEq)]
pub enum FilterSource {
    /// ogc:Filter / fes:Filter XML
    Xml(String),
    /// GeoServer CQL_FILTER / ECQL text
    Cql(String),
}

/// Ad hoc query
#[derive(Clone, Debug, Default)]
pub struct Query {
    pub type_names: Vec<TypeName>,
    pub aliases: Vec<String>,
    pub srs_name: Option<String>,
    pub property_names: Vec<String>,
    pub filter: Option<FilterSource>,
    pub sort_by: Vec<(String, bool)>,
    pub feature_version: Option<String>,
    /// Prefix bindings for property names and filters
    pub namespaces: HashMap<String, String>,
    pub default_ns: Option<String>,
    /// Query handle (exception locator for errors of this query)
    pub handle: Option<String>,
    /// WFS 1.1 per property link traversal depth (wfs:XlinkPropertyName / PROPTRAVXLINKDEPTH).
    /// Depth None: unlimited
    pub xlink_depths: Vec<(String, Option<u32>)>,
}

/// Stored query invocation
#[derive(Clone, Debug)]
pub struct StoredQueryCall {
    pub id: String,
    /// (name, value text or XML, namespaces in scope)
    pub params: Vec<(String, String, HashMap<String, String>)>,
}

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum QueryExpr {
    Adhoc(Query),
    Stored(StoredQueryCall),
}

/// GetFeature / GetPropertyValue request
#[derive(Clone, Debug)]
pub struct GetFeatureRequest {
    pub version: Version,
    pub output_format: Option<String>,
    pub result_type: ResultType,
    pub start_index: u64,
    pub count: Option<u64>,
    pub resolve: Resolve,
    /// None: `*` (unlimited)
    pub resolve_depth: Option<u32>,
    pub resolve_timeout: Option<u64>,
    pub queries: Vec<QueryExpr>,
    /// Global feature ids (KVP FEATUREID / RESOURCEID)
    pub feature_ids: Vec<String>,
    /// KVP BBOX (coordinates, optional CRS)
    pub bbox: Option<(Vec<f64>, Option<String>)>,
    /// GetPropertyValue value reference
    pub value_reference: Option<String>,
    /// GeoServer format_options
    pub format_options: HashMap<String, String>,
    /// Request was encoded as KVP
    pub kvp: bool,
    /// Original KVP parameters (for paging links)
    pub kvp_params: Vec<(String, String)>,
}

/// DescribeFeatureType request
#[derive(Clone, Debug)]
pub struct DescribeRequest {
    pub version: Version,
    pub type_names: Vec<TypeName>,
    pub output_format: Option<String>,
}

/// KVP namespace bindings: NAMESPACES (2.0) or NAMESPACE (1.0/1.1)
fn kvp_namespaces(kvp: &Kvp) -> (HashMap<String, String>, Option<String>) {
    kvp.value("namespaces")
        .or_else(|| kvp.value("namespace"))
        .map(parse_namespaces)
        .unwrap_or_default()
}

/// Parse KVP NAMESPACES (2.0: `xmlns(p,uri)`, 1.1: `xmlns(p=uri)`, default: `xmlns(uri)`)
pub fn parse_namespaces(value: &str) -> (HashMap<String, String>, Option<String>) {
    let mut map = HashMap::new();
    let mut default = None;
    let mut rest = value;
    while let Some(start) = rest.find("xmlns(") {
        let after = &rest[start + 6..];
        let Some(end) = after.find(')') else { break };
        let decl = &after[..end];
        // uri may contain '=' or ',' only after the separator
        if let Some((p, uri)) = decl.split_once(',') {
            map.insert(p.trim().to_string(), uri.trim().to_string());
        } else if let Some((p, uri)) = decl.split_once('=').filter(|(p, _)| !p.contains(':')) {
            map.insert(p.trim().to_string(), uri.trim().to_string());
        } else {
            default = Some(decl.trim().to_string());
        }
        rest = &after[end + 1..];
    }
    (map, default)
}

/// Split KVP parenthesised lists `(a,b)(c)` into groups
fn groups(value: &str) -> Option<Vec<String>> {
    let v = value.trim();
    if !v.starts_with('(') {
        return None;
    }
    let mut out = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in v.char_indices() {
        match c {
            '(' => {
                if depth == 0 {
                    start = i + 1;
                }
                depth += 1;
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    out.push(v[start..i].to_string());
                }
            }
            _ => {}
        }
    }
    Some(out)
}

/// Split a FILTER parameter into per-query filters: `(<Filter/>)(<Filter/>)` or a single filter
fn split_filters(value: &str) -> Vec<String> {
    let v = value.trim();
    if v.starts_with('(') {
        if let Some(g) = groups(v) {
            return g;
        }
    }
    vec![v.to_string()]
}

fn parse_u64(kvp_name: &str, v: &str) -> WfsResult<u64> {
    v.trim()
        .parse::<u64>()
        .map_err(|_| WfsError::invalid(kvp_name, format!("Invalid value `{v}`")))
}

fn parse_resolve(v: &str) -> WfsResult<Resolve> {
    match v.trim() {
        "local" => Ok(Resolve::Local),
        "remote" => Ok(Resolve::Remote),
        "all" => Ok(Resolve::All),
        "none" => Ok(Resolve::None),
        other => Err(WfsError::invalid(
            "resolve",
            format!("Invalid resolve value `{other}`"),
        )),
    }
}

fn parse_resolve_depth(v: &str) -> WfsResult<Option<u32>> {
    match v.trim() {
        "*" => Ok(None),
        n => n
            .parse::<u32>()
            .map(Some)
            .map_err(|_| WfsError::invalid("resolveDepth", format!("Invalid resolveDepth `{n}`"))),
    }
}

/// Parse sortBy KVP: `a ASC,b DESC` (2.0), `a A,b D` (1.1)
fn parse_sort_by(v: &str) -> Vec<(String, bool)> {
    v.split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            let mut parts = s.split_whitespace();
            let name = parts.next().unwrap_or("").to_string();
            let desc = matches!(
                parts.next().map(|o| o.to_uppercase()).as_deref(),
                Some("DESC") | Some("D")
            );
            (name, desc)
        })
        .collect()
}

fn output_format_for_dft(version: Version, f: &str) -> WfsResult<()> {
    // GeoServer JSON schema description (all versions)
    let json = matches!(
        f.to_ascii_lowercase().as_str(),
        "application/json" | "json" | "text/javascript"
    );
    let ok = json
        || match version {
            Version::V100 => f.eq_ignore_ascii_case("XMLSCHEMA") || f.starts_with("text/xml"),
            _ => {
                f.eq_ignore_ascii_case("XMLSCHEMA")
                    || f.starts_with("text/xml")
                    || f.starts_with("application/gml+xml")
                    || f.starts_with("application/xml")
            }
        };
    if ok {
        Ok(())
    } else {
        Err(WfsError::invalid(
            "outputFormat",
            format!("Unsupported output format `{f}`"),
        ))
    }
}

pub fn parse_describe(
    raw: &RawRequest,
    server_ns: &HashMap<String, String>,
) -> WfsResult<DescribeRequest> {
    let version = raw.operation_version()?;
    if raw.is_kvp() {
        let kvp = &raw.kvp;
        let (mut ns, default_ns) = kvp_namespaces(kvp);
        for (p, u) in server_ns {
            ns.entry(p.clone()).or_insert_with(|| u.clone());
        }
        let names = kvp.value("typenames").or_else(|| kvp.value("typename"));
        let type_names = names
            .map(|v| {
                v.trim_matches(|c| c == '(' || c == ')')
                    .split([',', ')', '('])
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| TypeName::parse(s, &ns, default_ns.as_deref()))
                    .collect()
            })
            .unwrap_or_default();
        let output_format = kvp.value("outputformat").map(str::to_string);
        if let Some(f) = &output_format {
            output_format_for_dft(version, f)?;
        }
        Ok(DescribeRequest {
            version,
            type_names,
            output_format,
        })
    } else {
        let xml = raw.xml.as_deref().unwrap_or("");
        let doc = roxmltree::Document::parse(xml)
            .map_err(|e| WfsError::parsing("request", e.to_string()))?;
        let root = doc.root_element();
        let mut type_names = Vec::new();
        for n in root
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "TypeName")
        {
            let text = n.text().unwrap_or("").trim();
            type_names.push(type_name_in_scope(&n, text, server_ns));
        }
        let output_format = root.attribute("outputFormat").map(str::to_string);
        if let Some(f) = &output_format {
            output_format_for_dft(version, f)?;
        }
        Ok(DescribeRequest {
            version,
            type_names,
            output_format,
        })
    }
}

/// Resolve a prefixed name in the scope of an XML node (server prefixes as fallback)
fn type_name_in_scope(
    node: &roxmltree::Node,
    name: &str,
    server_ns: &HashMap<String, String>,
) -> TypeName {
    match name.split_once(':') {
        Some((prefix, local)) => TypeName {
            ns: node
                .lookup_namespace_uri(Some(prefix))
                .map(str::to_string)
                .or_else(|| server_ns.get(prefix).cloned()),
            prefix: Some(prefix.to_string()),
            local: local.to_string(),
        },
        None => TypeName {
            ns: None,
            prefix: None,
            local: name.to_string(),
        },
    }
}

fn scope_namespaces(
    node: &roxmltree::Node,
    server_ns: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut map = server_ns.clone();
    for ns in node.namespaces() {
        if let Some(p) = ns.name() {
            map.insert(p.to_string(), ns.uri().to_string());
        }
    }
    map
}

pub fn parse_get_feature(
    raw: &RawRequest,
    server_ns: &HashMap<String, String>,
    property_value: bool,
) -> WfsResult<GetFeatureRequest> {
    let version = raw.operation_version()?;
    if raw.is_kvp() {
        parse_get_feature_kvp(&raw.kvp, version, server_ns, property_value)
    } else {
        parse_get_feature_xml(
            raw.xml.as_deref().unwrap_or(""),
            version,
            server_ns,
            property_value,
        )
    }
}

fn parse_get_feature_kvp(
    kvp: &Kvp,
    version: Version,
    server_ns: &HashMap<String, String>,
    property_value: bool,
) -> WfsResult<GetFeatureRequest> {
    let (mut ns, default_ns) = kvp_namespaces(kvp);
    for (p, u) in server_ns {
        ns.entry(p.clone()).or_insert_with(|| u.clone());
    }
    let count = match kvp.value("count").or_else(|| kvp.value("maxfeatures")) {
        Some(v) => Some(parse_u64("count", v)?),
        None => None,
    };
    let start_index = match kvp.value("startindex") {
        Some(v) => parse_u64("startIndex", v)?,
        None => 0,
    };
    let result_type = match kvp.value("resulttype").map(|v| v.to_lowercase()) {
        None => ResultType::Results,
        Some(v) if v == "results" => ResultType::Results,
        Some(v) if v == "hits" => ResultType::Hits,
        Some(v) => {
            return Err(WfsError::invalid(
                "resultType",
                format!("Invalid resultType `{v}`"),
            ))
        }
    };
    let traverse = kvp.value("traversexlinkdepth");
    let resolve = match kvp.value("resolve") {
        Some(v) => parse_resolve(v)?,
        None if traverse.is_some() => Resolve::All,
        None => Resolve::None,
    };
    let resolve_depth = match kvp.value("resolvedepth").or(traverse) {
        Some(v) => parse_resolve_depth(v)?,
        None if version.is_v2() => None,
        None => Some(0),
    };
    let resolve_timeout = kvp
        .value("resolvetimeout")
        .or_else(|| kvp.value("traversexlinkexpiry"))
        .map(|v| parse_u64("resolveTimeout", v))
        .transpose()?;
    let feature_ids: Vec<String> = kvp
        .list("resourceid")
        .or_else(|| kvp.list("featureid"))
        .unwrap_or_default();
    let bbox = match kvp.value("bbox") {
        Some(v) => {
            let parts: Vec<&str> = v.split(',').map(str::trim).collect();
            let nums: Vec<f64> = parts
                .iter()
                .take_while(|p| p.parse::<f64>().is_ok())
                .map(|p| p.parse().unwrap())
                .collect();
            if nums.len() != 4 && nums.len() != 6 {
                return Err(WfsError::invalid("bbox", format!("Invalid BBOX `{v}`")));
            }
            let crs = parts.get(nums.len()).map(|s| s.to_string());
            Some((nums, crs))
        }
        None => None,
    };
    let mut format_options = HashMap::new();
    if let Some(fo) = kvp.value("format_options") {
        for opt in fo.split(';') {
            if let Some((k, v)) = opt.split_once(':') {
                format_options.insert(k.trim().to_lowercase(), v.trim().to_string());
            }
        }
    }
    let mut queries = Vec::new();
    // STOREDQUERY_ID (GeoServer also accepts storedQueryId)
    if let Some(sq) = kvp
        .value("storedquery_id")
        .or_else(|| kvp.value("storedqueryid"))
    {
        let reserved = [
            "service",
            "version",
            "request",
            "storedquery_id",
            "storedqueryid",
            "count",
            "maxfeatures",
            "startindex",
            "resulttype",
            "outputformat",
            "resolve",
            "resolvedepth",
            "resolvetimeout",
            "namespaces",
            "namespace",
            "valuereference",
            "resolvepath",
            "format_options",
        ];
        let params = kvp
            .iter()
            .filter(|(k, _)| !reserved.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone(), ns.clone()))
            .collect();
        queries.push(QueryExpr::Stored(StoredQueryCall {
            id: sq.to_string(),
            params,
        }));
    } else if let Some(tn) = kvp.value("typenames").or_else(|| kvp.value("typename")) {
        let aliases: Vec<String> = kvp.list("aliases").unwrap_or_default();
        let type_groups: Vec<Vec<String>> = match groups(tn) {
            Some(g) => g
                .into_iter()
                .map(|grp| {
                    grp.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .collect(),
            None => {
                let names: Vec<String> = tn
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if version.is_v2() && names.len() > 1 && aliases.len() == names.len() {
                    vec![names]
                } else {
                    names.into_iter().map(|n| vec![n]).collect()
                }
            }
        };
        let n = type_groups.len();
        let per_query = |key: &str| -> Vec<Option<String>> {
            match kvp.value(key) {
                None => vec![None; n],
                Some(v) => match groups(v) {
                    Some(g) if g.len() == n => g.into_iter().map(Some).collect(),
                    _ => vec![Some(v.to_string()); n],
                },
            }
        };
        let srs = per_query("srsname");
        let props = per_query("propertyname");
        let prop_depths = per_query("proptravxlinkdepth");
        let sorts = per_query("sortby");
        let filters: Vec<Option<FilterSource>> =
            match (kvp.value("filter"), kvp.value("cql_filter")) {
                (Some(f), _) => {
                    let fs = split_filters(f);
                    // an empty group `()` means no filter for that query
                    let xml = |f: String| (!f.trim().is_empty()).then_some(FilterSource::Xml(f));
                    if fs.len() == n {
                        fs.into_iter().map(xml).collect()
                    } else if fs.len() == 1 {
                        vec![xml(fs[0].clone()); n]
                    } else {
                        return Err(WfsError::invalid(
                            "filter",
                            "Number of filters does not match number of queries",
                        ));
                    }
                }
                (None, Some(c)) => {
                    let cs: Vec<&str> = c.split(';').collect();
                    if cs.len() == n {
                        cs.into_iter()
                            .map(|c| Some(FilterSource::Cql(c.to_string())))
                            .collect()
                    } else {
                        vec![Some(FilterSource::Cql(c.to_string())); n]
                    }
                }
                _ => vec![None; n],
            };
        for (i, grp) in type_groups.into_iter().enumerate() {
            let type_names: Vec<TypeName> = grp
                .iter()
                .map(|t| TypeName::parse(t, &ns, default_ns.as_deref()))
                .collect();
            let property_names: Vec<String> = props[i]
                .as_deref()
                .map(|p| {
                    p.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            // PROPTRAVXLINKDEPTH: depths aligned with PROPERTYNAME
            let mut xlink_depths = Vec::new();
            if let Some(d) = prop_depths[i].as_deref() {
                for (name, depth) in property_names.iter().zip(d.split(',')) {
                    xlink_depths.push((name.clone(), parse_resolve_depth(depth)?));
                }
            }
            queries.push(QueryExpr::Adhoc(Query {
                aliases: if type_names.len() > 1 {
                    aliases.clone()
                } else {
                    Vec::new()
                },
                type_names,
                srs_name: srs[i].clone(),
                property_names,
                filter: filters[i].clone(),
                sort_by: sorts[i].as_deref().map(parse_sort_by).unwrap_or_default(),
                feature_version: kvp.value("featureversion").map(str::to_string),
                namespaces: ns.clone(),
                default_ns: default_ns.clone(),
                xlink_depths,
                handle: None,
            }));
        }
    } else if feature_ids.is_empty() {
        return Err(WfsError::missing(if version.is_v2() {
            "typeNames"
        } else {
            "typeName"
        }));
    }
    let value_reference = kvp.get("valuereference").map(str::to_string);
    if property_value && value_reference.is_none() {
        return Err(WfsError::missing("valueReference"));
    }
    // per property link traversal (WFS 1.1) enables resolution with global depth 0
    let resolve = match resolve {
        Resolve::None
            if queries
                .iter()
                .any(|q| matches!(q, QueryExpr::Adhoc(a) if !a.xlink_depths.is_empty())) =>
        {
            Resolve::All
        }
        r => r,
    };
    Ok(GetFeatureRequest {
        version,
        output_format: kvp.value("outputformat").map(str::to_string),
        result_type,
        start_index,
        count,
        resolve,
        resolve_depth,
        resolve_timeout,
        queries,
        feature_ids,
        bbox,
        value_reference,
        format_options,
        kvp: true,
        kvp_params: kvp.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
    })
}

/// Parse a GetFeature XML document (e.g. an expanded stored query template)
pub fn parse_get_feature_body(
    xml: &str,
    version: Version,
    server_ns: &HashMap<String, String>,
) -> WfsResult<GetFeatureRequest> {
    parse_get_feature_xml(xml, version, server_ns, false)
}

fn parse_get_feature_xml(
    xml: &str,
    version: Version,
    server_ns: &HashMap<String, String>,
    property_value: bool,
) -> WfsResult<GetFeatureRequest> {
    let doc =
        roxmltree::Document::parse(xml).map_err(|e| WfsError::parsing("request", e.to_string()))?;
    let root = doc.root_element();
    let attr_u64 = |name: &str| -> WfsResult<Option<u64>> {
        root.attribute(name).map(|v| parse_u64(name, v)).transpose()
    };
    let count = attr_u64("count")?.or(attr_u64("maxFeatures")?);
    let start_index = attr_u64("startIndex")?.unwrap_or(0);
    let result_type = match root.attribute("resultType") {
        None | Some("results") => ResultType::Results,
        Some("hits") => ResultType::Hits,
        Some(v) => {
            return Err(WfsError::invalid(
                "resultType",
                format!("Invalid resultType `{v}`"),
            ))
        }
    };
    let traverse = root.attribute("traverseXlinkDepth");
    let resolve = match root.attribute("resolve") {
        Some(v) => parse_resolve(v)?,
        None if traverse.is_some() => Resolve::All,
        None => Resolve::None,
    };
    let resolve_depth = match root.attribute("resolveDepth").or(traverse) {
        Some(v) => parse_resolve_depth(v)?,
        None if version.is_v2() => None,
        None => Some(0),
    };
    let resolve_timeout = root
        .attribute("resolveTimeout")
        .or(root.attribute("traverseXlinkExpiry"))
        .map(|v| parse_u64("resolveTimeout", v))
        .transpose()?;
    let mut queries = Vec::new();
    for q in root.children().filter(|c| c.is_element()) {
        match q.tag_name().name() {
            "Query" => {
                let ns = scope_namespaces(&q, server_ns);
                let names = q
                    .attribute("typeNames")
                    .or(q.attribute("typeName"))
                    .unwrap_or("");
                let type_names: Vec<TypeName> = names
                    .split(|c: char| c.is_whitespace() || c == ',')
                    .filter(|s| !s.is_empty())
                    .map(|s| type_name_in_scope(&q, s, server_ns))
                    .collect();
                if type_names.is_empty() {
                    return Err(WfsError::missing(if version.is_v2() {
                        "typeNames"
                    } else {
                        "typeName"
                    }));
                }
                let aliases = q
                    .attribute("aliases")
                    .map(|a| a.split_whitespace().map(str::to_string).collect())
                    .unwrap_or_default();
                let mut property_names = Vec::new();
                let mut xlink_depths = Vec::new();
                let mut filter = None;
                let mut sort_by = Vec::new();
                for c in q.children().filter(|c| c.is_element()) {
                    match c.tag_name().name() {
                        "PropertyName" => {
                            property_names.push(c.text().unwrap_or("").trim().to_string())
                        }
                        // not a wfs:Query child; ignored like GeoServer
                        "ValueReference" => {}
                        "XlinkPropertyName" => {
                            let name = c.text().unwrap_or("").trim().to_string();
                            let depth = match c.attribute("traverseXlinkDepth") {
                                Some(d) => parse_resolve_depth(d)?,
                                None => Some(0),
                            };
                            xlink_depths.push((name.clone(), depth));
                            property_names.push(name);
                        }
                        "Filter" => filter = Some(FilterSource::Xml(serialize_node(c))),
                        "SortBy" => {
                            for sp in c
                                .children()
                                .filter(|s| s.is_element() && s.tag_name().name() == "SortProperty")
                            {
                                let mut name = String::new();
                                let mut desc = false;
                                for e in sp.children().filter(|e| e.is_element()) {
                                    match e.tag_name().name() {
                                        "PropertyName" | "ValueReference" => {
                                            name = e.text().unwrap_or("").trim().to_string()
                                        }
                                        "SortOrder" => {
                                            desc = matches!(
                                                e.text().map(str::trim),
                                                Some("DESC") | Some("D")
                                            )
                                        }
                                        _ => {}
                                    }
                                }
                                sort_by.push((name, desc));
                            }
                        }
                        _ => {}
                    }
                }
                queries.push(QueryExpr::Adhoc(Query {
                    type_names,
                    aliases,
                    srs_name: q.attribute("srsName").map(str::to_string),
                    property_names,
                    filter,
                    sort_by,
                    feature_version: q.attribute("featureVersion").map(str::to_string),
                    namespaces: ns,
                    default_ns: None,
                    xlink_depths,
                    handle: q.attribute("handle").map(str::to_string),
                }));
            }
            "StoredQuery" => {
                let id = q
                    .attribute("id")
                    .ok_or_else(|| WfsError::missing("StoredQuery_id"))?
                    .to_string();
                let params = q
                    .children()
                    .filter(|c| c.is_element() && c.tag_name().name() == "Parameter")
                    .map(|p| {
                        let name = p.attribute("name").unwrap_or("").to_string();
                        let value = match p.children().find(|c| c.is_element()) {
                            Some(e) => serialize_node(e),
                            None => p.text().unwrap_or("").trim().to_string(),
                        };
                        (name, value, scope_namespaces(&p, server_ns))
                    })
                    .collect();
                queries.push(QueryExpr::Stored(StoredQueryCall { id, params }));
            }
            _ => {}
        }
    }
    if queries.is_empty() && !property_value {
        return Err(WfsError::missing("Query"));
    }
    let value_reference = root.attribute("valueReference").map(str::to_string);
    if property_value && value_reference.is_none() {
        return Err(WfsError::missing("valueReference"));
    }
    // per property link traversal (WFS 1.1) enables resolution with global depth 0
    let resolve = match resolve {
        Resolve::None
            if queries
                .iter()
                .any(|q| matches!(q, QueryExpr::Adhoc(a) if !a.xlink_depths.is_empty())) =>
        {
            Resolve::All
        }
        r => r,
    };
    Ok(GetFeatureRequest {
        version,
        output_format: root.attribute("outputFormat").map(str::to_string),
        result_type,
        start_index,
        count,
        resolve,
        resolve_depth,
        resolve_timeout,
        queries,
        feature_ids: Vec::new(),
        bbox: None,
        value_reference,
        format_options: HashMap::new(),
        kvp: false,
        kvp_params: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_param() {
        let (m, d) = parse_namespaces("xmlns(xml,http://www.w3.org/XML/1998/namespace),xmlns(wfs,http://www.opengis.net/wfs/2.0),xmlns(ns42,http://x.org)");
        assert_eq!(m.get("ns42").map(String::as_str), Some("http://x.org"));
        assert_eq!(d, None);
        let (m, _) = parse_namespaces("xmlns(sf=http://cite.opengeospatial.org/gmlsf)");
        assert_eq!(
            m.get("sf").map(String::as_str),
            Some("http://cite.opengeospatial.org/gmlsf")
        );
        let (_, d) = parse_namespaces(
            "xmlns(http://www.opengis.net/cite/data),xmlns(wfs,http://www.opengis.net/wfs/2.0)",
        );
        assert_eq!(d.as_deref(), Some("http://www.opengis.net/cite/data"));
    }

    #[test]
    fn kvp_query_groups() {
        let raw = RawRequest::from_kvp("service=WFS&version=2.0.0&request=GetFeature&typenames=(a:A)(a:B,a:C)&filter=(<f1/>)(<f2/>)");
        let req = parse_get_feature(&raw, &HashMap::new(), false).unwrap();
        assert_eq!(req.queries.len(), 2);
        let QueryExpr::Adhoc(q2) = &req.queries[1] else {
            panic!()
        };
        assert_eq!(q2.type_names.len(), 2);
        assert_eq!(q2.filter, Some(FilterSource::Xml("<f2/>".into())));
        // plain list: separate queries, with aliases: join
        let raw =
            RawRequest::from_kvp("service=WFS&version=2.0.0&request=GetFeature&typenames=A,B");
        assert_eq!(
            parse_get_feature(&raw, &HashMap::new(), false)
                .unwrap()
                .queries
                .len(),
            2
        );
        let raw = RawRequest::from_kvp(
            "service=WFS&version=2.0.0&request=GetFeature&typenames=A,B&aliases=a,b",
        );
        assert_eq!(
            parse_get_feature(&raw, &HashMap::new(), false)
                .unwrap()
                .queries
                .len(),
            1
        );
        let raw = RawRequest::from_kvp(
            "service=WFS&version=2.0.0&request=GetFeature&typenames=A&sortBy=x DESC,y",
        );
        let req = parse_get_feature(&raw, &HashMap::new(), false).unwrap();
        let QueryExpr::Adhoc(q) = &req.queries[0] else {
            panic!()
        };
        assert_eq!(
            q.sort_by,
            vec![("x".to_string(), true), ("y".to_string(), false)]
        );
        // stored query parameters
        let raw = RawRequest::from_kvp(
            "service=WFS&version=2.0.0&request=GetFeature&storedquery_id=urn:x&id=f1",
        );
        let req = parse_get_feature(&raw, &HashMap::new(), false).unwrap();
        let QueryExpr::Stored(sq) = &req.queries[0] else {
            panic!()
        };
        assert_eq!(sq.params[0].0, "id");
    }

    #[test]
    fn errors() {
        let raw = RawRequest::from_kvp("service=WFS&version=2.0.0&request=GetFeature");
        assert_eq!(
            parse_get_feature(&raw, &HashMap::new(), false)
                .unwrap_err()
                .code,
            "MissingParameterValue"
        );
        let raw = RawRequest::from_kvp(
            "service=WFS&version=2.0.0&request=GetFeature&typenames=a&count=x",
        );
        assert_eq!(
            parse_get_feature(&raw, &HashMap::new(), false)
                .unwrap_err()
                .code,
            "InvalidParameterValue"
        );
    }
}
