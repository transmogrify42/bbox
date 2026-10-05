//! Join queries (WFS 2.0 standard, spatial and temporal joins) returning wfs:Tuple members.
//!
//! Conjuncts referencing a single member are pushed down to that member's store (SQL); join
//! predicates are evaluated with a hash join (equality), an R-tree (spatial) or nested loops.

use crate::wfs::endpoint::{Body, OpResult, WfsResponse};
use crate::wfs::exception::{WfsError, WfsResult};
use crate::wfs::filter::{
    self, evaluate, join_member, parse_filter_str, value_to_vals, ComparisonOp, EvalContext, Expr,
    Filter, PropertyPath, SpatialOp, SpatialOperand, TupleSource,
};
use crate::wfs::gml::GmlVersion;
use crate::wfs::model::*;
use crate::wfs::output::{FeatureEncoder, OutputCrs};
use crate::wfs::query::{FilterSource, GetFeatureRequest, Query, ResultType};
use crate::wfs::service::{FeatureTypeEntry, WfsService};
use crate::wfs::store::StoreQuery;
use futures::TryStreamExt;
use geo::BoundingRect;
use rstar::{RTree, RTreeObject, AABB};
use std::collections::HashMap;
use std::sync::Arc;

/// Maximum number of candidate pairs evaluated by nested loops
const MAX_PAIRS: usize = 50_000_000;

struct Member<'a> {
    entry: &'a FeatureTypeEntry,
    alias: Option<String>,
}

fn conjuncts(f: Filter) -> Vec<Filter> {
    match f {
        Filter::And(parts) => parts.into_iter().flat_map(conjuncts).collect(),
        other => vec![other],
    }
}

/// Members referenced by a filter
fn referenced(f: &Filter, defs: &[(&FeatureTypeDef, Option<&str>)]) -> WfsResult<Vec<usize>> {
    let mut idx = Vec::new();
    for p in f.properties() {
        let (i, _) = join_member(p, defs)
            .ok_or_else(|| WfsError::invalid("filter", format!("Unknown property `{}`", p.text)))?;
        if !idx.contains(&i) {
            idx.push(i);
        }
    }
    Ok(idx)
}

/// Property path of an expression in member `i`, relative to the member
fn member_path(
    e: &Expr,
    defs: &[(&FeatureTypeDef, Option<&str>)],
) -> Option<(usize, PropertyPath)> {
    match e {
        Expr::Property(p) => join_member(p, defs),
        _ => None,
    }
}

struct Envelope {
    rect: AABB<[f64; 2]>,
    index: usize,
}

impl RTreeObject for Envelope {
    type Envelope = AABB<[f64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        self.rect
    }
}

fn first_text(f: &Feature, def: &FeatureTypeDef, path: &PropertyPath) -> Option<String> {
    let src = filter::FeatureSource {
        feature_type: def,
        feature: f,
    };
    use filter::PropertySource;
    src.resolve(path)
        .ok()?
        .vals
        .first()
        .and_then(|v| v.as_string())
}

fn geometry_bbox(f: &Feature, def: &FeatureTypeDef, path: &PropertyPath) -> Option<geo::Rect<f64>> {
    let (idx, _, _) = filter::resolve_property(def, path)?;
    match &f.values[idx] {
        Value::Geometry(g) => g.geometry.bounding_rect(),
        _ => None,
    }
}

