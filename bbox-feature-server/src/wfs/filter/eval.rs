//! Filter evaluation against features.

use super::temporal::{parse_datetime, relation, Interval};
use super::*;
use crate::wfs::crs;
use crate::wfs::model::*;
use chrono::{DateTime, Utc};
use geo::{BoundingRect, Distance, Euclidean, Intersects, Relate};

/// Value during evaluation
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Null,
    Str(String),
    Int(i64),
    Num(f64),
    Bool(bool),
    Time(DateTime<Utc>),
    Geom(Geometry<f64>),
}

impl Val {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Val::Int(i) => Some(*i as f64),
            Val::Num(n) => Some(*n),
            Val::Str(s) => s.trim().parse().ok(),
            Val::Bool(b) => Some(*b as i32 as f64),
            _ => None,
        }
    }
    pub fn as_string(&self) -> Option<String> {
        match self {
            Val::Null => None,
            Val::Str(s) => Some(s.clone()),
            Val::Int(i) => Some(i.to_string()),
            Val::Num(n) => Some(format_double(*n)),
            Val::Bool(b) => Some(b.to_string()),
            Val::Time(t) => Some(t.to_rfc3339()),
            Val::Geom(g) => Some(format!("{g:?}")),
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Val::Bool(b) => Some(*b),
            Val::Str(s) => match s.trim() {
                "true" | "1" => Some(true),
                "false" | "0" => Some(false),
                _ => None,
            },
            Val::Int(i) => Some(*i != 0),
            _ => None,
        }
    }
    pub fn as_time(&self) -> Option<DateTime<Utc>> {
        match self {
            Val::Time(t) => Some(*t),
            Val::Str(s) => parse_datetime(s),
            _ => None,
        }
    }
    pub fn as_geom(&self) -> Option<&Geometry<f64>> {
        match self {
            Val::Geom(g) => Some(g),
            _ => None,
        }
    }
}

/// Convert a stored value to evaluation values
pub fn value_to_vals(value: &Value) -> Vec<Val> {
    match value {
        Value::Null => vec![],
        Value::String(s) => vec![Val::Str(s.clone())],
        Value::Integer(i) => vec![Val::Int(*i)],
        Value::Double(d) => vec![Val::Num(*d)],
        Value::Decimal(s) => vec![s.parse().map(Val::Num).unwrap_or(Val::Str(s.clone()))],
        Value::Boolean(b) => vec![Val::Bool(*b)],
        Value::Date(s) | Value::DateTime(s) => {
            vec![parse_datetime(s)
                .map(Val::Time)
                .unwrap_or(Val::Str(s.clone()))]
        }
        Value::Time(s) => vec![Val::Str(s.clone())],
        Value::Geometry(g) => vec![Val::Geom(g.geometry.clone())],
        Value::Xml(_) => value.text_values().into_iter().map(Val::Str).collect(),
    }
}

/// Typed view of a value for comparisons
fn coerce(text: &str, value_type: &ValueType) -> Val {
    match value_type {
        ValueType::Integer => text
            .trim()
            .parse::<i64>()
            .map(Val::Int)
            .or_else(|_| text.trim().parse::<f64>().map(Val::Num))
            .unwrap_or(Val::Str(text.to_string())),
        ValueType::Double | ValueType::Decimal => text
            .trim()
            .parse::<f64>()
            .map(Val::Num)
            .unwrap_or(Val::Str(text.to_string())),
        ValueType::Boolean => match text.trim() {
            "true" | "1" => Val::Bool(true),
            "false" | "0" => Val::Bool(false),
            _ => Val::Str(text.to_string()),
        },
        ValueType::Date | ValueType::DateTime => parse_datetime(text)
            .map(Val::Time)
            .unwrap_or(Val::Str(text.to_string())),
        _ => Val::Str(text.to_string()),
    }
}

/// Resolved property values with their declared type
pub struct Resolved {
    pub vals: Vec<Val>,
    pub value_type: Option<ValueType>,
    /// Stored value is null
    pub is_null: bool,
    /// Value is emitted as xsi:nil
    pub is_nil: bool,
}

/// Access to property values for filter evaluation
pub trait PropertySource {
    fn resolve(&self, path: &PropertyPath) -> Result<Resolved, FilterError>;
    /// All geometry values (for BBOX without property)
    fn all_geometries(&self) -> Vec<Geometry<f64>>;
    fn feature_id(&self) -> Option<&str>;
}

/// A single feature with its type
pub struct FeatureSource<'a> {
    pub feature_type: &'a FeatureTypeDef,
    pub feature: &'a Feature,
}

pub(crate) fn step_matches_type(step: &PathStep, ft: &FeatureTypeDef) -> bool {
    step.local == ft.name.local
        && match (&step.ns, &step.prefix) {
            (Some(ns), _) => *ns == ft.name.ns,
            (None, Some(prefix)) => *prefix == ft.name.prefix,
            (None, None) => true,
        }
        || (step.schema_element && ft.supertypes.iter().any(|s| s.local == step.local))
}

fn step_matches_name(step: &PathStep, name: &QName) -> bool {
    if step.local != name.local {
        return false;
    }
    match (&step.ns, &step.prefix) {
        (Some(ns), _) => *ns == name.ns || (is_gml_ns(ns) && name.is_gml()),
        (None, Some(prefix)) => *prefix == name.prefix || (prefix == "gml" && name.is_gml()),
        (None, None) => true,
    }
}

/// Find property definition for the first step of a path (after an optional type step)
pub fn resolve_property<'a>(
    ft: &'a FeatureTypeDef,
    path: &PropertyPath,
) -> Option<(usize, &'a PropertyDef, usize)> {
    let mut first = 0;
    if path.steps.len() > 1 && step_matches_type(&path.steps[0], ft) {
        first = 1;
    }
    let step = path.steps.get(first)?;
    if step.attribute {
        return None;
    }
    ft.properties
        .iter()
        .enumerate()
        .find(|(_, p)| step_matches_name(step, &p.name))
        .or_else(|| {
            // column mapped to a standard GML property (e.g. `app:name` for gml:name)
            let in_type_ns = match (&step.ns, &step.prefix) {
                (Some(ns), _) => *ns == ft.name.ns,
                (None, Some(prefix)) => *prefix == ft.name.prefix,
                (None, None) => true,
            };
            ft.properties.iter().enumerate().find(|(_, p)| {
                in_type_ns
                    && p.name.is_gml()
                    && p.name.local == step.local
                    && p.column == step.local
            })
        })
        .map(|(i, p)| (i, p, first))
}

