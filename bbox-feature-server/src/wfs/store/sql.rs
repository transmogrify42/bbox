//! Translation of filters into SQL (PostGIS and GeoPackage/SQLite).
//!
//! Predicates on plain columns are always pushed down with the column left untouched and
//! parameters cast to the column type, so that database indexes can be used. Parts of a
//! filter that can not be expressed in SQL are returned as residual filter, which is
//! evaluated on the streamed features.

use crate::wfs::filter::*;
use crate::wfs::model::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    Sqlite,
    Postgres,
    ClickHouse,
}

/// Bind parameter value
#[derive(Clone, Debug, PartialEq)]
pub enum SqlValue {
    Text(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Bytes(Vec<u8>),
    TextArray(Vec<String>),
}

/// Column metadata needed for translation
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnInfo {
    /// Database type used for parameter casts (Postgres `format_type`, e.g. `bigint`, `uuid`)
    pub db_type: String,
    /// GeoPackage rtree table of a geometry column
    pub rtree: Option<String>,
    /// SQL expressions of x and y of point geometries (ClickHouse range pushdown)
    pub coords: Option<(String, String)>,
}

/// How feature ids map to columns
#[derive(Clone, Debug, PartialEq)]
pub enum IdMapping {
    /// `{type}.{pk}` ids
    Pk { column: String, db_type: String },
    /// Stored gml:id column with `{type}.{pk}` fallback
    GmlId { column: String, pk: String },
    /// No usable key
    None,
}

/// Translation context for a feature type stored in a table/query
pub struct SqlContext<'a> {
    pub dialect: Dialect,
    pub feature_type: &'a FeatureTypeDef,
    /// Column info aligned to feature type properties (None: property not stored)
    pub columns: &'a [Option<ColumnInfo>],
    pub id: &'a IdMapping,
    /// Primary key column used for rtree lookups (GeoPackage)
    pub rowid_column: Option<&'a str>,
}

/// SQL predicate with bind values
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SqlExpr {
    pub sql: String,
    pub binds: Vec<SqlValue>,
}

/// Result of splitting a filter
#[derive(Clone, Debug, PartialEq)]
pub struct Translation {
    /// SQL predicate (None: no pushdown)
    pub sql: Option<SqlExpr>,
    /// Filter to evaluate on fetched features (None: SQL is exact)
    pub residual: Option<Filter>,
}

pub fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// Translate a prepared filter (literals in native CRS)
pub fn translate(filter: &Filter, ctx: &SqlContext) -> Translation {
    match tr(filter, ctx) {
        T::Exact(e) => Translation {
            sql: Some(e),
            residual: None,
        },
        T::Prefilter(e) => Translation {
            sql: Some(e),
            residual: Some(filter.clone()),
        },
        T::None => {
            // And: push down translatable conjuncts
            if let Filter::And(parts) = filter {
                let mut sqls = Vec::new();
                let mut residual = Vec::new();
                for p in parts {
                    match tr(p, ctx) {
                        T::Exact(e) => sqls.push(e),
                        T::Prefilter(e) => {
                            sqls.push(e);
                            residual.push(p.clone());
                        }
                        T::None => residual.push(p.clone()),
                    }
                }
                let residual = match residual.len() {
                    0 => None,
                    1 => residual.pop(),
                    _ => Some(Filter::And(residual)),
                };
                return Translation {
                    sql: join(sqls, " AND "),
                    residual,
                };
            }
            Translation {
                sql: None,
                residual: Some(filter.clone()),
            }
        }
    }
}

/// Internal translation result
enum T {
    /// Equivalent SQL
    Exact(SqlExpr),
    /// SQL selecting a superset (index prefilter)
    Prefilter(SqlExpr),
    /// Not translatable
    None,
}

fn expr(sql: impl Into<String>, binds: Vec<SqlValue>) -> SqlExpr {
    SqlExpr {
        sql: sql.into(),
        binds,
    }
}

fn join(parts: Vec<SqlExpr>, sep: &str) -> Option<SqlExpr> {
    match parts.len() {
        0 => None,
        1 => parts.into_iter().next(),
        _ => {
            let mut binds = Vec::new();
            let sql = parts
                .into_iter()
                .map(|p| {
                    binds.extend(p.binds);
                    format!("({})", p.sql)
                })
                .collect::<Vec<_>>()
                .join(sep);
            Some(SqlExpr { sql, binds })
        }
    }
}

/// Stored simple column referenced by an expression
struct Col<'a> {
    prop: &'a PropertyDef,
    info: &'a ColumnInfo,
}

fn column<'a>(e: &Expr, ctx: &'a SqlContext) -> Option<Col<'a>> {
    let Expr::Property(path) = e else { return None };
    let (idx, prop, first) = resolve_property(ctx.feature_type, path)?;
    if path.steps.len() != first + 1 {
        return None;
    }
    let step = &path.steps[first];
    if step.index.is_some() || step.predicate.is_some() || prop.is_xml() {
        return None;
    }
    let info = ctx.columns.get(idx)?.as_ref()?;
    Some(Col { prop, info })
}

/// Property without storage column (always null), excluding computed geometries
fn absent<'a>(e: &Expr, ctx: &'a SqlContext) -> Option<&'a PropertyDef> {
    let Expr::Property(path) = e else { return None };
    let (idx, prop, _) = resolve_property(ctx.feature_type, path)?;
    if ctx.columns.get(idx).map(|c| c.is_none()).unwrap_or(false) && !prop.is_geometry() {
        Some(prop)
    } else {
        None
    }
}

fn literal(e: &Expr) -> Option<&str> {
    match e {
        Expr::Literal(s) => Some(s.as_str()),
        _ => None,
    }
}

fn is_text_type(db_type: &str) -> bool {
    matches!(
        db_type,
        "text" | "character varying" | "varchar" | "TEXT" | "String" | "name" | "bpchar"
    ) || db_type.starts_with("character varying")
        || db_type.starts_with("TEXT")
        || db_type.starts_with("VARCHAR")
}

