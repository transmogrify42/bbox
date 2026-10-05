//! Filter XML parser (Filter Encoding 1.0, 1.1 and FES 2.0).

use super::*;
use crate::wfs::crs::Crs;
use crate::wfs::gml;
use std::collections::HashMap;

pub const OGC_NS: &str = "http://www.opengis.net/ogc";
pub const FES_NS: &str = "http://www.opengis.net/fes/2.0";
/// Namespace of BBOX extension operators (FES 2.0 Extended_Capabilities)
pub const EXT_NS: &str = "https://www.bbox.earth/fes";

/// Context for parsing filters
#[derive(Clone, Debug)]
pub struct ParseContext {
    pub version: FilterVersion,
    /// Additional prefix bindings (KVP NAMESPACES, server prefixes); in-scope XML declarations win
    pub namespaces: HashMap<String, String>,
    /// CRS assumed for geometry literals without srsName (EPSG code)
    pub default_srid: u16,
    /// Axis order of geometry literals without srsName is y/x
    pub default_swap_xy: bool,
    /// CRS of geometry literals without srsName (e.g. the query srsName); None: native CRS
    pub literal_srid: Option<u16>,
}

impl ParseContext {
    pub fn new(version: FilterVersion) -> Self {
        ParseContext {
            version,
            namespaces: HashMap::new(),
            default_srid: 4326,
            default_swap_xy: false,
            literal_srid: None,
        }
    }
}

/// Parse a filter given as XML text (e.g. KVP FILTER parameter)
pub fn parse_filter_str(xml: &str, ctx: &ParseContext) -> Result<Filter, FilterError> {
    let doc = roxmltree::Document::parse(xml.trim())
        .map_err(|e| FilterError::Parse(format!("filter is not well-formed XML: {e}")))?;
    parse_filter(doc.root_element(), ctx)
}

/// Parse an ogc:Filter / fes:Filter element
pub fn parse_filter(node: roxmltree::Node, ctx: &ParseContext) -> Result<Filter, FilterError> {
    if !is_filter_ns(&node) || node.tag_name().name() != "Filter" {
        return Err(FilterError::Parse(format!(
            "expected Filter element, found `{}`",
            node.tag_name().name()
        )));
    }
    let children: Vec<_> = elements(node).collect();
    if children.is_empty() {
        return Err(FilterError::Parse("empty filter".into()));
    }
    // identifier filters may consist of multiple id elements
    if children.iter().all(|c| is_id_element(c)) {
        return Ok(Filter::ResourceIds(
            children.iter().map(resource_id).collect::<Result<_, _>>()?,
        ));
    }
    if children.len() > 1 {
        return Err(FilterError::Parse(
            "filter must contain a single predicate".into(),
        ));
    }
    Parser { ctx }.predicate(children[0])
}

/// Filter encoding element (OGC or FES namespace, or unqualified as accepted by GeoServer)
fn is_filter_ns(node: &roxmltree::Node) -> bool {
    matches!(
        node.tag_name().namespace(),
        Some(OGC_NS) | Some(FES_NS) | None
    )
}

fn elements<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
) -> impl Iterator<Item = roxmltree::Node<'a, 'input>> {
    node.children().filter(|c| c.is_element())
}

fn is_id_element(node: &roxmltree::Node) -> bool {
    is_filter_ns(node)
        && matches!(
            node.tag_name().name(),
            "FeatureId" | "GmlObjectId" | "ResourceId"
        )
}

fn resource_id(node: &roxmltree::Node) -> Result<ResourceId, FilterError> {
    let rid = match node.tag_name().name() {
        "FeatureId" => node.attribute("fid"),
        "GmlObjectId" => node
            .attribute((gml::GML_NS, "id"))
            .or_else(|| node.attribute((gml::GML32_NS, "id")))
            .or_else(|| node.attribute("id")),
        _ => node.attribute("rid"),
    }
    .ok_or_else(|| FilterError::Parse("identifier without id attribute".into()))?;
    let version = node.attribute("version").map(|v| match v {
        "FIRST" => VersionSpec::First,
        "LAST" => VersionSpec::Last,
        "PREVIOUS" => VersionSpec::Previous,
        "NEXT" => VersionSpec::Next,
        "ALL" => VersionSpec::All,
        v => v
            .parse::<u32>()
            .map(VersionSpec::Index)
            .unwrap_or_else(|_| VersionSpec::Timestamp(v.to_string())),
    });
    Ok(ResourceId {
        rid: rid.to_string(),
        version,
        start_date: node.attribute("startDate").map(str::to_string),
        end_date: node.attribute("endDate").map(str::to_string),
    })
}