/// Whether a path refers to the feature identifier (`@gml:id`, `@fid`)
pub fn is_id_path(ft: &FeatureTypeDef, path: &PropertyPath) -> bool {
    let steps: &[PathStep] = if path.steps.len() > 1 && step_matches_type(&path.steps[0], ft) {
        &path.steps[1..]
    } else {
        &path.steps
    };
    steps.len() == 1 && steps[0].attribute && matches!(steps[0].local.as_str(), "id" | "fid")
}

impl PropertySource for FeatureSource<'_> {
    fn resolve(&self, path: &PropertyPath) -> Result<Resolved, FilterError> {
        let ft = self.feature_type;
        if is_id_path(ft, path) {
            return Ok(Resolved {
                vals: vec![Val::Str(self.feature.id.clone())],
                value_type: Some(ValueType::String),
                is_null: false,
                is_nil: false,
            });
        }
        let (idx, prop, first) = resolve_property(ft, path)
            .ok_or_else(|| FilterError::UnknownProperty(path.text.clone()))?;
        let computed;
        let value = match &self.feature.values[idx] {
            Value::Null if prop.name.is_gml() && prop.name.local == "boundedBy" => {
                computed = match self.feature.bbox() {
                    Some(r) => Value::Geometry(GeomValue::new(Geometry::Polygon(r.to_polygon()))),
                    None => Value::Null,
                };
                &computed
            }
            v => v,
        };
        let step = &path.steps[first];
        let rest = &path.steps[first + 1..];
        let is_null = value.is_null();
        let is_nil = is_null && prop.nillable && prop.min_occurs > 0;
        if rest.is_empty() && step.index.is_none() && step.predicate.is_none() {
            return Ok(Resolved {
                vals: value_to_vals(value),
                value_type: Some(prop.value_type.clone()),
                is_null,
                is_nil,
            });
        }
        // navigate into XML fragment (complex / multi-valued properties)
        let vals = match value {
            Value::Xml(xml) => navigate_xml(xml, step, rest)?,
            v if rest.is_empty() && step.index == Some(1) => value_to_vals(v),
            _ => vec![],
        };
        let value_type = if rest.is_empty() {
            Some(prop.value_type.clone())
        } else {
            None
        };
        Ok(Resolved {
            is_null: vals.is_empty(),
            vals,
            value_type,
            is_nil: false,
        })
    }

    fn all_geometries(&self) -> Vec<Geometry<f64>> {
        self.feature_type
            .properties
            .iter()
            .zip(&self.feature.values)
            .filter(|(p, _)| !(p.name.is_gml() && p.name.local == "boundedBy"))
            .filter_map(|(_, v)| match v {
                Value::Geometry(g) => Some(g.geometry.clone()),
                _ => None,
            })
            .collect()
    }

    fn feature_id(&self) -> Option<&str> {
        Some(&self.feature.id)
    }
}

/// Navigate XML fragment: `first` selects the property element(s), `rest` descends
fn navigate_xml(xml: &str, first: &PathStep, rest: &[PathStep]) -> Result<Vec<Val>, FilterError> {
    let wrapped = format!("<r>{xml}</r>");
    let doc = roxmltree::Document::parse(&wrapped)
        .map_err(|e| FilterError::Processing(format!("invalid stored XML: {e}")))?;
    let mut nodes: Vec<roxmltree::Node> = select_children(doc.root_element(), first);
    let mut values = Vec::new();
    for (i, step) in rest.iter().enumerate() {
        if step.attribute {
            for n in &nodes {
                for a in n.attributes() {
                    if a.name() == step.local {
                        values.push(Val::Str(a.value().to_string()));
                    }
                }
            }
            if i + 1 == rest.len() {
                return Ok(values);
            }
            return Ok(vec![]);
        }
        nodes = nodes
            .iter()
            .flat_map(|n| select_children(*n, step))
            .collect();
    }
    Ok(nodes
        .iter()
        .map(|n| {
            Val::Str(
                n.descendants()
                    .filter(|d| d.is_text())
                    .filter_map(|d| d.text())
                    .collect::<String>()
                    .trim()
                    .to_string(),
            )
        })
        .collect())
}

fn select_children<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    step: &PathStep,
) -> Vec<roxmltree::Node<'a, 'input>> {
    let matching: Vec<_> = node
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == step.local)
        .filter(|c| match &step.ns {
            Some(ns) => c.tag_name().namespace() == Some(ns.as_str()),
            None => true,
        })
        .filter(|c| match &step.predicate {
            Some((pred, value)) => {
                if pred.attribute {
                    c.attributes()
                        .any(|a| a.name() == pred.local && a.value() == value)
                } else {
                    c.children().any(|cc| {
                        cc.is_element()
                            && cc.tag_name().name() == pred.local
                            && cc.text().map(str::trim) == Some(value.as_str())
                    })
                }
            }
            None => true,
        })
        .collect();
    match step.index {
        Some(i) => matching.get(i - 1).copied().into_iter().collect(),
        None => matching,
    }
}

/// Tuple of features of a join query
pub struct TupleSource<'a> {
    /// (type, feature, alias)
    pub members: Vec<(&'a FeatureTypeDef, &'a Feature, Option<&'a str>)>,
}