/// Bind parameter for a literal compared with a column: (placeholder SQL, value)
fn param(lit: &str, col: &Col, ctx: &SqlContext) -> Option<(String, SqlValue)> {
    let t = lit.trim();
    let db_type = &col.info.db_type;
    match ctx.dialect {
        Dialect::Postgres => {
            let cast = |value: String| Some((format!("?::{db_type}"), SqlValue::Text(value)));
            match &col.prop.value_type {
                ValueType::Integer => {
                    if t.parse::<i64>().is_ok() {
                        cast(t.to_string())
                    } else if t.parse::<f64>().is_ok() {
                        Some((
                            "?::double precision".to_string(),
                            SqlValue::Text(t.to_string()),
                        ))
                    } else {
                        None
                    }
                }
                ValueType::Double | ValueType::Decimal => {
                    t.parse::<f64>().ok().and_then(|_| cast(t.to_string()))
                }
                ValueType::Boolean => match t {
                    "true" | "1" => cast("true".to_string()),
                    "false" | "0" => cast("false".to_string()),
                    _ => None,
                },
                ValueType::DateTime => crate::wfs::filter::temporal::parse_datetime(t)
                    .and_then(|dt| cast(dt.to_rfc3339())),
                ValueType::Date => {
                    if t.len() >= 10
                        && chrono::NaiveDate::parse_from_str(&t[..10], "%Y-%m-%d").is_ok()
                    {
                        cast(t[..10].to_string())
                    } else {
                        None
                    }
                }
                ValueType::Geometry(_) | ValueType::Complex => None,
                _ => cast(lit.to_string()),
            }
        }
        Dialect::ClickHouse
            if matches!(col.prop.value_type, ValueType::DateTime | ValueType::Date) =>
        {
            let dt = crate::wfs::filter::temporal::parse_datetime(t)?;
            if col.prop.value_type == ValueType::Date {
                Some((
                    "toDate(?)".to_string(),
                    SqlValue::Text(dt.format("%Y-%m-%d").to_string()),
                ))
            } else {
                Some((
                    "parseDateTime64BestEffort(?, 6, 'UTC')".to_string(),
                    SqlValue::Text(dt.format("%Y-%m-%d %H:%M:%S%.f").to_string()),
                ))
            }
        }
        Dialect::Sqlite | Dialect::ClickHouse => match &col.prop.value_type {
            ValueType::Integer => t
                .parse::<i64>()
                .map(SqlValue::Int)
                .ok()
                .or_else(|| t.parse::<f64>().ok().map(SqlValue::Float))
                .map(|v| ("?".to_string(), v)),
            ValueType::Double | ValueType::Decimal => t
                .parse::<f64>()
                .ok()
                .map(|v| ("?".to_string(), SqlValue::Float(v))),
            ValueType::Boolean => match t {
                "true" | "1" => Some(("?".to_string(), SqlValue::Int(1))),
                "false" | "0" => Some(("?".to_string(), SqlValue::Int(0))),
                _ => None,
            },
            // text encoded dates in SQLite are not reliably comparable
            ValueType::Date | ValueType::DateTime | ValueType::Time => None,
            ValueType::Geometry(_) | ValueType::Complex => None,
            _ => Some(("?".to_string(), SqlValue::Text(lit.to_string()))),
        },
    }
}

fn op_sql(op: ComparisonOp) -> &'static str {
    match op {
        ComparisonOp::EqualTo => "=",
        ComparisonOp::NotEqualTo => "<>",
        ComparisonOp::LessThan => "<",
        ComparisonOp::GreaterThan => ">",
        ComparisonOp::LessThanOrEqualTo => "<=",
        ComparisonOp::GreaterThanOrEqualTo => ">=",
    }
}

fn flip(op: ComparisonOp) -> ComparisonOp {
    match op {
        ComparisonOp::LessThan => ComparisonOp::GreaterThan,
        ComparisonOp::GreaterThan => ComparisonOp::LessThan,
        ComparisonOp::LessThanOrEqualTo => ComparisonOp::GreaterThanOrEqualTo,
        ComparisonOp::GreaterThanOrEqualTo => ComparisonOp::LessThanOrEqualTo,
        o => o,
    }
}

/// Column reference with case folding for case-insensitive string comparison
fn col_ref(col: &Col, match_case: bool, ctx: &SqlContext) -> String {
    let name = quote_ident(&col.prop.column);
    if match_case || !matches!(col.prop.value_type, ValueType::String | ValueType::Uri) {
        name
    } else {
        match ctx.dialect {
            Dialect::ClickHouse => format!("lowerUTF8({name})"),
            _ => format!("lower({name})"),
        }
    }
}

fn comparison(
    op: ComparisonOp,
    left: &Expr,
    right: &Expr,
    match_case: bool,
    ctx: &SqlContext,
) -> T {
    let (col, lit, op) = match (column(left, ctx), column(right, ctx)) {
        (Some(c), None) => match literal(right) {
            Some(l) => (c, l, op),
            None => return T::None,
        },
        (None, Some(c)) => match literal(left) {
            Some(l) => (c, l, flip(op)),
            None => return T::None,
        },
        (Some(a), Some(b)) => {
            if a.prop.value_type != b.prop.value_type {
                return T::None;
            }
            return T::Exact(expr(
                format!(
                    "{} {} {}",
                    col_ref(&a, match_case, ctx),
                    op_sql(op),
                    col_ref(&b, match_case, ctx)
                ),
                vec![],
            ));
        }
        _ => return T::None,
    };
    let Some((ph, value)) = param(lit, &col, ctx) else {
        return T::None;
    };
    let case_insensitive =
        !match_case && matches!(col.prop.value_type, ValueType::String | ValueType::Uri);
    let rhs = if case_insensitive {
        match ctx.dialect {
            Dialect::Sqlite => {
                return T::Exact(expr(
                    format!(
                        "{} {} {ph} COLLATE NOCASE",
                        quote_ident(&col.prop.column),
                        op_sql(op)
                    ),
                    vec![value],
                ))
            }
            Dialect::ClickHouse => format!("lowerUTF8({ph})"),
            Dialect::Postgres => format!("lower({ph})"),
        }
    } else {
        ph
    };
    T::Exact(expr(
        format!("{} {} {rhs}", col_ref(&col, match_case, ctx), op_sql(op)),
        vec![value],
    ))
}