struct Parser<'c> {
    ctx: &'c ParseContext,
}

impl Parser<'_> {
    fn err<T>(&self, msg: impl Into<String>) -> Result<T, FilterError> {
        Err(FilterError::Parse(msg.into()))
    }

    fn predicate(&self, node: roxmltree::Node) -> Result<Filter, FilterError> {
        if node.tag_name().namespace() == Some(EXT_NS) {
            return self.extension(node);
        }
        if !is_filter_ns(&node) {
            // extended operator: element in another namespace naming a boolean function
            let local = node.tag_name().name();
            if let Some(f) =
                super::functions::function_names().find(|f| f.eq_ignore_ascii_case(local))
            {
                let args = elements(node)
                    .map(|c| self.expr(c))
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok(Filter::Function(Expr::Function {
                    name: f.to_string(),
                    args,
                }));
            }
            return self.err(format!(
                "unexpected element `{}` in filter",
                node.tag_name().name()
            ));
        }
        let name = node.tag_name().name();
        match name {
            "And" | "Or" => {
                let operands = elements(node)
                    .map(|c| self.predicate(c))
                    .collect::<Result<Vec<_>, _>>()?;
                if operands.len() < 2 && self.ctx.version != FilterVersion::V100 {
                    return self.err(format!("{name} requires at least two operands"));
                }
                Ok(if name == "And" {
                    Filter::And(operands)
                } else {
                    Filter::Or(operands)
                })
            }
            "Not" => {
                let mut ops = elements(node);
                let inner = ops
                    .next()
                    .ok_or_else(|| FilterError::Parse("empty Not".into()))?;
                if ops.next().is_some() {
                    return self.err("Not requires exactly one operand");
                }
                Ok(Filter::Not(Box::new(self.predicate(inner)?)))
            }
            "PropertyIsEqualTo"
            | "PropertyIsNotEqualTo"
            | "PropertyIsLessThan"
            | "PropertyIsGreaterThan"
            | "PropertyIsLessThanOrEqualTo"
            | "PropertyIsGreaterThanOrEqualTo" => self.comparison(node, name),
            "PropertyIsLike" => self.like(node),
            "PropertyIsNull" => {
                let expr = self.single_expr(node)?;
                Ok(Filter::IsNull(expr))
            }
            "PropertyIsNil" => {
                let expr = self.single_expr(node)?;
                Ok(Filter::IsNil {
                    expr,
                    nil_reason: node.attribute("nilReason").map(str::to_string),
                })
            }
            "PropertyIsBetween" => self.between(node),
            "FeatureId" | "GmlObjectId" | "ResourceId" => {
                Ok(Filter::ResourceIds(vec![resource_id(&node)?]))
            }
            "Function" => Ok(Filter::Function(self.expr(node)?)),
            "Include" => Ok(Filter::Constant(true)),
            "Exclude" => Ok(Filter::Constant(false)),
            _ => {
                if let Some(op) = SpatialOp::from_name(name) {
                    self.spatial(node, op)
                } else if let Some(op) = TemporalOp::from_name(name) {
                    self.temporal(node, op)
                } else {
                    self.err(format!("unsupported filter operator `{name}`"))
                }
            }
        }
    }

    /// Extension operators: PropertyIsILike (case-insensitive like), PropertyIsIn (value list)
    fn extension(&self, node: roxmltree::Node) -> Result<Filter, FilterError> {
        match node.tag_name().name() {
            "PropertyIsILike" => match self.like(node)? {
                Filter::Like {
                    expr,
                    pattern,
                    wild_card,
                    single_char,
                    escape_char,
                    ..
                } => Ok(Filter::Like {
                    expr,
                    pattern,
                    wild_card,
                    single_char,
                    escape_char,
                    match_case: false,
                }),
                f => Ok(f),
            },
            "PropertyIsIn" => {
                let mut it = elements(node);
                let expr =
                    self.expr(it.next().ok_or_else(|| {
                        FilterError::Parse("PropertyIsIn without property".into())
                    })?)?;
                let mut alts: Vec<Filter> = it
                    .map(|c| self.expr(c))
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .map(|v| Filter::Comparison {
                        op: ComparisonOp::EqualTo,
                        left: expr.clone(),
                        right: v,
                        match_case: true,
                        match_action: MatchAction::Any,
                    })
                    .collect();
                match alts.len() {
                    0 => Ok(Filter::Constant(false)),
                    1 => Ok(alts.pop().expect("one")),
                    _ => Ok(Filter::Or(alts)),
                }
            }
            other => self.err(format!("unsupported extension operator `{other}`")),
        }
    }

    fn match_case(node: &roxmltree::Node) -> bool {
        node.attribute("matchCase")
            .map(|v| v != "false" && v != "0")
            .unwrap_or(true)
    }

    fn comparison(&self, node: roxmltree::Node, name: &str) -> Result<Filter, FilterError> {
        let op = match name {
            "PropertyIsEqualTo" => ComparisonOp::EqualTo,
            "PropertyIsNotEqualTo" => ComparisonOp::NotEqualTo,
            "PropertyIsLessThan" => ComparisonOp::LessThan,
            "PropertyIsGreaterThan" => ComparisonOp::GreaterThan,
            "PropertyIsLessThanOrEqualTo" => ComparisonOp::LessThanOrEqualTo,
            _ => ComparisonOp::GreaterThanOrEqualTo,
        };
        let exprs = elements(node)
            .map(|c| self.expr(c))
            .collect::<Result<Vec<_>, _>>()?;
        if exprs.len() != 2 {
            return self.err(format!("{name} requires two expressions"));
        }
        let mut it = exprs.into_iter();
        let match_action = match node.attribute("matchAction") {
            Some("All") => MatchAction::All,
            Some("One") => MatchAction::One,
            Some("Any") | None => MatchAction::Any,
            Some(v) => return self.err(format!("invalid matchAction `{v}`")),
        };
        Ok(Filter::Comparison {
            op,
            left: it.next().expect("two"),
            right: it.next().expect("two"),
            match_case: Self::match_case(&node),
            match_action,
        })
    }

    fn like(&self, node: roxmltree::Node) -> Result<Filter, FilterError> {
        let mut expr = None;
        let mut pattern = None;
        for c in elements(node) {
            if is_filter_ns(&c) && c.tag_name().name() == "Literal" {
                pattern = Some(c.text().unwrap_or("").to_string());
            } else {
                expr = Some(self.expr(c)?);
            }
        }
        let attr_char = |names: &[&str], default: char| -> Result<char, FilterError> {
            for n in names {
                if let Some(v) = node.attribute(*n) {
                    return v
                        .chars()
                        .next()
                        .ok_or_else(|| FilterError::Parse(format!("empty `{n}` attribute")));
                }
            }
            Ok(default)
        };
        Ok(Filter::Like {
            expr: expr
                .ok_or_else(|| FilterError::Parse("PropertyIsLike without property".into()))?,
            pattern: pattern
                .ok_or_else(|| FilterError::Parse("PropertyIsLike without literal".into()))?,
            wild_card: attr_char(&["wildCard"], '*')?,
            single_char: attr_char(&["singleChar"], '?')?,
            escape_char: attr_char(&["escapeChar", "escape"], '\\')?,
            match_case: Self::match_case(&node),
        })
    }

    fn between(&self, node: roxmltree::Node) -> Result<Filter, FilterError> {
        let mut expr = None;
        let mut lower = None;
        let mut upper = None;
        for c in elements(node) {
            match (is_filter_ns(&c), c.tag_name().name()) {
                (true, "LowerBoundary") => lower = Some(self.single_expr(c)?),
                (true, "UpperBoundary") => upper = Some(self.single_expr(c)?),
                _ => expr = Some(self.expr(c)?),
            }
        }
        match (expr, lower, upper) {
            (Some(expr), Some(lower), Some(upper)) => Ok(Filter::Between { expr, lower, upper }),
            _ => self.err("PropertyIsBetween requires expression, LowerBoundary and UpperBoundary"),
        }
    }

    fn single_expr(&self, node: roxmltree::Node) -> Result<Expr, FilterError> {
        let mut it = elements(node);
        let first = it
            .next()
            .ok_or_else(|| FilterError::Parse(format!("empty `{}`", node.tag_name().name())))?;
        self.expr(first)
    }

    fn resolve_prefix(&self, node: &roxmltree::Node, prefix: &str) -> Option<String> {
        node.lookup_namespace_uri(Some(prefix))
            .map(str::to_string)
            .or_else(|| self.ctx.namespaces.get(prefix).cloned())
    }

    fn property_path(&self, node: &roxmltree::Node) -> Result<PropertyPath, FilterError> {
        let text = node.text().unwrap_or("");
        PropertyPath::parse(text, &|prefix| self.resolve_prefix(node, prefix))
    }

    fn expr(&self, node: roxmltree::Node) -> Result<Expr, FilterError> {
        if gml::is_gml_element(&node) {
            return Ok(Expr::GeometryLiteral(self.geometry_literal(node)?));
        }
        if !is_filter_ns(&node) {
            return self.err(format!(
                "unexpected element `{}` in expression",
                node.tag_name().name()
            ));
        }
        match node.tag_name().name() {
            "PropertyName" | "ValueReference" => Ok(Expr::Property(self.property_path(&node)?)),
            "Literal" => {
                if let Some(child) = elements(node).next() {
                    if gml::is_gml_element(&child) {
                        return Ok(Expr::GeometryLiteral(self.geometry_literal(child)?));
                    }
                }
                Ok(Expr::Literal(node.text().unwrap_or("").to_string()))
            }
            "Function" => {
                let name = node
                    .attribute("name")
                    .ok_or_else(|| FilterError::Parse("Function without name".into()))?;
                let args = elements(node)
                    .map(|c| self.expr(c))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Expr::Function {
                    name: name.to_string(),
                    args,
                })
            }
            n @ ("Add" | "Sub" | "Mul" | "Div") => {
                let op = match n {
                    "Add" => ArithOp::Add,
                    "Sub" => ArithOp::Sub,
                    "Mul" => ArithOp::Mul,
                    _ => ArithOp::Div,
                };
                let args = elements(node)
                    .map(|c| self.expr(c))
                    .collect::<Result<Vec<_>, _>>()?;
                if args.len() != 2 {
                    return self.err(format!("{n} requires two operands"));
                }
                let mut it = args.into_iter();
                Ok(Expr::Arith {
                    op,
                    left: Box::new(it.next().expect("two")),
                    right: Box::new(it.next().expect("two")),
                })
            }
            other => self.err(format!("unsupported expression `{other}`")),
        }
    }

    fn geometry_literal(&self, node: roxmltree::Node) -> Result<GeometryLiteral, FilterError> {
        let parsed = gml::parse_geometry(node).map_err(|e| FilterError::Parse(e.to_string()))?;
        let (srid, swap) = match &parsed.srs_name {
            Some(name) => {
                let crs = Crs::parse(name).map_err(|_| FilterError::Crs(name.clone()))?;
                if !Crs::is_known(crs.epsg) {
                    return Err(FilterError::Crs(name.clone()));
                }
                (Some(crs.epsg), crs.swap_xy())
            }
            None => (self.ctx.literal_srid, self.ctx.default_swap_xy),
        };
        let mut geometry = parsed.geometry;
        if swap {
            gml::swap_xy(&mut geometry);
        }
        Ok(GeometryLiteral {
            geometry,
            srid,
            srs_name: parsed.srs_name,
        })
    }

    fn spatial(&self, node: roxmltree::Node, op: SpatialOp) -> Result<Filter, FilterError> {
        let mut props = Vec::new();
        let mut geometry = None;
        let mut distance = None;
        for c in elements(node) {
            if gml::is_gml_element(&c) {
                geometry = Some(self.geometry_literal(c)?);
            } else if is_filter_ns(&c) && c.tag_name().name() == "Distance" {
                let value: f64 = c
                    .text()
                    .unwrap_or("")
                    .trim()
                    .parse()
                    .map_err(|_| FilterError::Parse("invalid Distance".into()))?;
                let units = c
                    .attribute("uom")
                    .or_else(|| c.attribute("units"))
                    .unwrap_or("")
                    .to_string();
                distance = Some((value, units));
            } else if matches!(c.tag_name().name(), "PropertyName" | "ValueReference")
                && c.text().map(str::trim).unwrap_or("").is_empty()
            {
                // empty property name: default geometry (as in GeoServer)
            } else {
                match self.expr(c)? {
                    Expr::GeometryLiteral(g) => geometry = Some(g),
                    e => props.push(e),
                }
            }
        }
        if matches!(op, SpatialOp::DWithin | SpatialOp::Beyond) && distance.is_none() {
            return self.err(format!("{} requires a Distance", op.name()));
        }
        let (property, operand) = match (geometry, props.len()) {
            // no property: default geometry
            (Some(g), 0) => (None, SpatialOperand::Geometry(g)),
            (Some(g), 1) => (Some(props.remove(0)), SpatialOperand::Geometry(g)),
            (None, 2) => {
                let second = props.remove(1);
                (Some(props.remove(0)), SpatialOperand::Expr(second))
            }
            _ => {
                return self.err(format!(
                    "{} requires a property and a geometry operand",
                    op.name()
                ))
            }
        };
        if op == SpatialOp::BBox {
            if let SpatialOperand::Geometry(g) = &operand {
                // BBOX operand must be an envelope; use its bounding rectangle
                use geo::BoundingRect;
                let Some(rect) = g.geometry.bounding_rect() else {
                    return self.err("empty BBOX envelope");
                };
                let mut g = g.clone();
                g.geometry = Geometry::Polygon(rect.to_polygon());
                return Ok(Filter::Spatial {
                    op,
                    property,
                    operand: SpatialOperand::Geometry(g),
                    distance,
                });
            }
        }
        Ok(Filter::Spatial {
            op,
            property,
            operand,
            distance,
        })
    }

    fn temporal(&self, node: roxmltree::Node, op: TemporalOp) -> Result<Filter, FilterError> {
        let mut exprs = Vec::new();
        let mut literal = None;
        for c in elements(node) {
            if gml::is_gml_element(&c) {
                literal = Some(temporal_literal(c)?);
            } else if is_filter_ns(&c) && c.tag_name().name() == "Literal" {
                match elements(c).next() {
                    Some(t) if gml::is_gml_element(&t) => literal = Some(temporal_literal(t)?),
                    _ => {
                        literal = Some(TemporalLiteral::Instant(
                            c.text().unwrap_or("").trim().to_string(),
                        ))
                    }
                }
            } else {
                exprs.push(self.expr(c)?);
            }
        }
        let (expr, operand) = match (literal, exprs.len()) {
            (Some(l), 1) => (exprs.remove(0), TemporalOperand::Literal(l)),
            (None, 2) => {
                let second = exprs.remove(1);
                (exprs.remove(0), TemporalOperand::Expr(second))
            }
            _ => {
                return self.err(format!(
                    "{} requires a property and a temporal operand",
                    op.name()
                ))
            }
        };
        Ok(Filter::Temporal { op, expr, operand })
    }
}