/// Member index of a join path and the path relative to that member
pub fn join_member(
    path: &PropertyPath,
    members: &[(&FeatureTypeDef, Option<&str>)],
) -> Option<(usize, PropertyPath)> {
    let first = path.first();
    if path.steps.len() > 1 && first.prefix.is_none() && first.ns.is_none() {
        if let Some(i) = members
            .iter()
            .position(|(_, a)| *a == Some(first.local.as_str()))
        {
            return Some((
                i,
                PropertyPath {
                    text: path.text.clone(),
                    steps: path.steps[1..].to_vec(),
                },
            ));
        }
    }
    if path.steps.len() > 1 {
        if let Some(i) = members
            .iter()
            .position(|(ft, _)| step_matches_type(first, ft))
        {
            return Some((i, path.clone()));
        }
    }
    members
        .iter()
        .position(|(ft, _)| is_id_path(ft, path) || resolve_property(ft, path).is_some())
        .map(|i| (i, path.clone()))
}

impl PropertySource for TupleSource<'_> {
    fn resolve(&self, path: &PropertyPath) -> Result<Resolved, FilterError> {
        let defs: Vec<(&FeatureTypeDef, Option<&str>)> =
            self.members.iter().map(|(d, _, a)| (*d, *a)).collect();
        let (i, rel) = join_member(path, &defs)
            .ok_or_else(|| FilterError::UnknownProperty(path.text.clone()))?;
        let (ft, f, _) = self.members[i];
        FeatureSource {
            feature_type: ft,
            feature: f,
        }
        .resolve(&rel)
    }
    fn all_geometries(&self) -> Vec<Geometry<f64>> {
        self.members
            .iter()
            .flat_map(|(ft, f, _)| {
                FeatureSource {
                    feature_type: ft,
                    feature: f,
                }
                .all_geometries()
            })
            .collect()
    }
    fn feature_id(&self) -> Option<&str> {
        None
    }
}

/// Check property references and reproject geometry literals into the native CRS of the
/// feature type(s). Must be called before evaluating.
pub fn prepare(filter: &Filter, types: &[&FeatureTypeDef]) -> Result<Filter, FilterError> {
    let native = types.first().map(|ft| ft.srid).unwrap_or(4326);
    let check = |p: &PropertyPath| -> Result<(), FilterError> {
        let ok = types.iter().any(|ft| {
            is_id_path(ft, p) || resolve_property(ft, p).is_some() || {
                // join path with explicit type step but unknown property
                false
            }
        });
        if ok {
            Ok(())
        } else {
            Err(FilterError::UnknownProperty(p.text.clone()))
        }
    };
    for p in filter.properties() {
        check(p)?;
    }
    let mut f = filter.clone();
    reproject_literals(&mut f, native)?;
    validate_operands(&f, types)?;
    Ok(f)
}

fn reproject_literal(g: &mut GeometryLiteral, native: u16) -> Result<(), FilterError> {
    if let Some(srid) = g.srid {
        if srid != native {
            crs::transform(&mut g.geometry, srid, native)
                .map_err(|e| FilterError::Processing(e.to_string()))?;
            g.srid = Some(native);
        }
    }
    Ok(())
}

fn reproject_expr(e: &mut Expr, native: u16) -> Result<(), FilterError> {
    match e {
        Expr::GeometryLiteral(g) => reproject_literal(g, native),
        Expr::Function { args, .. } => args.iter_mut().try_for_each(|a| reproject_expr(a, native)),
        Expr::Arith { left, right, .. } => {
            reproject_expr(left, native)?;
            reproject_expr(right, native)
        }
        _ => Ok(()),
    }
}

fn reproject_literals(f: &mut Filter, native: u16) -> Result<(), FilterError> {
    match f {
        Filter::And(fs) | Filter::Or(fs) => fs
            .iter_mut()
            .try_for_each(|x| reproject_literals(x, native)),
        Filter::Not(x) => reproject_literals(x, native),
        Filter::Spatial { operand, .. } => match operand {
            SpatialOperand::Geometry(g) => reproject_literal(g, native),
            SpatialOperand::Expr(e) => reproject_expr(e, native),
        },
        Filter::Comparison { left, right, .. } => {
            reproject_expr(left, native)?;
            reproject_expr(right, native)
        }
        Filter::Function(e) => reproject_expr(e, native),
        _ => Ok(()),
    }
}