/// Or of equalities on one column -> `= ANY(array)` / `IN (...)`
fn equality_list(parts: &[Filter], ctx: &SqlContext) -> Option<SqlExpr> {
    let mut column_name: Option<&str> = None;
    let mut values = Vec::new();
    let mut col_info = None;
    for p in parts {
        let Filter::Comparison {
            op: ComparisonOp::EqualTo,
            left,
            right,
            match_case: true,
            ..
        } = p
        else {
            return None;
        };
        let (col, lit) = match (column(left, ctx), column(right, ctx)) {
            (Some(c), None) => (c, literal(right)?),
            (None, Some(c)) => (c, literal(left)?),
            _ => return None,
        };
        match column_name {
            Some(n) if n != col.prop.column => return None,
            _ => column_name = Some(&col.prop.column),
        }
        let (_, value) = param(lit, &col, ctx)?;
        values.push(value);
        col_info = Some(col);
    }
    let col = col_info?;
    let name = quote_ident(&col.prop.column);
    match ctx.dialect {
        Dialect::Postgres => {
            // all values cast to the column type (fractional values on integer columns excluded)
            let texts = values
                .into_iter()
                .map(|v| match v {
                    SqlValue::Text(t) => Some(t),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?;
            if col.prop.value_type == ValueType::Integer
                && texts.iter().any(|t| t.trim().parse::<i64>().is_err())
            {
                return None;
            }
            Some(expr(
                format!("{name} = ANY(?::{}[])", col.info.db_type),
                vec![SqlValue::TextArray(texts)],
            ))
        }
        Dialect::Sqlite | Dialect::ClickHouse => {
            let placeholders = vec!["?"; values.len()].join(", ");
            Some(expr(format!("{name} IN ({placeholders})"), values))
        }
    }
}

/// Translate FES Like pattern into SQL LIKE (escape `\`) or GLOB pattern
fn like_pattern(pattern: &str, wild: char, single: char, escape: char, glob: bool) -> String {
    let mut out = String::new();
    let mut chars = pattern.chars();
    let lit = |c: char, out: &mut String| {
        if glob {
            match c {
                '*' | '?' | '[' => {
                    out.push('[');
                    out.push(c);
                    out.push(']');
                }
                c => out.push(c),
            }
        } else {
            if matches!(c, '%' | '_' | '\\') {
                out.push('\\');
            }
            out.push(c);
        }
    };
    while let Some(c) = chars.next() {
        if c == escape {
            if let Some(n) = chars.next() {
                lit(n, &mut out);
            }
        } else if c == wild {
            out.push(if glob { '*' } else { '%' });
        } else if c == single {
            out.push(if glob { '?' } else { '_' });
        } else {
            lit(c, &mut out);
        }
    }
    out
}

fn like(
    e: &Expr,
    pattern: &str,
    wild: char,
    single: char,
    escape: char,
    match_case: bool,
    ctx: &SqlContext,
) -> T {
    let Some(col) = column(e, ctx) else {
        return T::None;
    };
    if col.prop.value_type.is_geometry() {
        return T::None;
    }
    let name = quote_ident(&col.prop.column);
    match ctx.dialect {
        Dialect::Postgres => {
            let target = if is_text_type(&col.info.db_type) {
                name
            } else {
                format!("{name}::text")
            };
            let op = if match_case { "LIKE" } else { "ILIKE" };
            T::Exact(expr(
                format!("{target} {op} ? ESCAPE '\\'"),
                vec![SqlValue::Text(like_pattern(
                    pattern, wild, single, escape, false,
                ))],
            ))
        }
        Dialect::Sqlite => {
            if match_case {
                T::Exact(expr(
                    format!("{name} GLOB ?"),
                    vec![SqlValue::Text(like_pattern(
                        pattern, wild, single, escape, true,
                    ))],
                ))
            } else {
                T::Exact(expr(
                    format!("{name} LIKE ? ESCAPE '\\'"),
                    vec![SqlValue::Text(like_pattern(
                        pattern, wild, single, escape, false,
                    ))],
                ))
            }
        }
        Dialect::ClickHouse => {
            let op = if match_case { "LIKE" } else { "ILIKE" };
            T::Exact(expr(
                format!("toString({name}) {op} ?"),
                vec![SqlValue::Text(like_pattern(
                    pattern, wild, single, escape, false,
                ))],
            ))
        }
    }
}

fn geometry_column<'a>(property: &Option<Expr>, ctx: &'a SqlContext) -> Option<Col<'a>> {
    match property {
        Some(e) => {
            let c = column(e, ctx)?;
            c.prop.value_type.is_geometry().then_some(c)
        }
        None => {
            let (idx, prop) = ctx.feature_type.default_geometry()?;
            let info = ctx.columns.get(idx)?.as_ref()?;
            Some(Col { prop, info })
        }
    }
}

