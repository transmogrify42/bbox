//! GetFeature: query planning and streaming feature collection output.

use crate::wfs::crs::{self, Crs, CrsNotation};
use crate::wfs::describe::describe_url;
use crate::wfs::endpoint::{Body, OpResult, WfsResponse};
use crate::wfs::exception::{WfsError, WfsResult};
use crate::wfs::filter::{
    self, parse_filter_str, Filter, GeometryLiteral, ParseContext, PropertyPath, ResourceId,
    SpatialOp, SpatialOperand,
};
use crate::wfs::gml::GmlVersion;
use crate::wfs::model::*;
use crate::wfs::output::{FeatureEncoder, OutputCrs};
use crate::wfs::query::{
    parse_get_feature, FilterSource, GetFeatureRequest, Query, QueryExpr, ResultType, TypeName,
};
use crate::wfs::request::RawRequest;
use crate::wfs::service::{FeatureTypeEntry, WfsService};
use crate::wfs::store::{FeatureStore, SortKey, SortTarget, StoreQuery};
use crate::wfs::version::Version;
use actix_web::web::Bytes;
use futures::stream::{self, StreamExt};
use log::error;
use std::sync::Arc;

const CHUNK_SIZE: usize = 64 * 1024;

/// Query on one feature type
#[derive(Clone)]
pub struct PlannedQuery {
    pub def: Arc<FeatureTypeDef>,
    pub store: Arc<dyn FeatureStore>,
    pub filter: Option<Filter>,
    pub sort: Vec<SortKey>,
    /// Requested properties (None: all)
    pub properties: Option<Vec<usize>>,
    pub crs: OutputCrs,
    /// Per property link traversal depth (property name, depth; None: unlimited)
    pub xlink_depths: Vec<(QName, Option<u32>)>,
    /// Index of the query (wfs:Query) in the request
    pub query_index: usize,
}

pub use crate::wfs::formats::Format;

pub fn output_format(
    version: Version,
    requested: Option<&str>,
    format_options: &std::collections::HashMap<String, String>,
) -> WfsResult<Format> {
    let default = match version {
        Version::V100 => GmlVersion::V2,
        Version::V110 => GmlVersion::V31,
        _ => GmlVersion::V32,
    };
    let Some(f) = requested.map(str::trim) else {
        return Ok(Format::Gml(default));
    };
    let lf = f.to_lowercase().replace(' ', "");
    let gml = if lf == "gml2" || lf.contains("gml/2") {
        Some(GmlVersion::V2)
    } else if lf == "gml3" || lf.contains("gml/3.1") {
        Some(GmlVersion::V31)
    } else if lf == "gml32" || lf.contains("gml/3.2") || lf.contains("version=3.2") {
        Some(GmlVersion::V32)
    } else if lf == "text/xml" || lf == "application/xml" {
        Some(default)
    } else {
        None
    };
    if let Some(g) = gml {
        return Ok(Format::Gml(g));
    }
    let format = match lf.as_str() {
        "application/json"
        | "json"
        | "application/geo+json"
        | "application/geojson"
        | "geojson" => Format::GeoJson,
        "text/javascript" | "application/javascript" | "jsonp" => Format::Jsonp(
            format_options
                .get("callback")
                .cloned()
                .unwrap_or_else(|| "parseResponse".to_string()),
        ),
        "csv" | "text/csv" => Format::Csv,
        "shape-zip" | "application/zip" | "shp" | "shapezip" => Format::ShapeZip,
        "kml" | "application/vnd.google-earth.kml+xml" | "application/vnd.google-earth.kmlxml" => {
            Format::Kml
        }
        _ => {
            return Err(WfsError::invalid(
                "outputFormat",
                format!("Unsupported output format `{f}`"),
            ))
        }
    };
    Ok(format)
}

/// Default output CRS name notation per version
fn default_crs(version: Version, epsg: u16) -> Crs {
    match version {
        Version::V100 => Crs::new(epsg, CrsNotation::Epsg),
        _ => Crs::new(epsg, CrsNotation::Urn),
    }
}

fn check_output_crs(svc: &WfsService, def: &FeatureTypeDef, name: &str) -> WfsResult<Crs> {
    let invalid = || WfsError::invalid("srsName", format!("Unsupported srsName `{name}`"));
    let crs = Crs::parse(name).map_err(|_| invalid())?;
    let allowed = crs.epsg == def.srid || crs.epsg == 4326 || svc.cfg.other_crs.contains(&crs.epsg);
    if !allowed || !Crs::is_known(crs.epsg) {
        return Err(invalid());
    }
    Ok(crs)
}

/// Resolve type names of a query, including subtypes of abstract types (inheritance)
fn query_types<'a>(
    svc: &'a WfsService,
    names: &[TypeName],
    version: Version,
) -> WfsResult<Vec<&'a FeatureTypeEntry>> {
    crate::wfs::describe::resolve_types(svc, names, version).map_err(|mut e| {
        e.locator = Some("typeName".to_string());
        e
    })
}

fn parse_context(
    version: Version,
    query: &Query,
    def: &FeatureTypeDef,
    svc: &WfsService,
) -> ParseContext {
    let mut ctx = ParseContext::new(version.filter());
    ctx.namespaces = svc.prefix_map();
    for (p, u) in &query.namespaces {
        ctx.namespaces.insert(p.clone(), u.clone());
    }
    // literals without srsName are in the CRS of the query (srsName) or of the feature type
    match query.srs_name.as_deref().and_then(|n| Crs::parse(n).ok()) {
        Some(c) => {
            ctx.default_srid = c.epsg;
            ctx.default_swap_xy = version != Version::V100 && c.swap_xy();
            ctx.literal_srid = Some(c.epsg);
        }
        None => {
            ctx.default_srid = def.srid;
            ctx.default_swap_xy = version != Version::V100 && crs::is_geographic(def.srid);
        }
    }
    ctx
}