pub async fn run_join(svc: &Arc<WfsService>, req: &GetFeatureRequest, query: &Query) -> OpResult {
    let version = req.version;
    let err = |e: WfsError| (version, e);
    let mut members = Vec::new();
    for (i, tn) in query.type_names.iter().enumerate() {
        let entry = svc
            .find_type(tn.ns.as_deref(), tn.prefix.as_deref(), &tn.local)
            .ok_or_else(|| {
                err(WfsError::invalid(
                    "typeName",
                    format!("Unknown feature type `{}`", tn.display()),
                ))
            })?;
        members.push(Member {
            entry,
            alias: query.aliases.get(i).cloned(),
        });
    }
    // self join without aliases: generated aliases; within a predicate, the n-th reference
    // to the repeated type name denotes its n-th occurrence (like GeoServer)
    let self_join = query.aliases.is_empty()
        && members
            .iter()
            .enumerate()
            .any(|(i, m)| members[..i].iter().any(|o| std::ptr::eq(o.entry, m.entry)));
    if self_join {
        for (i, m) in members.iter_mut().enumerate() {
            m.alias = Some(format!("__t{}", i + 1));
        }
    }
    let defs: Vec<(&FeatureTypeDef, Option<&str>)> = members
        .iter()
        .map(|m| (m.entry.def.as_ref(), m.alias.as_deref()))
        .collect();
    // parse filter with the namespaces of the query
    let mut ctx = filter::ParseContext::new(version.filter());
    ctx.namespaces = svc.prefix_map();
    for (p, u) in &query.namespaces {
        ctx.namespaces.insert(p.clone(), u.clone());
    }
    ctx.default_swap_xy = crate::wfs::crs::is_geographic(members[0].entry.def.srid);
    let filter = match &query.filter {
        Some(FilterSource::Xml(xml)) => {
            Some(parse_filter_str(xml, &ctx).map_err(|e| err(WfsError::from_filter(e, "filter")))?)
        }
        Some(FilterSource::Cql(cql)) => Some(
            crate::wfs::cql::parse_cql(cql, &ctx)
                .map_err(|e| err(WfsError::from_filter(e, "cql_filter")))?,
        ),
        None => None,
    };
    // split conjuncts by referenced members
    let mut pushed: Vec<Vec<Filter>> = vec![Vec::new(); members.len()];
    let mut join_preds = Vec::new();
    let mut conjs = filter.map(conjuncts).unwrap_or_default();
    if self_join {
        conjs = conjs
            .into_iter()
            .map(|c| {
                let seen = std::cell::Cell::new(0usize);
                c.map_paths(&|p| {
                    if p.steps.len() < 2 {
                        return p.clone();
                    }
                    let matching: Vec<usize> = defs
                        .iter()
                        .enumerate()
                        .filter(|(_, (def, _))| filter::step_matches_type(&p.steps[0], def))
                        .map(|(i, _)| i)
                        .collect();
                    if matching.len() < 2 {
                        return p.clone();
                    }
                    let n = seen.get();
                    seen.set(n + 1);
                    let idx = matching[n % matching.len()];
                    let mut q = p.clone();
                    q.steps[0].ns = None;
                    q.steps[0].prefix = None;
                    q.steps[0].local = defs[idx].1.unwrap_or_default().to_string();
                    q
                })
            })
            .collect();
    }
    for c in conjs {
        let refs = referenced(&c, &defs).map_err(err)?;
        if refs.len() == 1 {
            let i = refs[0];
            let rel = c.map_paths(&|p| {
                join_member(p, &defs)
                    .map(|(_, r)| r)
                    .unwrap_or_else(|| p.clone())
            });
            pushed[i].push(rel);
        } else {
            join_preds.push(c);
        }
    }
    // fetch members (with pushed down filters)
    let mut rows: Vec<Vec<Feature>> = Vec::new();
    for (i, m) in members.iter().enumerate() {
        let f = match pushed[i].len() {
            0 => None,
            1 => Some(pushed[i][0].clone()),
            _ => Some(Filter::And(pushed[i].clone())),
        };
        let f = match f {
            Some(f) => Some(
                filter::prepare(&f, &[&m.entry.def])
                    .map_err(|e| err(WfsError::from_filter(e, "filter")))?,
            ),
            None => None,
        };
        let q = StoreQuery {
            filter: f,
            ..Default::default()
        };
        let features: Vec<Feature> = m
            .entry
            .store
            .query(q)
            .try_collect()
            .await
            .map_err(|e| err(WfsError::no_applicable(e.to_string())))?;
        rows.push(features);
    }
    let tuples = join_rows(&members, &defs, &rows, &join_preds).map_err(err)?;
    let matched = tuples.len() as u64;
    let limit = req.count.or(svc.cfg.count_default);
    let start = req.start_index as usize;
    let selected: Vec<&Vec<usize>> = tuples
        .iter()
        .skip(start)
        .take(limit.map(|l| l as usize).unwrap_or(usize::MAX))
        .collect();
    let returned = if req.result_type == ResultType::Hits {
        0
    } else {
        selected.len()
    };
    // CSV output of tuples
    let format = crate::wfs::getfeature::output_format(
        version,
        req.output_format.as_deref(),
        &req.format_options,
    )
    .map_err(err)?;
    if format == crate::wfs::formats::Format::Csv {
        let sep = crate::wfs::formats::csv_separator(&req.format_options)
            .map_err(|e| err(WfsError::invalid("format_options", e)))?;
        let crss: Vec<OutputCrs> = members
            .iter()
            .map(|m| {
                OutputCrs::new(
                    crate::wfs::crs::Crs::new(m.entry.def.srid, crate::wfs::crs::CrsNotation::Urn),
                    None,
                )
            })
            .collect();
        let prefixes: Vec<String> = members
            .iter()
            .map(|m| {
                m.alias
                    .clone()
                    .filter(|a| !a.starts_with("__t"))
                    .unwrap_or_else(|| m.entry.def.name.local.clone())
            })
            .collect();
        let mut out = String::new();
        let header: Vec<(&FeatureTypeDef, &str)> = members
            .iter()
            .zip(&prefixes)
            .map(|(m, p)| (m.entry.def.as_ref(), p.as_str()))
            .collect();
        crate::wfs::formats::csv_join_header(&mut out, &header, sep);
        if req.result_type == ResultType::Results {
            for tuple in &selected {
                let items: Vec<(&FeatureTypeDef, &OutputCrs, &Feature)> = tuple
                    .iter()
                    .enumerate()
                    .map(|(mi, &fi)| (members[mi].entry.def.as_ref(), &crss[mi], &rows[mi][fi]))
                    .collect();
                let id: Vec<&str> = items.iter().map(|(_, _, f)| f.id.as_str()).collect();
                crate::wfs::formats::csv_join_row(&mut out, &id.join("-"), &items, sep);
            }
        }
        let name = members
            .iter()
            .map(|m| m.entry.def.name.local.as_str())
            .collect::<Vec<_>>()
            .join("_");
        let mut resp = WfsResponse::xml(Body::Text(out)).with_type(format.content_type());
        resp.headers.push((
            "Content-Disposition".into(),
            format!("attachment; filename={name}.csv"),
        ));
        return Ok(resp);
    }
    // output
    let gml = GmlVersion::V32;
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let mut decls = String::new();
    let mut seen = Vec::new();
    for m in &members {
        let n = &m.entry.def.name;
        if !seen.contains(&n.ns) {
            seen.push(n.ns.clone());
            decls.push_str(&format!(
                r#" xmlns:{}="{}""#,
                n.prefix,
                crate::wfs::xml::escape(&n.ns)
            ));
        }
    }
    let mut out = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><wfs:FeatureCollection timeStamp="{timestamp}" numberMatched="{matched}" numberReturned="{returned}" xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"{decls} xsi:schemaLocation="http://www.opengis.net/wfs/2.0 http://schemas.opengis.net/wfs/2.0/wfs.xsd">"#
    );
    if req.result_type == ResultType::Results {
        let encoders: Vec<FeatureEncoder> = members
            .iter()
            .map(|m| FeatureEncoder {
                def: &m.entry.def,
                gml,
                crs: OutputCrs::new(
                    crate::wfs::crs::Crs::new(m.entry.def.srid, crate::wfs::crs::CrsNotation::Urn),
                    None,
                ),
                properties: None,
                feature_bounding: false,
            })
            .collect();
        // gml:ids must be unique in the document: repeated features get a tuple specific id
        let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (t, tuple) in selected.iter().enumerate() {
            out.push_str("<wfs:member><wfs:Tuple>");
            for (mi, &fi) in tuple.iter().enumerate() {
                let f = &rows[mi][fi];
                let mut feature = f.clone();
                if !used.insert(feature.id.clone()) {
                    feature.id = format!("{}.t{}", feature.id, t + start + 1);
                    used.insert(feature.id.clone());
                }
                out.push_str("<wfs:member>");
                encoders[mi].write_feature(&mut out, &feature);
                out.push_str("</wfs:member>");
            }
            out.push_str("</wfs:Tuple></wfs:member>");
        }
    }
    out.push_str("</wfs:FeatureCollection>");
    Ok(WfsResponse::xml(Body::Text(out)).with_type("application/gml+xml; version=3.2"))
}