fn spatial(
    op: SpatialOp,
    property: &Option<Expr>,
    operand: &SpatialOperand,
    distance: &Option<(f64, String)>,
    ctx: &SqlContext,
) -> T {
    use geo::BoundingRect;
    let SpatialOperand::Geometry(lit) = operand else {
        return T::None;
    };
    let Some(col) = geometry_column(property, ctx) else {
        return T::None;
    };
    let srid = ctx.feature_type.srid;
    let dist = distance
        .as_ref()
        .map(|(d, u)| crate::wfs::filter::distance_in_crs(*d, u, srid));
    let name = quote_ident(&col.prop.column);
    match ctx.dialect {
        Dialect::Postgres => {
            let wkb = || SqlValue::Bytes(crate::wfs::wkb::encode_wkb(&lit.geometry));
            let geom_sql = format!("ST_GeomFromWKB(?, {srid})");
            let f = |func: &str| T::Exact(expr(format!("{func}({name}, {geom_sql})"), vec![wkb()]));
            match op {
                SpatialOp::BBox => {
                    let Some(r) = lit.geometry.bounding_rect() else {
                        return T::None;
                    };
                    T::Exact(expr(
                        format!("ST_Intersects({name}, ST_MakeEnvelope(?, ?, ?, ?, {srid}))"),
                        vec![
                            SqlValue::Float(r.min().x),
                            SqlValue::Float(r.min().y),
                            SqlValue::Float(r.max().x),
                            SqlValue::Float(r.max().y),
                        ],
                    ))
                }
                SpatialOp::Intersects => f("ST_Intersects"),
                SpatialOp::Disjoint => f("ST_Disjoint"),
                SpatialOp::Equals => f("ST_Equals"),
                SpatialOp::Touches => f("ST_Touches"),
                SpatialOp::Within => f("ST_Within"),
                SpatialOp::Overlaps => f("ST_Overlaps"),
                SpatialOp::Crosses => f("ST_Crosses"),
                SpatialOp::Contains => f("ST_Contains"),
                SpatialOp::DWithin | SpatialOp::Beyond => {
                    let not = if op == SpatialOp::Beyond { "NOT " } else { "" };
                    T::Exact(expr(
                        format!("{not}ST_DWithin({name}, {geom_sql}, ?)"),
                        vec![wkb(), SqlValue::Float(dist.unwrap_or(0.0))],
                    ))
                }
            }
        }
        Dialect::Sqlite => {
            // rtree prefilter for operators implying envelope intersection
            let (Some(rtree), Some(rowid)) = (&col.info.rtree, ctx.rowid_column) else {
                return T::None;
            };
            let Some(r) = lit.geometry.bounding_rect() else {
                return T::None;
            };
            let grow = match op {
                SpatialOp::Disjoint | SpatialOp::Beyond => return T::None,
                SpatialOp::DWithin => dist.unwrap_or(0.0),
                _ => 0.0,
            };
            T::Prefilter(expr(
                format!(
                    "{} IN (SELECT id FROM {} WHERE minx <= ? AND maxx >= ? AND miny <= ? AND maxy >= ?)",
                    quote_ident(rowid),
                    quote_ident(rtree)
                ),
                vec![
                    SqlValue::Float(r.max().x + grow),
                    SqlValue::Float(r.min().x - grow),
                    SqlValue::Float(r.max().y + grow),
                    SqlValue::Float(r.min().y - grow),
                ],
            ))
        }
        Dialect::ClickHouse => {
            let Some((x, y)) = &col.info.coords else {
                return T::None;
            };
            let Some(r) = lit.geometry.bounding_rect() else {
                return T::None;
            };
            let grow = match op {
                SpatialOp::Disjoint | SpatialOp::Beyond => return T::None,
                SpatialOp::DWithin => dist.unwrap_or(0.0),
                _ => 0.0,
            };
            let e = expr(
                format!("({x} BETWEEN ? AND ? AND {y} BETWEEN ? AND ?)"),
                vec![
                    SqlValue::Float(r.min().x - grow),
                    SqlValue::Float(r.max().x + grow),
                    SqlValue::Float(r.min().y - grow),
                    SqlValue::Float(r.max().y + grow),
                ],
            );
            // a point intersects an envelope exactly when its coordinates are in range
            if op == SpatialOp::BBox {
                T::Exact(e)
            } else {
                T::Prefilter(e)
            }
        }
    }
}

fn resource_ids(ids: &[ResourceId], ctx: &SqlContext) -> T {
    let type_prefix = format!("{}.", ctx.feature_type.name.local);
    // only current versions exist
    let current: Vec<&ResourceId> = ids
        .iter()
        .filter(|rid| match &rid.version {
            None
            | Some(VersionSpec::First)
            | Some(VersionSpec::Last)
            | Some(VersionSpec::All)
            | Some(VersionSpec::Timestamp(_)) => true,
            Some(VersionSpec::Index(n)) => *n == 1,
            Some(VersionSpec::Previous) | Some(VersionSpec::Next) => false,
        })
        .collect();
    match ctx.id {
        IdMapping::Pk { column, db_type } => {
            let keys: Vec<String> = current
                .iter()
                .filter_map(|rid| rid.rid.strip_prefix(&type_prefix))
                .map(str::to_string)
                .collect();
            if keys.is_empty() {
                return T::Exact(expr("FALSE", vec![]));
            }
            let name = quote_ident(column);
            match ctx.dialect {
                Dialect::Postgres => T::Exact(expr(
                    format!("{name} = ANY(?::{db_type}[])"),
                    vec![SqlValue::TextArray(keys)],
                )),
                _ => {
                    let values: Vec<SqlValue> = keys
                        .into_iter()
                        .map(|k| {
                            k.parse::<i64>()
                                .map(SqlValue::Int)
                                .unwrap_or(SqlValue::Text(k))
                        })
                        .collect();
                    T::Exact(expr(
                        format!("{name} IN ({})", vec!["?"; values.len()].join(", ")),
                        values,
                    ))
                }
            }
        }
        IdMapping::GmlId { column, pk } => {
            if current.is_empty() {
                return T::Exact(expr("FALSE", vec![]));
            }
            let ids: Vec<String> = current.iter().map(|rid| rid.rid.clone()).collect();
            let pks: Vec<i64> = current
                .iter()
                .filter_map(|rid| rid.rid.strip_prefix(&type_prefix))
                .filter_map(|k| k.parse().ok())
                .collect();
            let (name, pk) = (quote_ident(column), quote_ident(pk));
            let mut binds: Vec<SqlValue> = ids.iter().cloned().map(SqlValue::Text).collect();
            let mut sql = format!("{name} IN ({})", vec!["?"; ids.len()].join(", "));
            if !pks.is_empty() {
                sql = format!(
                    "{sql} OR ({name} IS NULL AND {pk} IN ({}))",
                    vec!["?"; pks.len()].join(", ")
                );
                binds.extend(pks.into_iter().map(SqlValue::Int));
            }
            if ctx.dialect == Dialect::Postgres {
                // texts compare with text columns
            }
            T::Exact(expr(sql, binds))
        }
        IdMapping::None => T::None,
    }
}