/// Build filter for a query on one type, combined with KVP BBOX and resource ids
fn build_filter(
    svc: &WfsService,
    req: &GetFeatureRequest,
    query: &Query,
    def: &FeatureTypeDef,
) -> WfsResult<Option<Filter>> {
    let ctx = parse_context(req.version, query, def, svc);
    let mut parts = Vec::new();
    match &query.filter {
        Some(FilterSource::Xml(xml)) => {
            let f = parse_filter_str(xml, &ctx).map_err(|e| WfsError::from_filter(e, "filter"))?;
            parts.push(f);
        }
        Some(FilterSource::Cql(cql)) => {
            // CQL literals follow the same axis order rules as filter encoding literals
            let f = crate::wfs::cql::parse_cql(cql, &ctx)
                .map_err(|e| WfsError::from_filter(e, "cql_filter"))?;
            parts.push(f);
        }
        None => {}
    }
    if let Some((coords, crs_name)) = &req.bbox {
        let (x1, y1, x2, y2) = if coords.len() == 6 {
            (coords[0], coords[1], coords[3], coords[4])
        } else {
            (coords[0], coords[1], coords[2], coords[3])
        };
        let (srid, swap) = match crs_name {
            Some(name) => {
                let c = Crs::parse(name)
                    .map_err(|_| WfsError::invalid("bbox", format!("Unsupported CRS `{name}`")))?;
                (Some(c.epsg), c.swap_xy())
            }
            // KVP BBOX without CRS: native CRS of the feature type (not the query srsName)
            None => (
                Some(def.srid),
                req.version != Version::V100 && crs::is_geographic(def.srid),
            ),
        };
        let rect = if swap {
            geo::Rect::new((y1, x1), (y2, x2))
        } else {
            geo::Rect::new((x1, y1), (x2, y2))
        };
        parts.push(Filter::Spatial {
            op: SpatialOp::BBox,
            property: None,
            operand: SpatialOperand::Geometry(GeometryLiteral {
                geometry: geo::Geometry::Polygon(rect.to_polygon()),
                srid,
                srs_name: crs_name.clone(),
            }),
            distance: None,
        });
    }
    if !req.feature_ids.is_empty() {
        parts.push(Filter::ResourceIds(
            req.feature_ids
                .iter()
                .map(|id| ResourceId {
                    rid: id.clone(),
                    version: None,
                    start_date: None,
                    end_date: None,
                })
                .collect(),
        ));
    }
    let filter = match parts.len() {
        0 => return Ok(None),
        1 => parts.pop().expect("one"),
        _ => Filter::And(parts),
    };
    let prepared =
        filter::prepare(&filter, &[def]).map_err(|e| WfsError::from_filter(e, "filter"))?;
    Ok(Some(prepared))
}

fn plan_adhoc(
    svc: &WfsService,
    req: &GetFeatureRequest,
    query: &Query,
) -> WfsResult<Vec<PlannedQuery>> {
    if query.type_names.len() > 1 {
        return Err(WfsError::option_not_supported(
            "typeNames",
            "Join queries are not supported yet",
        ));
    }
    let entries = query_types(svc, &query.type_names, req.version)?;
    // RESOURCEID must reference the requested type (WFS 2.0)
    if req.version.is_v2() && req.kvp && !req.feature_ids.is_empty() {
        for id in &req.feature_ids {
            if let Some((type_part, _)) = id.rsplit_once('.') {
                let other_type = svc.types.iter().any(|t| t.def.name.local == type_part);
                // any type requested by the queries of the request
                let matches = req.queries.iter().any(|q| match q {
                    QueryExpr::Adhoc(q) => q.type_names.iter().any(|t| t.local == type_part),
                    _ => false,
                }) || entries.iter().any(|e| e.def.name.local == type_part);
                if other_type && !matches {
                    return Err(WfsError::invalid(
                        "resourceId",
                        format!(
                            "Resource id `{id}` does not belong to the requested feature types"
                        ),
                    ));
                }
            }
        }
    }
    let mut planned = Vec::new();
    for entry in entries {
        let def = &entry.def;
        let filter = build_filter(svc, req, query, def)?;
        let resolver = |prefix: &str| query.namespaces.get(prefix).cloned();
        // `*`: all properties
        let properties =
            if query.property_names.is_empty() || query.property_names.iter().any(|p| p == "*") {
                None
            } else {
                let mut props = Vec::new();
                for name in &query.property_names {
                    let path = PropertyPath::parse(name, &resolver)
                        .map_err(|e| WfsError::invalid("propertyName", e.to_string()))?;
                    if filter::is_id_path(def, &path) {
                        continue;
                    }
                    let (idx, _, _) = filter::resolve_property(def, &path).ok_or_else(|| {
                        WfsError::invalid("propertyName", format!("Unknown property `{name}`"))
                    })?;
                    props.push(idx);
                }
                Some(props)
            };
        let mut sort = Vec::new();
        for (name, desc) in &query.sort_by {
            let path = PropertyPath::parse(name, &resolver)
                .map_err(|e| WfsError::invalid("sortBy", e.to_string()))?;
            let target = if filter::is_id_path(def, &path) {
                SortTarget::Id
            } else {
                let (idx, _, _) = filter::resolve_property(def, &path).ok_or_else(|| {
                    WfsError::invalid("sortBy", format!("Unknown property `{name}`"))
                })?;
                SortTarget::Property(idx)
            };
            sort.push(SortKey {
                target,
                descending: *desc,
            });
        }
        let crs = match &query.srs_name {
            Some(name) => OutputCrs::new(check_output_crs(svc, def, name)?, Some(name)),
            None => OutputCrs::new(default_crs(req.version, def.srid), None),
        };
        let mut xlink_depths = Vec::new();
        for (name, depth) in &query.xlink_depths {
            let path = PropertyPath::parse(name, &resolver)
                .map_err(|e| WfsError::invalid("propertyName", e.to_string()))?;
            let (idx, _, _) = filter::resolve_property(def, &path).ok_or_else(|| {
                WfsError::invalid("propertyName", format!("Unknown property `{name}`"))
            })?;
            xlink_depths.push((def.properties[idx].name.clone(), *depth));
        }
        planned.push(PlannedQuery {
            def: entry.def.clone(),
            store: entry.store.clone(),
            filter,
            sort,
            properties,
            crs,
            xlink_depths,
            query_index: 0,
        });
    }
    Ok(planned)
}

/// Plan all queries of a request
pub fn plan(svc: &WfsService, req: &GetFeatureRequest) -> WfsResult<Vec<PlannedQuery>> {
    let mut planned = Vec::new();
    if req.queries.is_empty() {
        // FEATUREID without TYPENAME: query all types by id
        let query = Query::default();
        for entry in &svc.types {
            let def = &entry.def;
            let filter = build_filter(svc, req, &query, def)?;
            planned.push(PlannedQuery {
                def: entry.def.clone(),
                store: entry.store.clone(),
                filter,
                sort: Vec::new(),
                properties: None,
                crs: OutputCrs::new(default_crs(req.version, def.srid), None),
                xlink_depths: Vec::new(),
                query_index: 0,
            });
        }
        return Ok(planned);
    }
    for (index, q) in req.queries.iter().enumerate() {
        match q {
            QueryExpr::Adhoc(query) => {
                // errors of a query with a handle are located by the handle
                let located = plan_adhoc(svc, req, query).map_err(|mut e| {
                    if let Some(h) = &query.handle {
                        e.locator = Some(h.clone());
                    }
                    e
                })?;
                planned.extend(located.into_iter().map(|mut p| {
                    p.query_index = index;
                    p
                }));
            }
            QueryExpr::Stored(_) => {
                return Err(WfsError::invalid(
                    "storedQuery_id",
                    "Stored query execution handled separately",
                ))
            }
        }
    }
    Ok(planned)
}