/// Combine member rows into tuples satisfying all join predicates
fn join_rows(
    members: &[Member],
    defs: &[(&FeatureTypeDef, Option<&str>)],
    rows: &[Vec<Feature>],
    preds: &[Filter],
) -> WfsResult<Vec<Vec<usize>>> {
    let srid = members[0].entry.def.srid;
    let check = |tuple: &[usize]| -> WfsResult<bool> {
        let src = TupleSource {
            members: tuple
                .iter()
                .enumerate()
                .map(|(mi, &fi)| (defs[mi].0, &rows[mi][fi], defs[mi].1))
                .collect(),
        };
        for p in preds {
            if !evaluate(p, &src, &EvalContext { srid })
                .map_err(|e| WfsError::from_filter(e, "filter"))?
            {
                return Ok(false);
            }
        }
        Ok(true)
    };
    // tuples of the first two members by best available strategy
    let mut partial: Vec<Vec<usize>> = (0..rows[0].len()).map(|i| vec![i]).collect();
    for next in 1..members.len() {
        let mut candidates: Vec<Vec<usize>> = Vec::new();
        let strategy = preds.iter().find_map(|p| pair_strategy(p, defs, next));
        match strategy {
            Some(Strategy::Equal { left, right }) => {
                let (lm, lp) = left;
                let (_, rp) = right;
                let mut index: HashMap<String, Vec<usize>> = HashMap::new();
                for (j, f) in rows[next].iter().enumerate() {
                    if let Some(v) = first_text(f, defs[next].0, &rp) {
                        index.entry(v).or_default().push(j);
                    }
                }
                for t in &partial {
                    if let Some(v) = first_text(&rows[lm][t[lm]], defs[lm].0, &lp) {
                        if let Some(js) = index.get(&v) {
                            for &j in js {
                                let mut c = t.clone();
                                c.push(j);
                                candidates.push(c);
                            }
                        }
                    }
                }
            }
            Some(Strategy::Spatial { left, right, grow }) => {
                let (lm, lp) = left;
                let (_, rp) = right;
                let tree = RTree::bulk_load(
                    rows[next]
                        .iter()
                        .enumerate()
                        .filter_map(|(j, f)| {
                            geometry_bbox(f, defs[next].0, &rp).map(|r| Envelope {
                                rect: AABB::from_corners(
                                    [r.min().x - grow, r.min().y - grow],
                                    [r.max().x + grow, r.max().y + grow],
                                ),
                                index: j,
                            })
                        })
                        .collect(),
                );
                for t in &partial {
                    if let Some(r) = geometry_bbox(&rows[lm][t[lm]], defs[lm].0, &lp) {
                        let env =
                            AABB::from_corners([r.min().x, r.min().y], [r.max().x, r.max().y]);
                        for e in tree.locate_in_envelope_intersecting(env) {
                            let mut c = t.clone();
                            c.push(e.index);
                            candidates.push(c);
                        }
                    }
                }
            }
            None => {
                if partial.len().saturating_mul(rows[next].len()) > MAX_PAIRS {
                    return Err(WfsError::processing(
                        "filter",
                        "Join too large: add selective filters or a join condition",
                    ));
                }
                for t in &partial {
                    for j in 0..rows[next].len() {
                        let mut c = t.clone();
                        c.push(j);
                        candidates.push(c);
                    }
                }
            }
        }
        // keep candidates satisfying predicates which only reference members <= next
        let mut kept = Vec::new();
        for c in candidates {
            let applicable: Vec<&Filter> = preds
                .iter()
                .filter(|p| {
                    referenced(p, defs)
                        .map(|r| r.iter().all(|i| *i <= next))
                        .unwrap_or(false)
                })
                .collect();
            let src = TupleSource {
                members: c
                    .iter()
                    .enumerate()
                    .map(|(mi, &fi)| (defs[mi].0, &rows[mi][fi], defs[mi].1))
                    .collect(),
            };
            let mut ok = true;
            for p in applicable {
                if !evaluate(p, &src, &EvalContext { srid })
                    .map_err(|e| WfsError::from_filter(e, "filter"))?
                {
                    ok = false;
                    break;
                }
            }
            if ok {
                kept.push(c);
            }
        }
        partial = kept;
    }
    // final check with all predicates
    let mut result = Vec::new();
    for t in partial {
        if check(&t)? {
            result.push(t);
        }
    }
    Ok(result)
}