fn temporal(op: TemporalOp, e: &Expr, operand: &TemporalOperand, ctx: &SqlContext) -> T {
    use crate::wfs::filter::temporal::parse_datetime;
    if ctx.dialect == Dialect::Sqlite {
        return T::None;
    }
    let Some(col) = column(e, ctx) else {
        return T::None;
    };
    if !matches!(col.prop.value_type, ValueType::Date | ValueType::DateTime) {
        return T::None;
    }
    let TemporalOperand::Literal(l) = operand else {
        return T::None;
    };
    let (b1, b2) = match l {
        TemporalLiteral::Instant(t) => match parse_datetime(t) {
            Some(t) => (t, t),
            None => return T::None,
        },
        TemporalLiteral::Period { begin, end } => {
            match (parse_datetime(begin), parse_datetime(end)) {
                (Some(b), Some(e)) => (b, e),
                _ => return T::None,
            }
        }
    };
    let name = quote_ident(&col.prop.column);
    let (ph, ch) = match ctx.dialect {
        Dialect::ClickHouse if col.prop.value_type == ValueType::Date => {
            ("toDate(?)".to_string(), true)
        }
        Dialect::ClickHouse => ("parseDateTime64BestEffort(?, 6, 'UTC')".to_string(), true),
        _ => (format!("?::{}", col.info.db_type), false),
    };
    let v = |t: chrono::DateTime<chrono::Utc>| {
        if ch {
            // DateTime64 range: clamp open period bounds
            let lo = chrono::DateTime::from_timestamp(-2_208_988_800, 0).expect("1900"); // 1900-01-01
            let hi = chrono::DateTime::from_timestamp(10_413_791_999, 0).expect("2299"); // 2299-12-31T23:59:59
            SqlValue::Text(t.clamp(lo, hi).format("%Y-%m-%d %H:%M:%S%.f").to_string())
        } else {
            SqlValue::Text(t.to_rfc3339())
        }
    };
    let c = |cmp: &str, t| (format!("{name} {cmp} {ph}"), v(t));
    // property values are instants (a1 = a2 = col)
    let conds: Vec<(String, SqlValue)> = match op {
        TemporalOp::Before => vec![c("<", b1)],
        TemporalOp::After => vec![c(">", b2)],
        TemporalOp::Begins => vec![c("=", b1), c("<", b2)],
        TemporalOp::Ends => vec![c("=", b2), c(">", b1)],
        TemporalOp::During => vec![c(">", b1), c("<", b2)],
        TemporalOp::TEquals => vec![c("=", b1), c("=", b2)],
        TemporalOp::AnyInteracts => vec![c("<=", b2), c(">=", b1)],
        TemporalOp::TContains => vec![c("<", b1), c(">", b2)],
        TemporalOp::BegunBy => vec![c("=", b1), c(">", b2)],
        TemporalOp::EndedBy => vec![c("=", b2), c("<", b1)],
        TemporalOp::TOverlaps => vec![c("<", b1), c(">", b1), c("<", b2)],
        TemporalOp::OverlappedBy => vec![c(">", b1), c("<", b2), c(">", b2)],
        TemporalOp::Meets | TemporalOp::MetBy => return T::Exact(expr("FALSE", vec![])),
    };
    if conds.len() == 1 {
        let (sql, b) = conds.into_iter().next().expect("one");
        return T::Exact(expr(sql, vec![b]));
    }
    let (sqls, binds): (Vec<String>, Vec<SqlValue>) = conds.into_iter().unzip();
    T::Exact(expr(format!("({})", sqls.join(" AND ")), binds))
}

fn tr(filter: &Filter, ctx: &SqlContext) -> T {
    // predicates on properties which are never stored
    match filter {
        Filter::IsNull(e) if absent(e, ctx).is_some() => return T::Exact(expr("TRUE", vec![])),
        Filter::IsNil { expr: e, .. } if absent(e, ctx).is_some() => {
            return T::Exact(expr("FALSE", vec![]))
        }
        Filter::Comparison {
            left, right, op, ..
        } if (absent(left, ctx).is_some() || absent(right, ctx).is_some())
            && *op != ComparisonOp::NotEqualTo =>
        {
            return T::Exact(expr("FALSE", vec![]))
        }
        Filter::Like { expr: e, .. } | Filter::Between { expr: e, .. }
            if absent(e, ctx).is_some() =>
        {
            return T::Exact(expr("FALSE", vec![]))
        }
        _ => {}
    }
    match filter {
        Filter::Constant(b) => T::Exact(expr(if *b { "TRUE" } else { "FALSE" }, vec![])),
        Filter::Comparison {
            op,
            left,
            right,
            match_case,
            ..
        } => comparison(*op, left, right, *match_case, ctx),
        Filter::Like {
            expr: e,
            pattern,
            wild_card,
            single_char,
            escape_char,
            match_case,
        } => like(
            e,
            pattern,
            *wild_card,
            *single_char,
            *escape_char,
            *match_case,
            ctx,
        ),
        Filter::IsNull(e) => match column(e, ctx) {
            Some(col) if !(col.prop.nillable && col.prop.min_occurs > 0) => T::Exact(expr(
                format!("{} IS NULL", quote_ident(&col.prop.column)),
                vec![],
            )),
            Some(_) => T::Exact(expr("FALSE", vec![])),
            None => T::None,
        },
        Filter::IsNil { expr: e, .. } => match column(e, ctx) {
            Some(col) if col.prop.nillable && col.prop.min_occurs > 0 => T::Exact(expr(
                format!("{} IS NULL", quote_ident(&col.prop.column)),
                vec![],
            )),
            Some(_) => T::Exact(expr("FALSE", vec![])),
            None => T::None,
        },
        Filter::Between {
            expr: e,
            lower,
            upper,
        } => {
            let (Some(col), Some(lo), Some(hi)) = (column(e, ctx), literal(lower), literal(upper))
            else {
                return T::None;
            };
            match (param(lo, &col, ctx), param(hi, &col, ctx)) {
                (Some((p1, v1)), Some((p2, v2))) => T::Exact(expr(
                    format!("{} BETWEEN {p1} AND {p2}", quote_ident(&col.prop.column)),
                    vec![v1, v2],
                )),
                _ => T::None,
            }
        }
        Filter::Spatial {
            op,
            property,
            operand,
            distance,
        } => spatial(*op, property, operand, distance, ctx),
        Filter::Temporal {
            op,
            expr: e,
            operand,
        } => temporal(*op, e, operand, ctx),
        Filter::ResourceIds(ids) => resource_ids(ids, ctx),
        Filter::And(parts) => {
            let mut exact = true;
            let mut sqls = Vec::new();
            for p in parts {
                match tr(p, ctx) {
                    T::Exact(e) => sqls.push(e),
                    T::Prefilter(e) => {
                        exact = false;
                        sqls.push(e)
                    }
                    T::None => return T::None,
                }
            }
            match join(sqls, " AND ") {
                Some(e) if exact => T::Exact(e),
                Some(e) => T::Prefilter(e),
                None => T::None,
            }
        }
        Filter::Or(parts) => {
            if let Some(e) = equality_list(parts, ctx) {
                return T::Exact(e);
            }
            let mut exact = true;
            let mut sqls = Vec::new();
            for p in parts {
                match tr(p, ctx) {
                    T::Exact(e) => sqls.push(e),
                    T::Prefilter(e) => {
                        exact = false;
                        sqls.push(e)
                    }
                    T::None => return T::None,
                }
            }
            match join(sqls, " OR ") {
                Some(e) if exact => T::Exact(e),
                Some(e) => T::Prefilter(e),
                None => T::None,
            }
        }
        Filter::Not(inner) => match tr(inner, ctx) {
            T::Exact(e) => T::Exact(SqlExpr {
                sql: format!("NOT COALESCE(({}), FALSE)", e.sql),
                binds: e.binds,
            }),
            _ => T::None,
        },
        Filter::Function(_) => T::None,
    }
}