/// Per query (offset, limit) for global start index and count
pub fn distribute(counts: &[u64], start: u64, limit: Option<u64>) -> Vec<(u64, u64)> {
    let mut offset_rem = start;
    let mut limit_rem = limit.unwrap_or(u64::MAX);
    counts
        .iter()
        .map(|&n| {
            let skip = n.min(offset_rem);
            offset_rem -= skip;
            let take = (n - skip).min(limit_rem);
            limit_rem -= take;
            (skip, take)
        })
        .collect()
}

pub async fn get_feature(svc: &Arc<WfsService>, raw: &RawRequest) -> OpResult {
    let ev = raw.error_version();
    let req = parse_get_feature(raw, &svc.prefix_map(), false).map_err(|e| (ev, e))?;
    if let Some(QueryExpr::Stored(call)) = req.queries.first() {
        let call = call.clone();
        return crate::wfs::storedquery::execute(svc, &req, &call).await;
    }
    run_get_feature(svc, req).await
}

/// Execute a parsed GetFeature request with ad hoc queries
pub async fn run_get_feature(svc: &Arc<WfsService>, req: GetFeatureRequest) -> OpResult {
    let version = req.version;
    let err = |e: WfsError| (version, e);
    // join queries
    if let [QueryExpr::Adhoc(q)] = req.queries.as_slice() {
        if q.type_names.len() > 1 {
            let q = q.clone();
            return crate::wfs::join::run_join(svc, &req, &q).await;
        }
    }
    let format =
        output_format(version, req.output_format.as_deref(), &req.format_options).map_err(err)?;
    let content_type = format.content_type();
    let planned = plan(svc, &req).map_err(err)?;
    let limit = req.count.or(svc.cfg.count_default);
    let need_counts = version.is_v2()
        || matches!(format, Format::GeoJson | Format::Jsonp(_))
        || version == Version::V110
        || req.result_type == ResultType::Hits
        || (req.start_index > 0 && planned.len() > 1);
    // single query: the count runs concurrently with the first page of features
    if need_counts
        && planned.len() == 1
        && req.result_type == ResultType::Results
        && !svc.cfg.feature_bounding
    {
        if let Format::Gml(gml) = format {
            let doc_version = document_version(version, gml);
            let xlink = xlink_context(svc, &req, gml).map_err(err)?;
            let pq = &planned[0];
            let count = svc.count(&pq.def, &pq.store, pq.filter.clone());
            let body = feature_stream(
                planned.clone(),
                vec![(req.start_index, None)],
                limit,
                gml,
                String::new(),
                collection_footer(gml),
                xlink,
                doc_version == Version::V100,
                None,
            );
            let mut body = body.boxed();
            let (matched, first) = futures::join!(count, body.next());
            let matched = matched.map_err(|e| {
                error!("WFS count failed: {e}");
                err(WfsError::no_applicable(format!("Query failed: {e}")))
            })?;
            let first = match first {
                Some(Ok(b)) => b,
                Some(Err(e)) => return Err(err(WfsError::no_applicable(e))),
                None => Bytes::new(),
            };
            let available = matched.saturating_sub(req.start_index);
            let returned = limit.map(|l| l.min(available)).unwrap_or(available);
            let mut header = collection_header(
                svc,
                doc_version,
                gml,
                &planned,
                Some(matched),
                Some(returned),
                false,
            );
            if doc_version.is_v2() {
                header = with_paging_links(header, svc, &req, limit, matched, returned, false);
            }
            let rest = body.map(|r| r.map_err(actix_web::error::ErrorInternalServerError));
            let body = stream::once(async move { Ok(Bytes::from(header)) })
                .chain(stream::once(async move { Ok(first) }))
                .chain(rest)
                .boxed();
            return Ok(WfsResponse::xml(Body::Stream(body)).with_type(content_type));
        }
    }
    let counts: Option<Vec<u64>> = if need_counts {
        let futs = planned
            .iter()
            .map(|p| svc.count(&p.def, &p.store, p.filter.clone()));
        let counts = futures::future::try_join_all(futs).await.map_err(|e| {
            error!("WFS count failed: {e}");
            err(WfsError::no_applicable(format!("Query failed: {e}")))
        })?;
        Some(counts)
    } else {
        None
    };
    let matched: Option<u64> = counts.as_ref().map(|c| c.iter().sum());
    // paging per query
    let slices: Vec<(u64, Option<u64>)> = match &counts {
        Some(c) => distribute(c, req.start_index, limit)
            .into_iter()
            .map(|(s, t)| (s, Some(t)))
            .collect(),
        None => {
            // single query or no start index: offset on first query, limit applied while streaming
            planned
                .iter()
                .enumerate()
                .map(|(i, _)| (if i == 0 { req.start_index } else { 0 }, None))
                .collect()
        }
    };
    let returned: Option<u64> = counts
        .as_ref()
        .map(|_| slices.iter().map(|(_, t)| t.unwrap_or(0)).sum());
    let hits = req.result_type == ResultType::Hits;
    let gml = match &format {
        Format::Gml(g) => *g,
        _ => {
            return other_format(svc, &req, planned, slices, limit, format, matched, hits)
                .await
                .map_err(err);
        }
    };
    let doc_version = document_version(version, gml);
    let mut header = collection_header(
        svc,
        doc_version,
        gml,
        &planned,
        matched,
        if hits { Some(0) } else { returned },
        hits,
    );
    if svc.cfg.feature_bounding && !hits {
        if let Some(env) = result_bounds(&planned, &slices, limit, gml)
            .await
            .map_err(err)?
        {
            header = match doc_version {
                Version::V100 => header.replacen(
                    "<gml:boundedBy><gml:null>unknown</gml:null></gml:boundedBy>",
                    &format!("<gml:boundedBy>{env}</gml:boundedBy>"),
                    1,
                ),
                _ => {
                    let el = if doc_version.is_v2() {
                        "wfs:boundedBy"
                    } else {
                        "gml:boundedBy"
                    };
                    let start = header.find("<wfs:FeatureCollection").unwrap_or(0);
                    let end = header[start..]
                        .find('>')
                        .map(|i| start + i + 1)
                        .unwrap_or(header.len());
                    format!("{}<{el}>{env}</{el}>{}", &header[..end], &header[end..])
                }
            };
        }
    }
    if doc_version.is_v2() {
        header = with_paging_links(
            header,
            svc,
            &req,
            limit,
            matched.unwrap_or(0),
            returned.unwrap_or(0),
            hits,
        );
    }
    let footer = collection_footer(gml);
    if hits {
        let mut doc = header;
        doc.push_str(&footer);
        return Ok(WfsResponse::xml(Body::Text(doc)).with_type(content_type));
    }
    let xlink = xlink_context(svc, &req, gml).map_err(err)?;
    let bounding = doc_version == Version::V100 || svc.cfg.feature_bounding;
    // WFS 2.0 with several queries: nested collections with per query counts
    let queries = planned
        .iter()
        .map(|p| p.query_index)
        .max()
        .map(|m| m + 1)
        .unwrap_or(0);
    let nested = match &counts {
        Some(c) if doc_version.is_v2() && queries > 1 => {
            let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
            Some(
                (0..queries)
                    .map(|qi| {
                        let idx = planned.iter().enumerate().filter(|(_, p)| p.query_index == qi);
                        let m: u64 = idx.clone().map(|(i, _)| c[i]).sum();
                        let r: u64 = idx.map(|(i, _)| slices[i].1.unwrap_or(0)).sum();
                        format!(r#"<wfs:member><wfs:FeatureCollection timeStamp="{timestamp}" numberMatched="{m}" numberReturned="{r}">"#)
                    })
                    .collect(),
            )
        }
        _ => None,
    };
    let stream = feature_stream(
        planned, slices, limit, gml, header, footer, xlink, bounding, nested,
    );
    // fetch first chunk before committing the response
    let mut stream = stream.boxed();
    let first = stream.next().await;
    let first = match first {
        Some(Ok(b)) => b,
        Some(Err(e)) => return Err(err(WfsError::no_applicable(e))),
        None => Bytes::new(),
    };
    let rest = stream.map(|r| r.map_err(actix_web::error::ErrorInternalServerError));
    let body = stream::once(async move { Ok(first) }).chain(rest).boxed();
    Ok(WfsResponse::xml(Body::Stream(body)).with_type(content_type))
}

/// Version of the collection document for a GML version (e.g. GML 3.1 output of a 2.0 request)
fn document_version(version: Version, gml: GmlVersion) -> Version {
    match (version, gml) {
        (v, GmlVersion::V2) if v != Version::V100 => Version::V100,
        (v, GmlVersion::V31) if v != Version::V110 => Version::V110,
        (v, GmlVersion::V32) if !v.is_v2() => Version::V200,
        (v, _) => v,
    }
}

/// next / previous attributes added to a 2.0 collection header
fn with_paging_links(
    header: String,
    svc: &WfsService,
    req: &GetFeatureRequest,
    limit: Option<u64>,
    matched: u64,
    returned: u64,
    hits: bool,
) -> String {
    let links = paging_links(svc, req, limit, matched, returned, hits);
    if links.is_empty() {
        header
    } else {
        header.replacen(" timeStamp=", &format!("{links} timeStamp="), 1)
    }
}

/// XLink resolution settings of a request
#[allow(clippy::type_complexity)]
fn xlink_context(
    svc: &Arc<WfsService>,
    req: &GetFeatureRequest,
    gml: GmlVersion,
) -> WfsResult<Option<(crate::wfs::xlink::XlinkCtx, Option<u32>)>> {
    let version = req.version;
    if version == Version::V110 && req.resolve_timeout == Some(0) {
        return Err(WfsError::no_applicable(
            "XLink resolution expired (traverseXlinkExpiry=0)",
        ));
    }
    Ok(match req.resolve {
        crate::wfs::query::Resolve::None => None,
        r => {
            let timeout = req.resolve_timeout.map(|t| {
                std::time::Duration::from_secs(if version == Version::V110 { t * 60 } else { t })
            });
            let (local, remote) = match r {
                crate::wfs::query::Resolve::Local => (true, false),
                crate::wfs::query::Resolve::Remote => (false, svc.cfg.remote_resolve),
                _ => (true, svc.cfg.remote_resolve),
            };
            Some((
                crate::wfs::xlink::XlinkCtx::new(svc, version, gml, local, remote, timeout),
                req.resolve_depth,
            ))
        }
    })
}

fn used_namespaces(
    svc: &WfsService,
    planned: &[PlannedQuery],
) -> Vec<(String, String, Vec<String>)> {
    let mut result: Vec<(String, String, Vec<String>)> = Vec::new();
    for p in planned {
        let name = &p.def.name;
        match result.iter_mut().find(|(_, uri, _)| *uri == name.ns) {
            Some((_, _, types)) => {
                if !types.contains(&name.prefixed()) {
                    types.push(name.prefixed())
                }
            }
            None => result.push((name.prefix.clone(), name.ns.clone(), vec![name.prefixed()])),
        }
    }
    let _ = svc;
    result
}

fn collection_header(
    svc: &WfsService,
    version: Version,
    gml: GmlVersion,
    planned: &[PlannedQuery],
    matched: Option<u64>,
    returned: Option<u64>,
    _hits: bool,
) -> String {
    let nss = used_namespaces(svc, planned);
    let mut ns_decls = String::new();
    for (prefix, uri, _) in &nss {
        if !prefix.is_empty() {
            ns_decls.push_str(&format!(
                r#" xmlns:{prefix}="{}""#,
                crate::wfs::xml::escape(uri)
            ));
        }
    }
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let wfs_ns = version.wfs_ns();
    let gml_ns = if gml == GmlVersion::V32 {
        "http://www.opengis.net/gml/3.2"
    } else {
        "http://www.opengis.net/gml"
    };
    let common = format!(
        r#" xmlns:wfs="{wfs_ns}" xmlns:gml="{gml_ns}" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"{ns_decls}"#
    );
    // application schemas (DescribeFeatureType) of the namespaces of the result
    let app_hints: String = nss
        .iter()
        .map(|(_, uri, types)| {
            format!(
                " {} {}",
                crate::wfs::xml::escape(uri),
                describe_url(svc, version, types)
            )
        })
        .collect();
    match version {
        Version::V100 => format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><wfs:FeatureCollection{common} xsi:schemaLocation="http://www.opengis.net/wfs http://schemas.opengis.net/wfs/1.0.0/WFS-basic.xsd{app_hints}"><gml:boundedBy><gml:null>unknown</gml:null></gml:boundedBy>"#
        ),
        Version::V110 => format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><wfs:FeatureCollection numberOfFeatures="{}" timeStamp="{timestamp}"{common} xsi:schemaLocation="http://www.opengis.net/wfs http://schemas.opengis.net/wfs/1.1.0/wfs.xsd{app_hints}">"#,
            if _hits {
                matched.unwrap_or(0)
            } else {
                returned.or(matched).unwrap_or(0)
            }
        ),
        _ => {
            let mut hints =
                "http://www.opengis.net/wfs/2.0 http://schemas.opengis.net/wfs/2.0/wfs.xsd"
                    .to_string();
            for (_, uri, types) in &nss {
                hints.push_str(&format!(
                    " {} {}",
                    crate::wfs::xml::escape(uri),
                    describe_url(svc, version, types)
                ));
            }
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><wfs:FeatureCollection timeStamp="{timestamp}" numberMatched="{}" numberReturned="{}"{common} xsi:schemaLocation="{hints}">"#,
                matched
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                returned.unwrap_or(0)
            )
        }
    }
}