/// Bounds used for indeterminate (open) period ends
pub const TIME_MIN: &str = "0001-01-01T00:00:00Z";
pub const TIME_MAX: &str = "9999-12-31T23:59:59.999Z";

/// Value of a gml:TimePosition element, resolving gml:indeterminatePosition
/// (`now`, `unknown`, `before`, `after`). `begin` selects the open bound for unknown values.
fn time_position(pos: roxmltree::Node, begin: bool) -> Option<String> {
    let text = pos.text().map(str::trim).filter(|t| !t.is_empty());
    let open = || Some(if begin { TIME_MIN } else { TIME_MAX }.to_string());
    match pos.attribute("indeterminatePosition") {
        Some("now") => {
            Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        }
        Some("unknown") => open(),
        // some time before/after the given value: unbounded on that side
        Some("before") if begin => open(),
        Some("after") if !begin => open(),
        _ => text.map(str::to_string),
    }
}

fn find<'a, 'input>(
    n: roxmltree::Node<'a, 'input>,
    name: &str,
) -> Option<roxmltree::Node<'a, 'input>> {
    n.descendants()
        .find(|d| d.is_element() && d.tag_name().name() == name)
}

/// Parse gml:TimeInstant / gml:TimePeriod
fn temporal_literal(node: roxmltree::Node) -> Result<TemporalLiteral, FilterError> {
    match node.tag_name().name() {
        "TimeInstant" => find(node, "timePosition")
            .and_then(|p| time_position(p, true))
            .map(TemporalLiteral::Instant)
            .ok_or_else(|| FilterError::Parse("TimeInstant without timePosition".into())),
        "TimePeriod" => {
            let side = |pos: &str, wrapper: &str, begin: bool| -> Option<String> {
                elements(node)
                    .find(|c| c.tag_name().name() == pos)
                    .or_else(|| {
                        elements(node)
                            .find(|c| c.tag_name().name() == wrapper)
                            .and_then(|w| find(w, "timePosition"))
                    })
                    .and_then(|p| time_position(p, begin))
            };
            match (
                side("beginPosition", "begin", true),
                side("endPosition", "end", false),
            ) {
                (Some(begin), Some(end)) => Ok(TemporalLiteral::Period { begin, end }),
                _ => Err(FilterError::Parse(
                    "TimePeriod requires begin and end".into(),
                )),
            }
        }
        other => Err(FilterError::Parse(format!(
            "unsupported temporal operand `{other}`"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(version: FilterVersion) -> ParseContext {
        let mut ctx = ParseContext::new(version);
        ctx.namespaces.insert(
            "cdf".to_string(),
            "http://www.opengis.net/cite/data".to_string(),
        );
        ctx
    }

    fn parse(xml: &str, version: FilterVersion) -> Result<Filter, FilterError> {
        parse_filter_str(xml, &ctx(version))
    }

    #[test]
    fn comparison_1_0() {
        let f = parse(r#"<ogc:Filter xmlns:ogc="http://www.opengis.net/ogc"><ogc:PropertyIsEqualTo><ogc:PropertyName>cdf:integers</ogc:PropertyName><ogc:Literal>7</ogc:Literal></ogc:PropertyIsEqualTo></ogc:Filter>"#, FilterVersion::V100).unwrap();
        let Filter::Comparison {
            op,
            left,
            right,
            match_case,
            ..
        } = f
        else {
            panic!("{f:?}")
        };
        assert_eq!(op, ComparisonOp::EqualTo);
        assert!(match_case);
        let Expr::Property(p) = left else { panic!() };
        assert_eq!(
            p.first().ns.as_deref(),
            Some("http://www.opengis.net/cite/data")
        );
        assert_eq!(right, Expr::Literal("7".to_string()));
    }

    #[test]
    fn fes2_literal_first_and_match_action() {
        let f = parse(r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0"><fes:PropertyIsLessThan matchCase="false" matchAction="All"><fes:Literal>5</fes:Literal><fes:ValueReference xmlns:tns="http://example.com">tns:p</fes:ValueReference></fes:PropertyIsLessThan></fes:Filter>"#, FilterVersion::V200).unwrap();
        let Filter::Comparison {
            left,
            match_case,
            match_action,
            ..
        } = f
        else {
            panic!()
        };
        assert!(!match_case);
        assert_eq!(match_action, MatchAction::All);
        assert_eq!(left, Expr::Literal("5".to_string()));
    }

    #[test]
    fn identifiers() {
        let f = parse(r#"<ogc:Filter xmlns:ogc="http://www.opengis.net/ogc"><ogc:FeatureId fid="Inserts.1"/><ogc:FeatureId fid="Inserts.2"/></ogc:Filter>"#, FilterVersion::V100).unwrap();
        assert_eq!(f.resource_ids().unwrap().len(), 2);
        let f = parse(r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0"><fes:ResourceId rid="a.1" version="LAST"/></fes:Filter>"#, FilterVersion::V200).unwrap();
        assert_eq!(
            f.resource_ids().unwrap()[0].version,
            Some(VersionSpec::Last)
        );
        let f = parse(r#"<ogc:Filter xmlns:ogc="http://www.opengis.net/ogc" xmlns:gml="http://www.opengis.net/gml"><ogc:GmlObjectId gml:id="f001"/></ogc:Filter>"#, FilterVersion::V110).unwrap();
        assert_eq!(f.resource_ids().unwrap()[0].rid, "f001");
    }

    #[test]
    fn like_attributes() {
        let f = parse(r#"<ogc:Filter xmlns:ogc="http://www.opengis.net/ogc"><ogc:PropertyIsLike wildCard="*" singleChar="." escape="\"><ogc:PropertyName>cdf:string2</ogc:PropertyName><ogc:Literal>s.met*s</ogc:Literal></ogc:PropertyIsLike></ogc:Filter>"#, FilterVersion::V100).unwrap();
        let Filter::Like {
            pattern,
            single_char,
            escape_char,
            ..
        } = f
        else {
            panic!()
        };
        assert_eq!(
            (pattern.as_str(), single_char, escape_char),
            ("s.met*s", '.', '\\')
        );
    }

    #[test]
    fn spatial_with_stray_default_namespace() {
        let f = parse(r#"<ogc:Filter xmlns:ogc="http://www.opengis.net/ogc" xmlns:gml="http://www.opengis.net/gml"><ogc:BBOX><ogc:PropertyName>gml:pointProperty</ogc:PropertyName><gml:Box xmlns="http://www.opengis.net/cite/spatialTestSuite" srsName="EPSG:32615"><gml:coordinates>500000,500000 500100,500100</gml:coordinates></gml:Box></ogc:BBOX></ogc:Filter>"#, FilterVersion::V100).unwrap();
        let Filter::Spatial {
            op,
            property,
            operand: SpatialOperand::Geometry(g),
            ..
        } = f
        else {
            panic!()
        };
        assert_eq!(op, SpatialOp::BBox);
        assert!(property.is_some());
        assert_eq!(g.srid, Some(32615));
    }

    #[test]
    fn spatial_axis_order_and_distance() {
        let f = parse(r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:gml="http://www.opengis.net/gml/3.2"><fes:DWithin><fes:ValueReference>geom</fes:ValueReference><gml:Point gml:id="p" srsName="urn:ogc:def:crs:EPSG::4326"><gml:pos>47 8</gml:pos></gml:Point><fes:Distance uom="m">10</fes:Distance></fes:DWithin></fes:Filter>"#, FilterVersion::V200).unwrap();
        let Filter::Spatial {
            operand: SpatialOperand::Geometry(g),
            distance,
            ..
        } = f
        else {
            panic!()
        };
        assert_eq!(g.geometry, geo::point!(x: 8.0, y: 47.0).into());
        assert_eq!(distance, Some((10.0, "m".to_string())));
        // BBOX without property
        let f = parse(r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:gml="http://www.opengis.net/gml/3.2"><fes:BBOX><gml:Envelope srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>40 5</gml:lowerCorner><gml:upperCorner>50 10</gml:upperCorner></gml:Envelope></fes:BBOX></fes:Filter>"#, FilterVersion::V200).unwrap();
        let Filter::Spatial {
            property: None,
            operand: SpatialOperand::Geometry(g),
            ..
        } = f
        else {
            panic!()
        };
        use geo::BoundingRect;
        let r = g.geometry.bounding_rect().unwrap();
        assert_eq!(
            (r.min().x, r.min().y, r.max().x, r.max().y),
            (5., 40., 10., 50.)
        );
        assert!(matches!(
            parse(
                r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:gml="http://www.opengis.net/gml/3.2"><fes:BBOX><gml:Envelope srsName="urn:ogc:def:crs:EPSG::999999"><gml:lowerCorner>40 5</gml:lowerCorner><gml:upperCorner>50 10</gml:upperCorner></gml:Envelope></fes:BBOX></fes:Filter>"#,
                FilterVersion::V200
            ),
            Err(FilterError::Crs(_))
        ));
    }

    #[test]
    fn spatial_join_operands() {
        let f = parse(r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0"><fes:Intersects><fes:ValueReference>a:A/a:geom</fes:ValueReference><fes:ValueReference>b:B/b:geom</fes:ValueReference></fes:Intersects></fes:Filter>"#, FilterVersion::V200).unwrap();
        assert!(matches!(
            f,
            Filter::Spatial {
                operand: SpatialOperand::Expr(_),
                ..
            }
        ));
    }

    #[test]
    fn temporal_operands() {
        let f = parse(r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:gml="http://www.opengis.net/gml/3.2"><fes:During><fes:ValueReference>t</fes:ValueReference><gml:TimePeriod gml:id="TP1"><gml:beginPosition>2020-01-01T00:00:00Z</gml:beginPosition><gml:endPosition>2021-01-01T00:00:00Z</gml:endPosition></gml:TimePeriod></fes:During></fes:Filter>"#, FilterVersion::V200).unwrap();
        let Filter::Temporal {
            op,
            operand: TemporalOperand::Literal(TemporalLiteral::Period { begin, .. }),
            ..
        } = f
        else {
            panic!()
        };
        assert_eq!(op, TemporalOp::During);
        assert_eq!(begin, "2020-01-01T00:00:00Z");
        let f = parse(r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:gml="http://www.opengis.net/gml/3.2"><fes:After><fes:ValueReference>t</fes:ValueReference><gml:TimeInstant gml:id="T1"><gml:timePosition>2020-01-01T00:00:00+09:00</gml:timePosition></gml:TimeInstant></fes:After></fes:Filter>"#, FilterVersion::V200).unwrap();
        assert!(matches!(
            f,
            Filter::Temporal {
                operand: TemporalOperand::Literal(TemporalLiteral::Instant(_)),
                ..
            }
        ));
    }

    #[test]
    fn indeterminate_time_positions() {
        let period = |begin: &str, end: &str| {
            let f = parse(&format!(r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:gml="http://www.opengis.net/gml/3.2"><fes:During><fes:ValueReference>t</fes:ValueReference><gml:TimePeriod gml:id="TP1">{begin}{end}</gml:TimePeriod></fes:During></fes:Filter>"#), FilterVersion::V200).unwrap();
            let Filter::Temporal {
                operand: TemporalOperand::Literal(TemporalLiteral::Period { begin, end }),
                ..
            } = f
            else {
                panic!()
            };
            (begin, end)
        };
        // DGIWG ETS: open end
        let (b, e) = period(
            "<gml:beginPosition>2020-01-01T00:00:00.000</gml:beginPosition>",
            r#"<gml:endPosition indeterminatePosition="unknown"/>"#,
        );
        assert_eq!(
            (b.as_str(), e.as_str()),
            ("2020-01-01T00:00:00.000", TIME_MAX)
        );
        let (b, e) = period(
            r#"<gml:beginPosition indeterminatePosition="before">2020-01-01T00:00:00Z</gml:beginPosition>"#,
            r#"<gml:endPosition indeterminatePosition="after">2021-01-01T00:00:00Z</gml:endPosition>"#,
        );
        assert_eq!((b.as_str(), e.as_str()), (TIME_MIN, TIME_MAX));
        let (b, e) = period(
            r#"<gml:beginPosition indeterminatePosition="unknown"/>"#,
            r#"<gml:endPosition indeterminatePosition="now"/>"#,
        );
        assert_eq!(b, TIME_MIN);
        assert!(crate::wfs::filter::temporal::parse_datetime(&e).unwrap() <= chrono::Utc::now());
        // begin/end wrapper form
        let (b, e) = period(
            r#"<gml:begin><gml:TimeInstant gml:id="b"><gml:timePosition>2020-01-01T00:00:00Z</gml:timePosition></gml:TimeInstant></gml:begin>"#,
            r#"<gml:end><gml:TimeInstant gml:id="e"><gml:timePosition indeterminatePosition="unknown"/></gml:TimeInstant></gml:end>"#,
        );
        assert_eq!((b.as_str(), e.as_str()), ("2020-01-01T00:00:00Z", TIME_MAX));
        for s in [TIME_MIN, TIME_MAX] {
            assert!(
                crate::wfs::filter::temporal::parse_datetime(s).is_some(),
                "{s}"
            );
        }
    }

    #[test]
    fn logical_and_functions_and_arithmetic() {
        let f = parse(r#"<ogc:Filter xmlns:ogc="http://www.opengis.net/ogc"><ogc:And><ogc:Not><ogc:PropertyIsNull><ogc:PropertyName>a</ogc:PropertyName></ogc:PropertyIsNull></ogc:Not><ogc:PropertyIsGreaterThan><ogc:Add><ogc:PropertyName>a</ogc:PropertyName><ogc:Literal>1</ogc:Literal></ogc:Add><ogc:Function name="abs"><ogc:Literal>-3</ogc:Literal></ogc:Function></ogc:PropertyIsGreaterThan></ogc:And></ogc:Filter>"#, FilterVersion::V110).unwrap();
        let Filter::And(ops) = &f else { panic!() };
        assert_eq!(ops.len(), 2);
        assert_eq!(f.properties().len(), 2);
    }

    #[test]
    fn extension_operators() {
        let f = parse(r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:x="https://www.bbox.earth/fes"><x:PropertyIsIn><fes:ValueReference>id</fes:ValueReference><fes:Literal>a</fes:Literal><fes:Literal>b</fes:Literal></x:PropertyIsIn></fes:Filter>"#, FilterVersion::V200).unwrap();
        assert!(matches!(&f, Filter::Or(alts) if alts.len() == 2));
        let f = parse(r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:x="https://www.bbox.earth/fes"><x:PropertyIsILike wildCard="*" singleChar="?" escapeChar="\"><fes:ValueReference>n</fes:ValueReference><fes:Literal>AB*</fes:Literal></x:PropertyIsILike></fes:Filter>"#, FilterVersion::V200).unwrap();
        assert!(matches!(
            f,
            Filter::Like {
                match_case: false,
                ..
            }
        ));
    }

    #[test]
    fn invalid_filters() {
        for xml in [
            r#"<ogc:Filter xmlns:ogc="http://www.opengis.net/ogc"/>"#,
            r#"<ogc:Filter xmlns:ogc="http://www.opengis.net/ogc"><ogc:Foo/></ogc:Filter>"#,
            r#"<ogc:Filter xmlns:ogc="http://www.opengis.net/ogc"><ogc:PropertyIsEqualTo><ogc:Literal>1</ogc:Literal></ogc:PropertyIsEqualTo></ogc:Filter>"#,
            r#"<Filter/>"#,
            "<ogc:Filter",
        ] {
            assert!(parse(xml, FilterVersion::V110).is_err(), "{xml}");
        }
    }
}