/// Renumber placeholders: `?` for SQLite, `$n` for Postgres starting at `first`
pub fn render_placeholders(sql: &str, dialect: Dialect, first: usize) -> String {
    match dialect {
        Dialect::Sqlite | Dialect::ClickHouse => sql.to_string(),
        Dialect::Postgres => {
            let mut out = String::with_capacity(sql.len() + 8);
            let mut n = first;
            let mut in_str = false;
            for c in sql.chars() {
                if c == '\'' {
                    in_str = !in_str;
                }
                if c == '?' && !in_str {
                    out.push_str(&format!("${n}"));
                    n += 1;
                } else {
                    out.push(c);
                }
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::point;

    fn qn(local: &str) -> QName {
        QName::new("http://example.com/app", "app", local)
    }

    fn feature_type() -> FeatureTypeDef {
        let prop = |local: &str, vt: ValueType| PropertyDef::simple(qn(local), local, vt);
        FeatureTypeDef {
            name: qn("images"),
            title: None,
            abstract_: None,
            keywords: vec![],
            properties: vec![
                prop("image_id", ValueType::String),
                prop("count", ValueType::Integer),
                prop("score", ValueType::Double),
                prop("acquired", ValueType::DateTime),
                prop("geom", ValueType::Geometry(GeomType::Polygon)),
                PropertyDef {
                    max_occurs: None,
                    ..prop("tags", ValueType::String)
                },
            ],
            srid: 4326,
            wgs84_bbox: None,
            schema_xsd: None,
            supertypes: vec![],
        }
    }

    fn pg_columns() -> Vec<Option<ColumnInfo>> {
        let c = |t: &str| {
            Some(ColumnInfo {
                db_type: t.to_string(),
                rtree: None,
                coords: None,
            })
        };
        vec![
            c("uuid"),
            c("integer"),
            c("double precision"),
            c("timestamp with time zone"),
            c("geometry"),
            c("text"),
        ]
    }

    fn translate_pg(xml_body: &str) -> Translation {
        let ft = feature_type();
        let cols = pg_columns();
        let id = IdMapping::Pk {
            column: "fid".to_string(),
            db_type: "bigint".to_string(),
        };
        let ctx = SqlContext {
            dialect: Dialect::Postgres,
            feature_type: &ft,
            columns: &cols,
            id: &id,
            rowid_column: None,
        };
        translate(&parse(xml_body, &ft), &ctx)
    }

    fn translate_sqlite(xml_body: &str) -> Translation {
        let ft = feature_type();
        let c = |rtree: Option<&str>| {
            Some(ColumnInfo {
                db_type: String::new(),
                rtree: rtree.map(str::to_string),
                coords: None,
            })
        };
        let cols = vec![
            c(None),
            c(None),
            c(None),
            c(None),
            c(Some("rtree_images_geom")),
            c(None),
        ];
        let id = IdMapping::Pk {
            column: "fid".to_string(),
            db_type: "INTEGER".to_string(),
        };
        let ctx = SqlContext {
            dialect: Dialect::Sqlite,
            feature_type: &ft,
            columns: &cols,
            id: &id,
            rowid_column: Some("fid"),
        };
        translate(&parse(xml_body, &ft), &ctx)
    }

    fn parse(body: &str, ft: &FeatureTypeDef) -> Filter {
        let xml = format!(
            r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:app="http://example.com/app">{body}</fes:Filter>"#
        );
        let f = parse_filter_str(&xml, &ParseContext::new(FilterVersion::V200)).unwrap();
        prepare(&f, &[ft]).unwrap()
    }

    fn eq(prop: &str, lit: &str) -> String {
        format!("<fes:PropertyIsEqualTo><fes:ValueReference>app:{prop}</fes:ValueReference><fes:Literal>{lit}</fes:Literal></fes:PropertyIsEqualTo>")
    }

    fn sql(t: &Translation) -> &str {
        &t.sql.as_ref().expect("pushed down").sql
    }

    #[test]
    fn indexed_column_equality_is_pushed_with_bare_column() {
        let t = translate_pg(&eq("image_id", "a1b2"));
        assert_eq!(sql(&t), r#""image_id" = ?::uuid"#);
        assert_eq!(t.sql.unwrap().binds, vec![SqlValue::Text("a1b2".into())]);
        assert_eq!(t.residual, None);
        // literal first (FES 2.0 allows either order)
        let t = translate_pg(
            r#"<fes:PropertyIsEqualTo><fes:Literal>a1b2</fes:Literal><fes:ValueReference>app:image_id</fes:ValueReference></fes:PropertyIsEqualTo>"#,
        );
        assert_eq!(sql(&t), r#""image_id" = ?::uuid"#);
        assert_eq!(t.residual, None);
    }

    #[test]
    fn or_of_equalities_becomes_any() {
        let t = translate_pg(&format!(
            "<fes:Or>{}{}{}</fes:Or>",
            eq("image_id", "a"),
            eq("image_id", "b"),
            eq("image_id", "c")
        ));
        assert_eq!(sql(&t), r#""image_id" = ANY(?::uuid[])"#);
        assert_eq!(
            t.sql.unwrap().binds,
            vec![SqlValue::TextArray(vec![
                "a".into(),
                "b".into(),
                "c".into()
            ])]
        );
        assert_eq!(t.residual, None);
    }

    #[test]
    fn and_splits_pushable_parts() {
        // nested path into a multi-valued property can not be translated
        let t = translate_pg(&format!(
            "<fes:And>{}<fes:PropertyIsEqualTo><fes:ValueReference>app:tags</fes:ValueReference><fes:Literal>x</fes:Literal></fes:PropertyIsEqualTo></fes:And>",
            eq("image_id", "a1")
        ));
        assert_eq!(sql(&t), r#""image_id" = ?::uuid"#);
        assert!(matches!(t.residual, Some(Filter::Comparison { .. })));
    }

    #[test]
    fn comparisons_between_like_null() {
        let t = translate_pg(
            r#"<fes:PropertyIsGreaterThan><fes:ValueReference>app:count</fes:ValueReference><fes:Literal>5</fes:Literal></fes:PropertyIsGreaterThan>"#,
        );
        assert_eq!(sql(&t), r#""count" > ?::integer"#);
        // fractional literal on integer column: compare numerically
        let t = translate_pg(
            r#"<fes:PropertyIsLessThan><fes:ValueReference>app:count</fes:ValueReference><fes:Literal>5.5</fes:Literal></fes:PropertyIsLessThan>"#,
        );
        assert_eq!(sql(&t), r#""count" < ?::double precision"#);
        let t = translate_pg(
            r#"<fes:PropertyIsBetween><fes:ValueReference>app:score</fes:ValueReference><fes:LowerBoundary><fes:Literal>1</fes:Literal></fes:LowerBoundary><fes:UpperBoundary><fes:Literal>2</fes:Literal></fes:UpperBoundary></fes:PropertyIsBetween>"#,
        );
        assert_eq!(
            sql(&t),
            r#""score" BETWEEN ?::double precision AND ?::double precision"#
        );
        let t = translate_pg(
            r#"<fes:PropertyIsLike wildCard="*" singleChar="?" escapeChar="\"><fes:ValueReference>app:image_id</fes:ValueReference><fes:Literal>ab*_c?</fes:Literal></fes:PropertyIsLike>"#,
        );
        assert_eq!(sql(&t), r#""image_id"::text LIKE ? ESCAPE '\'"#);
        assert_eq!(
            t.sql.unwrap().binds,
            vec![SqlValue::Text("ab%\\_c_".into())]
        );
        let t = translate_pg(
            r#"<fes:PropertyIsLike wildCard="*" singleChar="?" escapeChar="\" matchCase="false"><fes:ValueReference>app:image_id</fes:ValueReference><fes:Literal>ab*</fes:Literal></fes:PropertyIsLike>"#,
        );
        assert_eq!(sql(&t), r#""image_id"::text ILIKE ? ESCAPE '\'"#);
        let t = translate_pg(
            r#"<fes:PropertyIsNull><fes:ValueReference>app:count</fes:ValueReference></fes:PropertyIsNull>"#,
        );
        assert_eq!(sql(&t), r#""count" IS NULL"#);
        // matchCase=false on non-text column does not wrap the column
        let t = translate_pg(
            r#"<fes:PropertyIsEqualTo matchCase="false"><fes:ValueReference>app:count</fes:ValueReference><fes:Literal>5</fes:Literal></fes:PropertyIsEqualTo>"#,
        );
        assert_eq!(sql(&t), r#""count" = ?::integer"#);
    }

    #[test]
    fn not_handles_nulls_like_evaluator() {
        let t = translate_pg(&format!("<fes:Not>{}</fes:Not>", eq("count", "5")));
        assert_eq!(sql(&t), r#"NOT COALESCE(("count" = ?::integer), FALSE)"#);
        assert_eq!(t.residual, None);
    }

    #[test]
    fn spatial_postgis() {
        let t = translate_pg(
            r#"<fes:BBOX><fes:ValueReference>app:geom</fes:ValueReference><gml:Envelope srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>40 5</gml:lowerCorner><gml:upperCorner>50 10</gml:upperCorner></gml:Envelope></fes:BBOX>"#,
        );
        assert_eq!(
            sql(&t),
            r#"ST_Intersects("geom", ST_MakeEnvelope(?, ?, ?, ?, 4326))"#
        );
        assert_eq!(
            t.sql.unwrap().binds,
            vec![
                SqlValue::Float(5.0),
                SqlValue::Float(40.0),
                SqlValue::Float(10.0),
                SqlValue::Float(50.0)
            ]
        );
        assert_eq!(t.residual, None);
        let t = translate_pg(
            r#"<fes:Within><fes:ValueReference>app:geom</fes:ValueReference><gml:Point gml:id="p" srsName="urn:ogc:def:crs:EPSG::4326"><gml:pos>47 8</gml:pos></gml:Point></fes:Within>"#,
        );
        assert_eq!(sql(&t), r#"ST_Within("geom", ST_GeomFromWKB(?, 4326))"#);
        let t = translate_pg(
            r#"<fes:DWithin><fes:ValueReference>app:geom</fes:ValueReference><gml:Point gml:id="p" srsName="urn:ogc:def:crs:EPSG::4326"><gml:pos>47 8</gml:pos></gml:Point><fes:Distance uom="deg">0.5</fes:Distance></fes:DWithin>"#,
        );
        assert_eq!(sql(&t), r#"ST_DWithin("geom", ST_GeomFromWKB(?, 4326), ?)"#);
        let t = translate_pg(
            r#"<fes:Beyond><fes:ValueReference>app:geom</fes:ValueReference><gml:Point gml:id="p" srsName="urn:ogc:def:crs:EPSG::4326"><gml:pos>47 8</gml:pos></gml:Point><fes:Distance uom="deg">0.5</fes:Distance></fes:Beyond>"#,
        );
        assert_eq!(
            sql(&t),
            r#"NOT ST_DWithin("geom", ST_GeomFromWKB(?, 4326), ?)"#
        );
        // BBOX without property: default geometry
        let t = translate_pg(
            r#"<fes:BBOX><gml:Envelope srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>40 5</gml:lowerCorner><gml:upperCorner>50 10</gml:upperCorner></gml:Envelope></fes:BBOX>"#,
        );
        assert_eq!(
            sql(&t),
            r#"ST_Intersects("geom", ST_MakeEnvelope(?, ?, ?, ?, 4326))"#
        );
    }

    #[test]
    fn spatial_geopackage_uses_rtree_prefilter() {
        let t = translate_sqlite(
            r#"<fes:BBOX><fes:ValueReference>app:geom</fes:ValueReference><gml:Envelope srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>40 5</gml:lowerCorner><gml:upperCorner>50 10</gml:upperCorner></gml:Envelope></fes:BBOX>"#,
        );
        assert_eq!(
            sql(&t),
            r#""fid" IN (SELECT id FROM "rtree_images_geom" WHERE minx <= ? AND maxx >= ? AND miny <= ? AND maxy >= ?)"#
        );
        assert_eq!(
            t.sql.unwrap().binds,
            vec![
                SqlValue::Float(10.0),
                SqlValue::Float(5.0),
                SqlValue::Float(50.0),
                SqlValue::Float(40.0)
            ]
        );
        // exact geometry test remains
        assert!(matches!(t.residual, Some(Filter::Spatial { .. })));
        // Disjoint can not use the index
        let t = translate_sqlite(
            r#"<fes:Disjoint><fes:ValueReference>app:geom</fes:ValueReference><gml:Point gml:id="p" srsName="urn:ogc:def:crs:EPSG::4326"><gml:pos>47 8</gml:pos></gml:Point></fes:Disjoint>"#,
        );
        assert_eq!(t.sql, None);
        assert!(t.residual.is_some());
        // plain comparisons are exact in SQLite too
        let t = translate_sqlite(&eq("count", "5"));
        assert_eq!(sql(&t), r#""count" = ?"#);
        assert_eq!(t.sql.unwrap().binds, vec![SqlValue::Int(5)]);
        assert_eq!(t.residual, None);
    }

    #[test]
    fn resource_ids() {
        let t = translate_pg(
            r#"<fes:ResourceId rid="images.12"/><fes:ResourceId rid="images.13"/><fes:ResourceId rid="other.1"/>"#,
        );
        assert_eq!(sql(&t), r#""fid" = ANY(?::bigint[])"#);
        assert_eq!(
            t.sql.unwrap().binds,
            vec![SqlValue::TextArray(vec!["12".into(), "13".into()])]
        );
        assert_eq!(t.residual, None);
        // no matching id: constant false
        let t = translate_pg(r#"<fes:ResourceId rid="other.1"/>"#);
        assert_eq!(sql(&t), "FALSE");
        // versions without history
        let t = translate_pg(r#"<fes:ResourceId rid="images.12" version="PREVIOUS"/>"#);
        assert_eq!(sql(&t), "FALSE");
    }

    #[test]
    fn temporal_postgres() {
        let t = translate_pg(
            r#"<fes:During><fes:ValueReference>app:acquired</fes:ValueReference><gml:TimePeriod gml:id="t"><gml:beginPosition>2020-01-01T00:00:00Z</gml:beginPosition><gml:endPosition>2021-01-01T00:00:00Z</gml:endPosition></gml:TimePeriod></fes:During>"#,
        );
        assert_eq!(
            sql(&t),
            r#"("acquired" > ?::timestamp with time zone AND "acquired" < ?::timestamp with time zone)"#
        );
        assert_eq!(t.residual, None);
        let t = translate_pg(
            r#"<fes:After><fes:ValueReference>app:acquired</fes:ValueReference><gml:TimeInstant gml:id="t"><gml:timePosition>2020-01-01T00:00:00+09:00</gml:timePosition></gml:TimeInstant></fes:After>"#,
        );
        assert_eq!(sql(&t), r#""acquired" > ?::timestamp with time zone"#);
        assert_eq!(
            t.sql.unwrap().binds,
            vec![SqlValue::Text("2019-12-31T15:00:00+00:00".into())]
        );
    }

    #[test]
    fn untranslatable_parts() {
        // functions are evaluated in Rust
        let t = translate_pg(
            r#"<fes:PropertyIsEqualTo><fes:Function name="strToUpperCase"><fes:ValueReference>app:image_id</fes:ValueReference></fes:Function><fes:Literal>A</fes:Literal></fes:PropertyIsEqualTo>"#,
        );
        assert_eq!(t.sql, None);
        assert!(t.residual.is_some());
        // Or with untranslatable branch can not be split
        let t = translate_pg(&format!("<fes:Or>{}<fes:PropertyIsEqualTo><fes:ValueReference>app:tags</fes:ValueReference><fes:Literal>x</fes:Literal></fes:PropertyIsEqualTo></fes:Or>", eq("image_id", "a")));
        assert_eq!(t.sql, None);
    }

    #[test]
    fn placeholders() {
        assert_eq!(
            render_placeholders("a = ? AND b = '?' AND c = ?", Dialect::Postgres, 3),
            "a = $3 AND b = '?' AND c = $4"
        );
        assert_eq!(render_placeholders("a = ?", Dialect::Sqlite, 1), "a = ?");
        let _ = point!(x: 1., y: 2.);
    }
}