fn collection_footer(_gml: GmlVersion) -> String {
    "</wfs:FeatureCollection>".to_string()
}

/// Extent of the features of a result in the output CRS of the first query, as GML envelope
async fn result_bounds(
    planned: &[PlannedQuery],
    slices: &[(u64, Option<u64>)],
    limit: Option<u64>,
    gml: GmlVersion,
) -> Result<Option<String>, WfsError> {
    let Some(first) = planned.first() else {
        return Ok(None);
    };
    let mut extent: Option<geo::Rect<f64>> = None;
    for (pq, (offset, take)) in planned.iter().zip(slices) {
        let geoms: Vec<usize> = pq
            .def
            .properties
            .iter()
            .enumerate()
            .filter(|(_, p)| p.is_geometry())
            .map(|(i, _)| i)
            .collect();
        if geoms.is_empty() {
            continue;
        }
        let q = StoreQuery {
            filter: pq.filter.clone(),
            sort: pq.sort.clone(),
            offset: *offset,
            limit: take.or(limit),
            properties: Some(geoms),
        };
        let encoder = FeatureEncoder {
            def: &pq.def,
            gml,
            crs: pq.crs.clone(),
            properties: None,
            feature_bounding: false,
        };
        let mut features = pq.store.query(q);
        while let Some(f) = features.next().await {
            let f = f.map_err(|e| WfsError::no_applicable(format!("Query failed: {e}")))?;
            let Some(r) = f.bbox() else { continue };
            let Some(mut g) = encoder.transform(&geo::Geometry::Polygon(r.to_polygon())) else {
                continue;
            };
            if pq.crs.epsg != first.crs.epsg
                && crs::transform(&mut g, pq.crs.epsg, first.crs.epsg).is_err()
            {
                continue;
            }
            if let Some(r) = geo::BoundingRect::bounding_rect(&g) {
                extent = Some(match extent {
                    None => r,
                    Some(e) => geo::Rect::new(
                        geo::coord! { x: e.min().x.min(r.min().x), y: e.min().y.min(r.min().y) },
                        geo::coord! { x: e.max().x.max(r.max().x), y: e.max().y.max(r.max().y) },
                    ),
                });
            }
        }
    }
    Ok(extent.map(|r| {
        let mut out = String::new();
        crate::wfs::gml::write_envelope(
            &mut out,
            &r,
            &crate::wfs::gml::GmlWriteOpts {
                version: gml,
                srs_name: Some(&first.crs.name),
                swap_xy: first.crs.swap_xy,
                id: None,
                meta: None,
                z: None,
            },
        );
        out
    }))
}