enum Strategy {
    /// left member/path equals right (member `next`) path
    Equal {
        left: (usize, PropertyPath),
        right: (usize, PropertyPath),
    },
    Spatial {
        left: (usize, PropertyPath),
        right: (usize, PropertyPath),
        grow: f64,
    },
}

/// Join strategy for a predicate connecting an earlier member with member `next`
fn pair_strategy(
    p: &Filter,
    defs: &[(&FeatureTypeDef, Option<&str>)],
    next: usize,
) -> Option<Strategy> {
    let orient = |a: (usize, PropertyPath),
                  b: (usize, PropertyPath)|
     -> Option<((usize, PropertyPath), (usize, PropertyPath))> {
        if b.0 == next && a.0 < next {
            Some((a, b))
        } else if a.0 == next && b.0 < next {
            Some((b, a))
        } else {
            None
        }
    };
    match p {
        Filter::Comparison {
            op: ComparisonOp::EqualTo,
            left,
            right,
            ..
        } => {
            let (l, r) = orient(member_path(left, defs)?, member_path(right, defs)?)?;
            Some(Strategy::Equal { left: l, right: r })
        }
        Filter::Spatial {
            op,
            property: Some(prop),
            operand: SpatialOperand::Expr(other),
            distance,
        } => {
            if matches!(op, SpatialOp::Disjoint | SpatialOp::Beyond) {
                return None;
            }
            let (l, r) = orient(member_path(prop, defs)?, member_path(other, defs)?)?;
            let grow = match (op, distance) {
                (SpatialOp::DWithin, Some((d, u))) => {
                    filter::distance_in_crs(*d, u, defs[0].0.srid)
                }
                _ => 0.0,
            };
            Some(Strategy::Spatial {
                left: l,
                right: r,
                grow,
            })
        }
        _ => None,
    }
}

#[allow(dead_code)]
fn values(f: &Feature, idx: usize) -> Vec<filter::Val> {
    value_to_vals(&f.values[idx])
}