/// Reject operator/operand combinations that can not be evaluated
fn validate_operands(f: &Filter, types: &[&FeatureTypeDef]) -> Result<(), FilterError> {
    let prop_type = |e: &Expr| -> Option<ValueType> {
        if let Expr::Property(p) = e {
            types
                .iter()
                .find_map(|ft| resolve_property(ft, p))
                .filter(|(_, _, first)| p.steps.len() == first + 1)
                .map(|(_, prop, _)| prop.value_type.clone())
        } else {
            None
        }
    };
    match f {
        Filter::And(fs) | Filter::Or(fs) => fs.iter().try_for_each(|x| validate_operands(x, types)),
        Filter::Not(x) => validate_operands(x, types),
        Filter::Comparison {
            left, right, op, ..
        } => {
            let geom_operand = matches!(left, Expr::GeometryLiteral(_))
                || matches!(right, Expr::GeometryLiteral(_));
            let geom_prop = prop_type(left).map(|t| t.is_geometry()).unwrap_or(false)
                || prop_type(right).map(|t| t.is_geometry()).unwrap_or(false);
            if (geom_operand || geom_prop)
                && *op != ComparisonOp::EqualTo
                && *op != ComparisonOp::NotEqualTo
            {
                return Err(FilterError::Processing(
                    "comparison operators can not be applied to geometries".into(),
                ));
            }
            Ok(())
        }
        Filter::Spatial {
            property: Some(p), ..
        } => {
            if let Expr::Property(path) = p {
                if let Some(t) = prop_type(p) {
                    if !t.is_geometry() {
                        return Err(FilterError::UnknownProperty(format!(
                            "{} is not a geometry property",
                            path.text
                        )));
                    }
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Evaluation context
#[derive(Clone, Debug)]
pub struct EvalContext {
    /// Native CRS of geometries (EPSG)
    pub srid: u16,
}

/// Evaluate filter for a property source
pub fn evaluate(
    filter: &Filter,
    src: &dyn PropertySource,
    ctx: &EvalContext,
) -> Result<bool, FilterError> {
    Ok(match filter {
        Filter::Constant(b) => *b,
        Filter::And(fs) => {
            for f in fs {
                if !evaluate(f, src, ctx)? {
                    return Ok(false);
                }
            }
            true
        }
        Filter::Or(fs) => {
            for f in fs {
                if evaluate(f, src, ctx)? {
                    return Ok(true);
                }
            }
            false
        }
        Filter::Not(f) => !evaluate(f, src, ctx)?,
        Filter::ResourceIds(ids) => match src.feature_id() {
            Some(id) => ids.iter().any(|rid| rid_matches(rid, id)),
            None => false,
        },
        Filter::Comparison {
            op,
            left,
            right,
            match_case,
            match_action,
        } => {
            let (lv, lt) = eval_expr_typed(left, src, ctx)?;
            let (rv, rt) = eval_expr_typed(right, src, ctx)?;
            let value_type = lt.or(rt);
            compare_sets(
                &lv,
                &rv,
                *op,
                *match_case,
                *match_action,
                value_type.as_ref(),
            )
        }
        Filter::Like {
            expr,
            pattern,
            wild_card,
            single_char,
            escape_char,
            match_case,
        } => {
            let vals = eval_expr(expr, src, ctx)?;
            vals.iter().filter_map(Val::as_string).any(|v| {
                like_match(
                    &v,
                    pattern,
                    *wild_card,
                    *single_char,
                    *escape_char,
                    *match_case,
                )
            })
        }
        Filter::IsNull(expr) => match expr {
            Expr::Property(p) => {
                let r = src.resolve(p)?;
                r.is_null && !r.is_nil
            }
            e => eval_expr(e, src, ctx)?.is_empty(),
        },
        Filter::IsNil { expr, .. } => match expr {
            Expr::Property(p) => src.resolve(p)?.is_nil,
            _ => false,
        },
        Filter::Between { expr, lower, upper } => {
            let (vals, vt) = eval_expr_typed(expr, src, ctx)?;
            let lo = eval_expr(lower, src, ctx)?;
            let hi = eval_expr(upper, src, ctx)?;
            vals.iter().any(|v| {
                lo.iter().any(|l| {
                    cmp_vals(v, l, true, vt.as_ref())
                        .map(|o| o != std::cmp::Ordering::Less)
                        .unwrap_or(false)
                }) && hi.iter().any(|h| {
                    cmp_vals(v, h, true, vt.as_ref())
                        .map(|o| o != std::cmp::Ordering::Greater)
                        .unwrap_or(false)
                })
            })
        }
        Filter::Spatial {
            op,
            property,
            operand,
            distance,
        } => {
            let geoms: Vec<Geometry<f64>> = match property {
                Some(e) => eval_expr(e, src, ctx)?
                    .into_iter()
                    .filter_map(|v| match v {
                        Val::Geom(g) => Some(g),
                        _ => None,
                    })
                    .collect(),
                None => src.all_geometries(),
            };
            let others: Vec<Geometry<f64>> = match operand {
                SpatialOperand::Geometry(g) => vec![g.geometry.clone()],
                SpatialOperand::Expr(e) => eval_expr(e, src, ctx)?
                    .into_iter()
                    .filter_map(|v| match v {
                        Val::Geom(g) => Some(g),
                        _ => None,
                    })
                    .collect(),
            };
            let dist = distance
                .as_ref()
                .map(|(d, units)| distance_in_crs(*d, units, ctx.srid));
            geoms
                .iter()
                .any(|g| others.iter().any(|o| spatial_relation(*op, g, o, dist)))
        }
        Filter::Temporal { op, expr, operand } => {
            let vals = eval_expr(expr, src, ctx)?;
            let others: Vec<Interval> = match operand {
                TemporalOperand::Literal(l) => vec![literal_interval(l)?],
                TemporalOperand::Expr(e) => eval_expr(e, src, ctx)?
                    .iter()
                    .filter_map(Val::as_time)
                    .map(Interval::instant)
                    .collect(),
            };
            vals.iter()
                .filter_map(Val::as_time)
                .map(Interval::instant)
                .any(|a| others.iter().any(|b| relation(*op, &a, b)))
        }
        Filter::Function(e) => eval_expr(e, src, ctx)?
            .first()
            .and_then(Val::as_bool)
            .unwrap_or(false),
    })
}

fn rid_matches(rid: &ResourceId, id: &str) -> bool {
    if rid.rid != id {
        return false;
    }
    // Single current version per feature
    match &rid.version {
        None | Some(VersionSpec::First) | Some(VersionSpec::Last) | Some(VersionSpec::All) => true,
        Some(VersionSpec::Index(n)) => *n == 1,
        Some(VersionSpec::Previous) | Some(VersionSpec::Next) => false,
        Some(VersionSpec::Timestamp(_)) => true,
    }
}

fn literal_interval(l: &TemporalLiteral) -> Result<Interval, FilterError> {
    let parse = |s: &str| {
        parse_datetime(s).ok_or_else(|| FilterError::Parse(format!("invalid time `{s}`")))
    };
    Ok(match l {
        TemporalLiteral::Instant(t) => Interval::instant(parse(t)?),
        TemporalLiteral::Period { begin, end } => Interval {
            begin: parse(begin)?,
            end: parse(end)?,
        },
    })
}

/// Convert a distance with units into CRS units
pub fn distance_in_crs(d: f64, units: &str, srid: u16) -> f64 {
    let u = units
        .trim_start_matches('#')
        .rsplit([':', '/', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let meters = match u.as_str() {
        "km" | "kilometre" | "kilometer" | "kilometers" | "kilometres" => Some(d * 1000.0),
        "m" | "metre" | "meter" | "meters" | "metres" | "9001" => Some(d),
        "mi" | "mile" | "miles" => Some(d * 1609.344),
        "ft" | "foot" | "feet" => Some(d * 0.3048),
        "nm" | "nautical_mile" => Some(d * 1852.0),
        _ => None, // degrees or CRS units
    };
    let geographic = crs::is_geographic(srid);
    match (meters, geographic) {
        (Some(m), true) => m / 111_319.490_793_273_57,
        (Some(m), false) => m,
        (None, _) => d,
    }
}

fn spatial_relation(
    op: SpatialOp,
    a: &Geometry<f64>,
    b: &Geometry<f64>,
    dist: Option<f64>,
) -> bool {
    match op {
        SpatialOp::BBox => a.intersects(b),
        SpatialOp::Intersects => a.intersects(b),
        SpatialOp::Disjoint => !a.intersects(b),
        SpatialOp::DWithin => Euclidean.distance(a, b) <= dist.unwrap_or(0.0),
        SpatialOp::Beyond => Euclidean.distance(a, b) > dist.unwrap_or(0.0),
        _ => {
            // envelope pre-check for predicates implying intersection
            if let (Some(ra), Some(rb)) = (a.bounding_rect(), b.bounding_rect()) {
                if !ra.intersects(&rb) {
                    return false;
                }
            }
            let m = a.relate(b);
            match op {
                SpatialOp::Equals => m.is_equal_topo(),
                SpatialOp::Touches => m.is_touches(),
                SpatialOp::Within => m.is_within(),
                SpatialOp::Overlaps => m.is_overlaps(),
                SpatialOp::Crosses => m.is_crosses(),
                SpatialOp::Contains => m.is_contains(),
                _ => unreachable!(),
            }
        }
    }
}

/// Evaluate expression to values
pub fn eval_expr(
    expr: &Expr,
    src: &dyn PropertySource,
    ctx: &EvalContext,
) -> Result<Vec<Val>, FilterError> {
    eval_expr_typed(expr, src, ctx).map(|(v, _)| v)
}

fn eval_expr_typed(
    expr: &Expr,
    src: &dyn PropertySource,
    ctx: &EvalContext,
) -> Result<(Vec<Val>, Option<ValueType>), FilterError> {
    Ok(match expr {
        Expr::Property(p) => {
            let r = src.resolve(p)?;
            (r.vals, r.value_type)
        }
        Expr::Literal(s) => (vec![Val::Str(s.clone())], None),
        Expr::GeometryLiteral(g) => (vec![Val::Geom(g.geometry.clone())], None),
        Expr::Function { name, args } => {
            let mut arg_vals = Vec::new();
            for a in args {
                arg_vals.push(
                    eval_expr(a, src, ctx)?
                        .into_iter()
                        .next()
                        .unwrap_or(Val::Null),
                );
            }
            (vec![super::functions::call(name, &arg_vals)?], None)
        }
        Expr::Arith { op, left, right } => {
            let l = eval_expr(left, src, ctx)?;
            let r = eval_expr(right, src, ctx)?;
            let (Some(a), Some(b)) = (
                l.first().and_then(Val::as_f64),
                r.first().and_then(Val::as_f64),
            ) else {
                return Ok((vec![], None));
            };
            let v = match op {
                ArithOp::Add => a + b,
                ArithOp::Sub => a - b,
                ArithOp::Mul => a * b,
                ArithOp::Div => a / b,
            };
            (vec![Val::Num(v)], Some(ValueType::Double))
        }
    })
}

/// Compare two values; literals are coerced to the property type
fn cmp_vals(
    a: &Val,
    b: &Val,
    match_case: bool,
    value_type: Option<&ValueType>,
) -> Option<std::cmp::Ordering> {
    let coerce_val = |v: &Val| -> Val {
        match (v, value_type) {
            (Val::Str(s), Some(t)) => coerce(s, t),
            _ => v.clone(),
        }
    };
    let (a, b) = (coerce_val(a), coerce_val(b));
    match (&a, &b) {
        (Val::Int(x), Val::Int(y)) => Some(x.cmp(y)),
        (Val::Time(x), Val::Time(y)) => Some(x.cmp(y)),
        (Val::Bool(x), Val::Bool(y)) => Some(x.cmp(y)),
        (Val::Geom(x), Val::Geom(y)) => {
            if x.relate(y).is_equal_topo() {
                Some(std::cmp::Ordering::Equal)
            } else {
                None
            }
        }
        (Val::Int(_) | Val::Num(_), Val::Int(_) | Val::Num(_)) => {
            a.as_f64()?.partial_cmp(&b.as_f64()?)
        }
        (Val::Int(_) | Val::Num(_), Val::Str(_)) | (Val::Str(_), Val::Int(_) | Val::Num(_)) => {
            match (a.as_f64(), b.as_f64()) {
                (Some(x), Some(y)) => x.partial_cmp(&y),
                _ => cmp_strings(&a.as_string()?, &b.as_string()?, match_case),
            }
        }
        (Val::Time(_), Val::Str(_)) | (Val::Str(_), Val::Time(_)) => {
            match (a.as_time(), b.as_time()) {
                (Some(x), Some(y)) => Some(x.cmp(&y)),
                _ => None,
            }
        }
        (Val::Str(x), Val::Str(y)) => {
            if value_type.is_none() {
                // untyped: numeric comparison if both are numbers
                if let (Ok(nx), Ok(ny)) = (x.trim().parse::<f64>(), y.trim().parse::<f64>()) {
                    return nx.partial_cmp(&ny);
                }
            }
            cmp_strings(x, y, match_case)
        }
        _ => cmp_strings(&a.as_string()?, &b.as_string()?, match_case),
    }
}

fn cmp_strings(a: &str, b: &str, match_case: bool) -> Option<std::cmp::Ordering> {
    if match_case {
        Some(a.cmp(b))
    } else {
        Some(a.to_lowercase().cmp(&b.to_lowercase()))
    }
}

fn op_holds(op: ComparisonOp, ord: std::cmp::Ordering) -> bool {
    use std::cmp::Ordering::*;
    match op {
        ComparisonOp::EqualTo => ord == Equal,
        ComparisonOp::NotEqualTo => ord != Equal,
        ComparisonOp::LessThan => ord == Less,
        ComparisonOp::GreaterThan => ord == Greater,
        ComparisonOp::LessThanOrEqualTo => ord != Greater,
        ComparisonOp::GreaterThanOrEqualTo => ord != Less,
    }
}

fn compare_sets(
    left: &[Val],
    right: &[Val],
    op: ComparisonOp,
    match_case: bool,
    action: MatchAction,
    value_type: Option<&ValueType>,
) -> bool {
    let results: Vec<bool> = left
        .iter()
        .flat_map(|l| {
            right.iter().map(move |r| {
                cmp_vals(l, r, match_case, value_type)
                    .map(|o| op_holds(op, o))
                    .unwrap_or(op == ComparisonOp::NotEqualTo)
            })
        })
        .collect();
    if results.is_empty() {
        return false;
    }
    match action {
        MatchAction::Any => results.iter().any(|b| *b),
        MatchAction::All => results.iter().all(|b| *b),
        MatchAction::One => results.iter().filter(|b| **b).count() == 1,
    }
}

/// Match value against a PropertyIsLike pattern (whole value)
pub fn like_match(
    value: &str,
    pattern: &str,
    wild: char,
    single: char,
    escape: char,
    match_case: bool,
) -> bool {
    #[derive(Debug, PartialEq)]
    enum Tok {
        Char(char),
        Any,
        One,
    }
    let mut toks = Vec::new();
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c == escape {
            if let Some(n) = chars.next() {
                toks.push(Tok::Char(n));
            }
        } else if c == wild {
            toks.push(Tok::Any);
        } else if c == single {
            toks.push(Tok::One);
        } else {
            toks.push(Tok::Char(c));
        }
    }
    let norm = |c: char| -> char {
        if match_case {
            c
        } else {
            c.to_lowercase().next().unwrap_or(c)
        }
    };
    let text: Vec<char> = value.chars().map(norm).collect();
    // dynamic programming match
    let (n, m) = (text.len(), toks.len());
    let mut dp = vec![vec![false; m + 1]; n + 1];
    dp[0][0] = true;
    for j in 1..=m {
        dp[0][j] = dp[0][j - 1] && toks[j - 1] == Tok::Any;
    }
    for i in 1..=n {
        for j in 1..=m {
            dp[i][j] = match &toks[j - 1] {
                Tok::Any => dp[i - 1][j] || dp[i][j - 1],
                Tok::One => dp[i - 1][j - 1],
                Tok::Char(c) => dp[i - 1][j - 1] && norm(*c) == text[i - 1],
            };
        }
    }
    dp[n][m]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wfs::xsd::read_feature_types;
    use geo::point;

    fn other_type() -> FeatureTypeDef {
        let xsd = std::fs::read_to_string(format!(
            "{}/tests/data/cite/wfs10/dataFeatures.xsd",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let mut ft = read_feature_types(&xsd, "cdf")
            .unwrap()
            .into_iter()
            .find(|ft| ft.name.local == "Other")
            .unwrap();
        ft.srid = 32615;
        ft
    }

    fn other_feature(ft: &FeatureTypeDef) -> Feature {
        let mut values = vec![Value::Null; ft.properties.len()];
        let set = |values: &mut Vec<Value>, name: &str, v: Value| {
            let (i, _) = ft.property_by_local(name).unwrap();
            values[i] = v;
        };
        set(&mut values, "name", Value::String("singleFeature".into()));
        set(
            &mut values,
            "pointProperty",
            Value::Geometry(GeomValue::new(point!(x: 500050., y: 500050.).into())),
        );
        set(&mut values, "string1", Value::String("always".into()));
        set(&mut values, "string2", Value::String("sometimes".into()));
        set(&mut values, "integers", Value::Integer(7));
        set(&mut values, "dates", Value::Date("2002-12-02".into()));
        Feature {
            id: "Other.1".to_string(),
            values,
        }
    }

    fn ctx() -> ParseContext {
        let mut ctx = ParseContext::new(FilterVersion::V100);
        ctx.namespaces
            .insert("cdf".into(), "http://www.opengis.net/cite/data".into());
        ctx
    }

    fn eval_on_other(filter_body: &str) -> Result<bool, FilterError> {
        let ft = other_type();
        let feature = other_feature(&ft);
        let xml = format!(
            r#"<ogc:Filter xmlns:ogc="http://www.opengis.net/ogc" xmlns:gml="http://www.opengis.net/gml" xmlns:cdf="http://www.opengis.net/cite/data">{filter_body}</ogc:Filter>"#
        );
        let f = parse_filter_str(&xml, &ctx())?;
        let f = prepare(&f, &[&ft])?;
        evaluate(
            &f,
            &FeatureSource {
                feature_type: &ft,
                feature: &feature,
            },
            &EvalContext { srid: ft.srid },
        )
    }

    fn cmp(op: &str, prop: &str, lit: &str) -> String {
        format!("<ogc:{op}><ogc:PropertyName>cdf:{prop}</ogc:PropertyName><ogc:Literal>{lit}</ogc:Literal></ogc:{op}>")
    }

    /// ets-wfs10 comparison operator tests 1-21 against cdf:Other
    #[test]
    fn wfs10_comparison_table() {
        let cases = [
            (cmp("PropertyIsGreaterThanOrEqualTo", "integers", "7"), true),
            ("<ogc:PropertyIsBetween><ogc:PropertyName>cdf:integers</ogc:PropertyName><ogc:LowerBoundary><ogc:Literal>7</ogc:Literal></ogc:LowerBoundary><ogc:UpperBoundary><ogc:Literal>7</ogc:Literal></ogc:UpperBoundary></ogc:PropertyIsBetween>".to_string(), true),
            ("<ogc:PropertyIsBetween><ogc:PropertyName>cdf:dates</ogc:PropertyName><ogc:LowerBoundary><ogc:Literal>2002-12-02</ogc:Literal></ogc:LowerBoundary><ogc:UpperBoundary><ogc:Literal>2002-12-02</ogc:Literal></ogc:UpperBoundary></ogc:PropertyIsBetween>".to_string(), true),
            (cmp("PropertyIsEqualTo", "string2", "sometimes"), true),
            (cmp("PropertyIsEqualTo", "integers", "7"), true),
            (cmp("PropertyIsEqualTo", "dates", "2002-12-02"), true),
            (cmp("PropertyIsGreaterThan", "integers", "6"), true),
            (cmp("PropertyIsGreaterThan", "dates", "2002-12-01"), true),
            (cmp("PropertyIsGreaterThanOrEqualTo", "dates", "2002-12-02"), true),
            (cmp("PropertyIsLessThan", "string2", "tometimes"), true),
            (cmp("PropertyIsLessThan", "integers", "8"), true),
            (cmp("PropertyIsLessThan", "dates", "2002-12-03"), true),
            (cmp("PropertyIsLessThanOrEqualTo", "string2", "sometimes"), true),
            (cmp("PropertyIsLessThanOrEqualTo", "integers", "7"), true),
            (cmp("PropertyIsLessThanOrEqualTo", "dates", "2002-12-02"), true),
            (r#"<ogc:PropertyIsLike wildCard="*" singleChar="." escape="\"><ogc:PropertyName>cdf:string2</ogc:PropertyName><ogc:Literal>s.met*s</ogc:Literal></ogc:PropertyIsLike>"#.to_string(), true),
            (cmp("PropertyIsNotEqualTo", "string2", "sometimes"), false),
            (cmp("PropertyIsNotEqualTo", "integers", "7"), false),
            (cmp("PropertyIsNotEqualTo", "dates", "2002-12-02"), false),
            // numeric comparison, not lexical
            (cmp("PropertyIsLessThan", "integers", "10"), true),
            // null checks
            ("<ogc:PropertyIsNull><ogc:PropertyName>gml:description</ogc:PropertyName></ogc:PropertyIsNull>".to_string(), true),
            ("<ogc:PropertyIsNull><ogc:PropertyName>gml:name</ogc:PropertyName></ogc:PropertyIsNull>".to_string(), false),
            ("<ogc:Not><ogc:PropertyIsNull><ogc:PropertyName>cdf:integers</ogc:PropertyName></ogc:PropertyIsNull></ogc:Not>".to_string(), true),
            (r#"<ogc:FeatureId fid="Other.1"/>"#.to_string(), true),
            (r#"<ogc:FeatureId fid="NONEXISTING"/>"#.to_string(), false),
            ("<ogc:PropertyIsGreaterThan><ogc:Add><ogc:PropertyName>cdf:integers</ogc:PropertyName><ogc:Literal>1</ogc:Literal></ogc:Add><ogc:Literal>7.5</ogc:Literal></ogc:PropertyIsGreaterThan>".to_string(), true),
            (r#"<ogc:PropertyIsEqualTo><ogc:Function name="strToUpperCase"><ogc:PropertyName>cdf:string1</ogc:PropertyName></ogc:Function><ogc:Literal>ALWAYS</ogc:Literal></ogc:PropertyIsEqualTo>"#.to_string(), true),
        ];
        for (i, (body, expected)) in cases.iter().enumerate() {
            assert_eq!(eval_on_other(body).unwrap(), *expected, "case {i}: {body}");
        }
    }

    #[test]
    fn unknown_property_is_rejected() {
        assert_eq!(
            eval_on_other(&cmp("PropertyIsEqualTo", "undefined", "1")),
            Err(FilterError::UnknownProperty("cdf:undefined".into()))
        );
        assert!(matches!(
            eval_on_other(
                r#"<ogc:PropertyIsLessThanOrEqualTo><ogc:PropertyName>gml:boundedBy</ogc:PropertyName><ogc:Literal><gml:Envelope srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>-90 -180</gml:lowerCorner><gml:upperCorner>90 180</gml:upperCorner></gml:Envelope></ogc:Literal></ogc:PropertyIsLessThanOrEqualTo>"#
            ),
            Err(FilterError::Processing(_))
        ));
        assert!(matches!(
            eval_on_other(
                r#"<ogc:BBOX><ogc:PropertyName>gml:description</ogc:PropertyName><gml:Box srsName="EPSG:32615"><gml:coordinates>0,0 1,1</gml:coordinates></gml:Box></ogc:BBOX>"#
            ),
            Err(FilterError::UnknownProperty(_))
        ));
    }

    #[test]
    fn like_patterns() {
        assert!(like_match("sometimes", "s.met*s", '*', '.', '\\', true));
        assert!(like_match("Main St", "*in St", '*', '?', '\\', true));
        assert!(!like_match("Main Rd", "*in St", '*', '?', '\\', true));
        assert!(like_match("a*b", "a\\*b", '*', '?', '\\', true));
        assert!(!like_match("axb", "a\\*b", '*', '?', '\\', true));
        assert!(like_match("ABC", "a?c", '*', '?', '\\', false));
        assert!(!like_match("ABC", "a?c", '*', '?', '\\', true));
        assert!(like_match("", "*", '*', '?', '\\', true));
    }

    #[test]
    fn spatial_reprojected_literal() {
        // feature in EPSG:32615, literal in lat/lon urn EPSG:4326
        assert!(eval_on_other(r#"<ogc:Intersects><ogc:PropertyName>gml:pointProperty</ogc:PropertyName><gml:Polygon srsName="EPSG:32615"><gml:outerBoundaryIs><gml:LinearRing><gml:coordinates>500000,500000 500100,500000 500100,500100 500000,500100 500000,500000</gml:coordinates></gml:LinearRing></gml:outerBoundaryIs></gml:Polygon></ogc:Intersects>"#).unwrap());
        // 500050,500050 in UTM 15N is about lat 4.5223, lon -92.9996
        assert!(eval_on_other(r#"<ogc:BBOX><ogc:PropertyName>gml:pointProperty</ogc:PropertyName><gml:Envelope xmlns:gml="http://www.opengis.net/gml" srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>4.5 -93.01</gml:lowerCorner><gml:upperCorner>4.6 -92.99</gml:upperCorner></gml:Envelope></ogc:BBOX>"#).unwrap());
        assert!(!eval_on_other(r#"<ogc:BBOX><ogc:PropertyName>gml:pointProperty</ogc:PropertyName><gml:Envelope xmlns:gml="http://www.opengis.net/gml" srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>10.5 -93.01</gml:lowerCorner><gml:upperCorner>10.6 -92.99</gml:upperCorner></gml:Envelope></ogc:BBOX>"#).unwrap());
        // lon/lat order in a lat/lon CRS gives an invalid latitude: processing error, not a silent miss
        assert!(matches!(
            eval_on_other(
                r#"<ogc:BBOX><ogc:PropertyName>gml:pointProperty</ogc:PropertyName><gml:Envelope xmlns:gml="http://www.opengis.net/gml" srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>-93.01 4.5</gml:lowerCorner><gml:upperCorner>-92.99 4.6</gml:upperCorner></gml:Envelope></ogc:BBOX>"#
            ),
            Err(FilterError::Processing(_))
        ));
    }

    #[test]
    fn metric_distance_on_geographic_crs() {
        assert!((distance_in_crs(111_319.490_793_273_57, "m", 4326) - 1.0).abs() < 1e-9);
        assert_eq!(distance_in_crs(10.0, "#metre", 32615), 10.0);
        assert_eq!(distance_in_crs(2.0, "km", 32615), 2000.0);
        assert_eq!(distance_in_crs(0.5, "deg", 4326), 0.5);
    }

    /// All 138 spatial operator tests of ets-wfs10 (section 11 of the test spec)
    #[test]
    fn wfs10_spatial_operator_table() {
        let dir = format!("{}/tests/data/cite/wfs10", env!("CARGO_MANIFEST_DIR"));
        let xsd = std::fs::read_to_string(format!("{dir}/geometryFeatures.xsd")).unwrap();
        let types: Vec<FeatureTypeDef> = read_feature_types(&xsd, "cgf")
            .unwrap()
            .into_iter()
            .map(|mut ft| {
                ft.srid = 32615;
                ft
            })
            .collect();
        let geom = |xml: &str| -> Geometry<f64> {
            let wrapped = format!(r#"<r xmlns:gml="http://www.opengis.net/gml">{xml}</r>"#);
            let doc = roxmltree::Document::parse(&wrapped).unwrap();
            crate::wfs::gml::parse_geometry(doc.root_element().first_element_child().unwrap())
                .unwrap()
                .geometry
        };
        let data = [
            ("Points", "t0000", "<gml:Point><gml:coordinates>500050,500050</gml:coordinates></gml:Point>"),
            ("Lines", "t0001", "<gml:LineString><gml:coordinates>500125,500025 500175,500075</gml:coordinates></gml:LineString>"),
            ("Polygons", "t0002", "<gml:Polygon><gml:outerBoundaryIs><gml:LinearRing><gml:coordinates>500225,500025 500225,500075 500275,500050 500275,500025 500225,500025</gml:coordinates></gml:LinearRing></gml:outerBoundaryIs></gml:Polygon>"),
            ("MPoints", "t0003", "<gml:MultiPoint><gml:pointMember><gml:Point><gml:coordinates>500325,500025</gml:coordinates></gml:Point></gml:pointMember><gml:pointMember><gml:Point><gml:coordinates>500375,500075</gml:coordinates></gml:Point></gml:pointMember></gml:MultiPoint>"),
            ("MLines", "t0004", "<gml:MultiLineString><gml:lineStringMember><gml:LineString><gml:coordinates>500425,500025 500475,500075</gml:coordinates></gml:LineString></gml:lineStringMember><gml:lineStringMember><gml:LineString><gml:coordinates>500425,500075 500475,500025</gml:coordinates></gml:LineString></gml:lineStringMember></gml:MultiLineString>"),
            ("MPolygons", "t0005", "<gml:MultiPolygon><gml:polygonMember><gml:Polygon><gml:outerBoundaryIs><gml:LinearRing><gml:coordinates>500525,500025 500550,500050 500575,500025 500525,500025</gml:coordinates></gml:LinearRing></gml:outerBoundaryIs></gml:Polygon></gml:polygonMember><gml:polygonMember><gml:Polygon><gml:outerBoundaryIs><gml:LinearRing><gml:coordinates>500525,500050 500525,500075 500550,500075 500550,500050 500525,500050</gml:coordinates></gml:LinearRing></gml:outerBoundaryIs></gml:Polygon></gml:polygonMember></gml:MultiPolygon>"),
        ];
        let features: Vec<(FeatureTypeDef, Feature)> = data
            .iter()
            .map(|(name, id, g)| {
                let ft = types
                    .iter()
                    .find(|ft| ft.name.local == *name)
                    .unwrap()
                    .clone();
                let mut values = vec![Value::Null; ft.properties.len()];
                let (i, _) = ft.property_by_local("id").unwrap();
                values[i] = Value::String(id.to_string());
                let (i, _) = ft.default_geometry().unwrap();
                values[i] = Value::Geometry(GeomValue::new(geom(g)));
                let f = Feature {
                    id: format!("{name}.1"),
                    values,
                };
                (ft, f)
            })
            .collect();
        let cases = std::fs::read_to_string(format!("{dir}/spatial-cases.tsv")).unwrap();
        let mut ctx = ParseContext::new(FilterVersion::V100);
        ctx.namespaces
            .insert("cgf".into(), "http://www.opengis.net/cite/geometry".into());
        let mut failures = Vec::new();
        let mut count = 0;
        for line in cases.lines().filter(|l| !l.starts_with('#')) {
            let cols: Vec<&str> = line.split('\t').collect();
            let (test, type_name, filter, expected) =
                (cols[0], cols[1], cols[3], cols[5] == "true");
            let local = type_name.trim_start_matches("cgf:");
            let (ft, feature) = features
                .iter()
                .find(|(ft, _)| ft.name.local == local)
                .unwrap();
            let xml = format!(
                r#"<ogc:Filter xmlns:ogc="http://www.opengis.net/ogc" xmlns:gml="http://www.opengis.net/gml">{filter}</ogc:Filter>"#
            );
            let result = parse_filter_str(&xml, &ctx)
                .and_then(|f| prepare(&f, &[ft]))
                .and_then(|f| {
                    evaluate(
                        &f,
                        &FeatureSource {
                            feature_type: ft,
                            feature,
                        },
                        &EvalContext { srid: 32615 },
                    )
                });
            count += 1;
            match result {
                Ok(r) if r == expected => {}
                other => failures.push(format!("{test}: expected {expected}, got {other:?}")),
            }
        }
        assert_eq!(count, 138);
        assert!(
            failures.is_empty(),
            "{} failures:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