#[allow(clippy::too_many_arguments)]
fn feature_stream(
    planned: Vec<PlannedQuery>,
    slices: Vec<(u64, Option<u64>)>,
    limit: Option<u64>,
    gml: GmlVersion,
    header: String,
    footer: String,
    xlink: Option<(crate::wfs::xlink::XlinkCtx, Option<u32>)>,
    feature_bounding: bool,
    nested: Option<Vec<String>>,
) -> impl futures::Stream<Item = Result<Bytes, String>> {
    let (member_open, member_close) = match gml {
        GmlVersion::V32 => ("<wfs:member>", "</wfs:member>"),
        _ => ("<gml:featureMember>", "</gml:featureMember>"),
    };
    async_stream::stream! {
        let mut buf = String::with_capacity(CHUNK_SIZE + 4096);
        buf.push_str(&header);
        // remaining global limit when per-query limits are not known in advance
        let mut remaining: Option<u64> = limit;
        // WFS 2.0 multiple queries: one nested feature collection per query
        let mut open_query: Option<usize> = None;
        for (pq, (offset, take)) in planned.iter().zip(slices) {
            if let Some(tags) = &nested {
                if open_query != Some(pq.query_index) {
                    if open_query.is_some() {
                        buf.push_str("</wfs:FeatureCollection></wfs:member>");
                    }
                    buf.push_str(&tags[pq.query_index]);
                    open_query = Some(pq.query_index);
                }
            }
            let lim: Option<u64> = take.or(remaining);
            if lim == Some(0) {
                continue;
            }
            let q = StoreQuery {
                filter: pq.filter.clone(),
                sort: pq.sort.clone(),
                offset,
                limit: lim,
                properties: pq.properties.as_ref().map(|props| {
                    let mut p = props.clone();
                    for (i, d) in pq.def.properties.iter().enumerate() {
                        // mandatory properties, and geometries for feature bounding boxes
                        if (d.min_occurs > 0 || (feature_bounding && d.is_geometry())) && !p.contains(&i) {
                            p.push(i);
                        }
                    }
                    p
                }),
            };
            let encoder = FeatureEncoder {
                def: &pq.def,
                gml,
                crs: pq.crs.clone(),
                properties: pq.properties.clone(),
                feature_bounding,
            };
            let mut features = pq.store.query(q);
            while let Some(r) = features.next().await {
                match r {
                    Ok(f) => {
                        buf.push_str(member_open);
                        match &xlink {
                            None => encoder.write_feature(&mut buf, &f),
                            Some((ctx, depth)) => {
                                let mut fxml = String::new();
                                encoder.write_feature(&mut fxml, &f);
                                match crate::wfs::xlink::resolve_feature(fxml, *depth, &pq.xlink_depths, vec![f.id.clone()], ctx).await {
                                    Ok(resolved) => buf.push_str(&resolved),
                                    Err(e) => {
                                        yield Err(e.text);
                                        return;
                                    }
                                }
                            }
                        }
                        buf.push_str(member_close);
                        remaining = remaining.map(|r| r.saturating_sub(1));
                        if buf.len() >= CHUNK_SIZE {
                            yield Ok(Bytes::from(std::mem::replace(&mut buf, String::with_capacity(CHUNK_SIZE + 4096))));
                        }
                    }
                    Err(e) => {
                        error!("WFS GetFeature failed: {e}");
                        yield Err(e.to_string());
                        return;
                    }
                }
            }
        }
        if open_query.is_some() {
            buf.push_str("</wfs:FeatureCollection></wfs:member>");
        }
        buf.push_str(&footer);
        yield Ok(Bytes::from(buf));
    }
}

/// Responses in non-GML formats
#[allow(clippy::too_many_arguments)]
async fn other_format(
    svc: &WfsService,
    req: &GetFeatureRequest,
    planned: Vec<PlannedQuery>,
    slices: Vec<(u64, Option<u64>)>,
    limit: Option<u64>,
    format: Format,
    matched: Option<u64>,
    hits: bool,
) -> WfsResult<WfsResponse> {
    use crate::wfs::formats::*;
    use futures::TryStreamExt;
    let content_type = format.content_type();
    let options = &req.format_options;
    // file name: format option `filename`, else the type names
    let base_name = {
        let types: Vec<&str> = planned.iter().map(|p| p.def.name.local.as_str()).collect();
        if types.is_empty() {
            "features".to_string()
        } else {
            types.join("_")
        }
    };
    let file_name = |ext: &str| match options.get("filename") {
        Some(f) if f.to_lowercase().ends_with(&format!(".{ext}")) => f.clone(),
        Some(f) => format!("{f}.{ext}"),
        None => format!("{base_name}.{ext}"),
    };
    if format == Format::ShapeZip {
        let shp_opts = ShapeZipOptions::from_options(options)
            .map_err(|e| WfsError::invalid("format_options", e))?;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let mut remaining = limit;
        for (pq, (offset, take)) in planned.iter().zip(slices) {
            let lim = take.or(remaining);
            let features: Vec<Feature> = if lim == Some(0) {
                Vec::new()
            } else {
                let q = StoreQuery {
                    filter: pq.filter.clone(),
                    sort: pq.sort.clone(),
                    offset,
                    limit: lim,
                    properties: None,
                };
                pq.store
                    .query(q)
                    .try_collect()
                    .await
                    .map_err(|e| WfsError::no_applicable(e.to_string()))?
            };
            remaining = remaining.map(|r| r.saturating_sub(features.len() as u64));
            shape_zip(&mut zip, &pq.def, &pq.crs, &features, &shp_opts)
                .map_err(WfsError::no_applicable)?;
        }
        // request that produced the archive (GeoServer adds it as <type>.txt)
        let request_url = format!(
            "{}?{}",
            svc.endpoint_url,
            serde_urlencoded::to_string(link_params(req)).unwrap_or_default()
        );
        {
            use std::io::Write;
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file(format!("{base_name}.txt"), opts)
                .map_err(|e| WfsError::no_applicable(e.to_string()))?;
            zip.write_all(request_url.as_bytes())
                .map_err(|e| WfsError::no_applicable(e.to_string()))?;
        }
        let data = zip
            .finish()
            .map_err(|e| WfsError::no_applicable(e.to_string()))?
            .into_inner();
        let mut resp = WfsResponse::xml(Body::Bytes(data)).with_type(content_type);
        resp.headers.push((
            "Content-Disposition".into(),
            format!("attachment; filename={}", file_name("zip")),
        ));
        return Ok(resp);
    }
    let crs_for_json = planned.first().map(|p| p.crs.clone());
    let csv_sep = if format == Format::Csv {
        csv_separator(options).map_err(|e| WfsError::invalid("format_options", e))?
    } else {
        ','
    };
    let id_policy = IdPolicy::from_options(options);
    // GeoJSON paging links (WFS 2.0, when startIndex is given)
    let start_given = req.start_index > 0
        || req
            .kvp_params
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("startindex"));
    let mut links: Vec<(&'static str, String)> = Vec::new();
    if req.version.is_v2()
        && start_given
        && matches!(format, Format::GeoJson | Format::Jsonp(_))
        && !hits
    {
        let returned: u64 = slices.iter().map(|(_, t)| t.unwrap_or(0)).sum();
        let start = req.start_index;
        if start > 0 {
            let (ps, pc) = match limit {
                Some(l) => (start.saturating_sub(l), l.min(start)),
                None => (0, start),
            };
            links.push(("previous", paging_url(svc, req, ps, Some(pc), None)));
        }
        if let Some(m) = matched {
            if returned > 0 && start + returned < m {
                links.push((
                    "next",
                    paging_url(svc, req, start + returned, limit.or(Some(returned)), None),
                ));
            }
        }
    }
    let disposition = match &format {
        Format::Csv => Some(format!("attachment; filename={}", file_name("csv"))),
        Format::GeoJson => Some(format!("inline; filename={}", file_name("json"))),
        _ => None,
    };
    let stream = async_stream::stream! {
        let mut buf = String::with_capacity(CHUNK_SIZE + 4096);
        match &format {
            Format::GeoJson => buf.push_str(r#"{"type":"FeatureCollection","features":["#),
            Format::Jsonp(cb) => {
                buf.push_str(cb);
                buf.push_str(r#"({"type":"FeatureCollection","features":["#);
            }
            Format::Kml => buf.push_str(KML_HEADER),
            _ => {}
        }
        let mut remaining = limit;
        let mut count: u64 = 0;
        let mut bbox: Option<[f64; 4]> = None;
        let json = matches!(format, Format::GeoJson | Format::Jsonp(_));
        if !hits {
            for (qi, (pq, (offset, take))) in planned.iter().zip(slices).enumerate() {
                if format == Format::Csv && qi == 0 {
                    csv_header(&mut buf, &pq.def, &pq.properties, csv_sep);
                }
                let lim = take.or(remaining);
                if lim == Some(0) {
                    continue;
                }
                let q = StoreQuery { filter: pq.filter.clone(), sort: pq.sort.clone(), offset, limit: lim, properties: None };
                let mut features = pq.store.query(q);
                while let Some(r) = features.next().await {
                    let f = match r {
                        Ok(f) => f,
                        Err(e) => {
                            yield Err(e.to_string());
                            return;
                        }
                    };
                    match &format {
                        Format::GeoJson | Format::Jsonp(_) => {
                            if count > 0 {
                                buf.push(',');
                            }
                            geojson_feature(&mut buf, &pq.def, &pq.crs, &f, &pq.properties, &id_policy);
                        }
                        Format::Csv => csv_row(&mut buf, &pq.def, &pq.crs, &f, &pq.properties, csv_sep),
                        Format::Kml => kml_placemark(&mut buf, &pq.def, &f, &pq.properties),
                        _ => {}
                    }
                    if json {
                        // collection extent in output coordinates (east/north)
                        let encoder = FeatureEncoder { def: &pq.def, gml: GmlVersion::V32, crs: pq.crs.clone(), properties: None, feature_bounding: false };
                        if let Some(r) = f.bbox().and_then(|r| encoder.transform(&geo::Geometry::Polygon(r.to_polygon()))).and_then(|g| geo::BoundingRect::bounding_rect(&g)) {
                            bbox = Some(match bbox {
                                None => [r.min().x, r.min().y, r.max().x, r.max().y],
                                Some(b) => [b[0].min(r.min().x), b[1].min(r.min().y), b[2].max(r.max().x), b[3].max(r.max().y)],
                            });
                        }
                    }
                    count += 1;
                    remaining = remaining.map(|r| r.saturating_sub(1));
                    if buf.len() >= CHUNK_SIZE {
                        yield Ok(Bytes::from(std::mem::replace(&mut buf, String::with_capacity(CHUNK_SIZE + 4096))));
                    }
                }
            }
        }
        match &format {
            Format::GeoJson => geojson_footer(&mut buf, matched, count, crs_for_json.as_ref(), bbox, &links),
            Format::Jsonp(_) => {
                geojson_footer(&mut buf, matched, count, crs_for_json.as_ref(), bbox, &links);
                buf.push(')');
            }
            Format::Kml => buf.push_str(KML_FOOTER),
            _ => {}
        }
        yield Ok(Bytes::from(buf));
    };
    let mut stream = stream.boxed();
    let first = match stream.next().await {
        Some(Ok(b)) => b,
        Some(Err(e)) => return Err(WfsError::no_applicable(e)),
        None => Bytes::new(),
    };
    let rest = stream.map(|r| r.map_err(actix_web::error::ErrorInternalServerError));
    let body = stream::once(async move { Ok(first) }).chain(rest).boxed();
    let mut resp = WfsResponse::xml(Body::Stream(body)).with_type(content_type);
    if let Some(d) = disposition {
        resp.headers.push(("Content-Disposition".into(), d));
    }
    Ok(resp)
}

/// KVP parameters equivalent to a request (for links)
fn link_params(req: &GetFeatureRequest) -> Vec<(String, String)> {
    if req.kvp {
        return req.kvp_params.clone();
    }
    let mut params = vec![
        ("service".to_string(), "WFS".to_string()),
        ("version".to_string(), req.version.label().to_string()),
        ("request".to_string(), "GetFeature".to_string()),
    ];
    if let Some(f) = &req.output_format {
        params.push(("outputFormat".to_string(), f.clone()));
    }
    let adhoc: Vec<&Query> = req
        .queries
        .iter()
        .filter_map(|q| match q {
            QueryExpr::Adhoc(q) => Some(q),
            _ => None,
        })
        .collect();
    let multi = adhoc.len() > 1;
    let group = |items: Vec<String>| -> String {
        if multi {
            items.iter().map(|i| format!("({i})")).collect()
        } else {
            items.join(",")
        }
    };
    let names = |q: &Query| -> String {
        q.type_names
            .iter()
            .map(|t| match (&t.prefix, &t.ns) {
                (Some(p), _) => format!("{p}:{}", t.local),
                (None, Some(ns)) => format!("{{{ns}}}{}", t.local),
                _ => t.local.clone(),
            })
            .collect::<Vec<_>>()
            .join(",")
    };
    params.push((
        "typeNames".to_string(),
        group(adhoc.iter().map(|q| names(q)).collect()),
    ));
    let mut nss: Vec<String> = Vec::new();
    for q in &adhoc {
        for t in &q.type_names {
            if let (Some(p), Some(ns)) = (&t.prefix, &t.ns) {
                let d = format!("xmlns({p},{ns})");
                if !nss.contains(&d) {
                    nss.push(d);
                }
            }
        }
    }
    if !nss.is_empty() {
        params.push(("namespaces".to_string(), nss.join(",")));
    }
    let filters: Vec<String> = adhoc
        .iter()
        .map(|q| match &q.filter {
            Some(FilterSource::Xml(x)) => x.clone(),
            _ => String::new(),
        })
        .collect();
    if filters.iter().any(|f| !f.is_empty()) {
        params.push(("filter".to_string(), group(filters)));
    }
    if let Some(srs) = adhoc.first().and_then(|q| q.srs_name.clone()) {
        params.push(("srsName".to_string(), srs));
    }
    let props: Vec<String> = adhoc.iter().map(|q| q.property_names.join(",")).collect();
    if props.iter().any(|p| !p.is_empty()) {
        params.push(("propertyName".to_string(), group(props)));
    }
    let sorts: Vec<String> = adhoc
        .iter()
        .map(|q| {
            q.sort_by
                .iter()
                .map(|(n, d)| format!("{n} {}", if *d { "DESC" } else { "ASC" }))
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect();
    if sorts.iter().any(|s| !s.is_empty()) {
        params.push(("sortBy".to_string(), group(sorts)));
    }
    params
}

fn paging_url(
    svc: &WfsService,
    req: &GetFeatureRequest,
    start: u64,
    count: Option<u64>,
    result_type: Option<&str>,
) -> String {
    let skip = ["startindex", "count", "maxfeatures", "resulttype"];
    let mut params: Vec<(String, String)> = link_params(req)
        .into_iter()
        .filter(|(k, _)| !skip.contains(&k.to_lowercase().as_str()))
        .collect();
    if let Some(count) = count {
        params.push(("count".to_string(), count.to_string()));
    }
    params.push(("startIndex".to_string(), start.to_string()));
    if let Some(rt) = result_type {
        params.push(("resultType".to_string(), rt.to_string()));
    }
    let query = serde_urlencoded::to_string(&params).unwrap_or_default();
    format!("{}?{query}", svc.endpoint_url)
}

/// next / previous attributes of a 2.0 feature collection
fn paging_links(
    svc: &WfsService,
    req: &GetFeatureRequest,
    limit: Option<u64>,
    matched: u64,
    returned: u64,
    hits: bool,
) -> String {
    let start = req.start_index;
    let mut links = String::new();
    let mut add = |rel: &str, url: String| {
        links.push_str(&format!(r#" {rel}="{}""#, crate::wfs::xml::escape(&url)));
    };
    if hits {
        // the first page of results
        if matched > 0 && (req.count.is_some() || start > 0) {
            add("next", paging_url(svc, req, 0, req.count, Some("results")));
        }
        return links;
    }
    if returned > 0 && start + returned < matched {
        add(
            "next",
            paging_url(svc, req, start + returned, limit.or(Some(returned)), None),
        );
    }
    if start > 0 {
        let (prev_start, prev_count) = match limit {
            Some(l) => (start.saturating_sub(l), l.min(start)),
            None => (0, start),
        };
        add(
            "previous",
            paging_url(svc, req, prev_start, Some(prev_count), None),
        );
    }
    links
}

/// GetPropertyValue: values of one property of the selected features
pub async fn get_property_value(svc: &Arc<WfsService>, raw: &RawRequest) -> OpResult {
    use futures::TryStreamExt;
    let ev = raw.error_version();
    let req = parse_get_feature(raw, &svc.prefix_map(), true).map_err(|e| (ev, e))?;
    let version = req.version;
    let err = |e: WfsError| (version, e);
    let value_ref = req.value_reference.clone().unwrap_or_default();
    if value_ref.trim().is_empty() {
        return Err(err(WfsError::invalid(
            "valueReference",
            "Empty valueReference",
        )));
    }
    let planned = plan(svc, &req).map_err(err)?;
    let namespaces = match req.queries.first() {
        Some(QueryExpr::Adhoc(q)) => q.namespaces.clone(),
        _ => svc.prefix_map(),
    };
    let resolver = |prefix: &str| namespaces.get(prefix).cloned();
    let path = PropertyPath::parse(&value_ref, &resolver)
        .map_err(|e| err(WfsError::invalid("valueReference", e.to_string())))?;
    for pq in &planned {
        if !filter::is_id_path(&pq.def, &path) && filter::resolve_property(&pq.def, &path).is_none()
        {
            return Err(err(WfsError::invalid(
                "valueReference",
                format!("Unknown property `{value_ref}`"),
            )));
        }
    }
    let limit = req.count.or(svc.cfg.count_default);
    let futs = planned
        .iter()
        .map(|p| svc.count(&p.def, &p.store, p.filter.clone()));
    let counts = futures::future::try_join_all(futs)
        .await
        .map_err(|e| err(WfsError::no_applicable(e.to_string())))?;
    let matched: u64 = counts.iter().sum();
    let slices = distribute(&counts, req.start_index, limit);
    let gml = version.gml();
    let mut members = Vec::new();
    if req.result_type == ResultType::Results {
        for (pq, (offset, take)) in planned.iter().zip(slices) {
            if take == 0 {
                continue;
            }
            let q = StoreQuery {
                filter: pq.filter.clone(),
                sort: pq.sort.clone(),
                offset,
                limit: Some(take),
                properties: None,
            };
            let features: Vec<_> = pq
                .store
                .query(q)
                .try_collect()
                .await
                .map_err(|e| err(WfsError::no_applicable(e.to_string())))?;
            let encoder = FeatureEncoder {
                def: &pq.def,
                gml,
                crs: pq.crs.clone(),
                properties: None,
                feature_bounding: false,
            };
            for f in &features {
                if filter::is_id_path(&pq.def, &path) {
                    members.push(crate::wfs::xml::escape(&f.id));
                    continue;
                }
                let (idx, _, first) = filter::resolve_property(&pq.def, &path).expect("checked");
                match &f.values[idx] {
                    Value::Null => {}
                    Value::Geometry(g) if path.steps.len() == first + 1 => {
                        if let Some(geom) = encoder.transform(&g.geometry) {
                            let mut s = String::new();
                            let gid = g.gml_id.clone().unwrap_or_else(|| {
                                format!("{}.{}", f.id, pq.def.properties[idx].name.local)
                            });
                            encoder.write_geometry(&mut s, &geom, Some(&gid));
                            members.push(s);
                        }
                    }
                    _ => {
                        let src = filter::FeatureSource {
                            feature_type: &pq.def,
                            feature: f,
                        };
                        use filter::PropertySource;
                        if let Ok(r) = src.resolve(&path) {
                            for v in r.vals {
                                if let Some(text) = v.as_string() {
                                    members.push(crate::wfs::xml::escape(&text));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let nss = used_namespaces(svc, &planned);
    let mut decls = String::new();
    for (prefix, uri, _) in &nss {
        decls.push_str(&format!(
            r#" xmlns:{prefix}="{}""#,
            crate::wfs::xml::escape(uri)
        ));
    }
    let gml_ns = version.gml_ns();
    let mut doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><wfs:ValueCollection timeStamp="{timestamp}" numberMatched="{matched}" numberReturned="{}" xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:gml="{gml_ns}" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"{decls} xsi:schemaLocation="http://www.opengis.net/wfs/2.0 http://schemas.opengis.net/wfs/2.0/wfs.xsd">"#,
        members.len()
    );
    for m in &members {
        doc.push_str("<wfs:member>");
        doc.push_str(m);
        doc.push_str("</wfs:member>");
    }
    doc.push_str("</wfs:ValueCollection>");
    Ok(WfsResponse::xml(Body::Text(doc)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribute_paging() {
        assert_eq!(distribute(&[15, 7], 0, Some(20)), vec![(0, 15), (0, 5)]);
        assert_eq!(distribute(&[15, 7], 10, Some(10)), vec![(10, 5), (0, 5)]);
        assert_eq!(distribute(&[15, 7], 20, None), vec![(15, 0), (5, 2)]);
        assert_eq!(distribute(&[3], 0, None), vec![(0, 3)]);
    }

    #[test]
    fn output_formats() {
        let opts = std::collections::HashMap::new();
        assert_eq!(
            output_format(Version::V100, None, &opts).unwrap(),
            Format::Gml(GmlVersion::V2)
        );
        assert_eq!(
            output_format(Version::V200, Some("GML2"), &opts).unwrap(),
            Format::Gml(GmlVersion::V2)
        );
        assert_eq!(
            output_format(Version::V110, Some("text/xml; subtype=gml/3.1.1"), &opts).unwrap(),
            Format::Gml(GmlVersion::V31)
        );
        assert_eq!(
            output_format(Version::V200, Some("application/json"), &opts).unwrap(),
            Format::GeoJson
        );
        assert_eq!(
            output_format(Version::V200, Some("SHAPE-ZIP"), &opts).unwrap(),
            Format::ShapeZip
        );
        assert!(output_format(Version::V100, Some("DUMMYFORMAT"), &opts).is_err());
    }
}
