//! OGC Filter Encoding 1.0 / 1.1 and FES 2.0: parsing and evaluation.

mod eval;
mod functions;
mod parse;
pub mod path;
pub mod temporal;

pub use eval::*;
pub use functions::{function_names, FunctionSignature, FUNCTIONS};
pub use parse::*;
pub use path::{PathStep, PropertyPath};

use geo::Geometry;

/// Filter encoding version, determines namespaces and some element names
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterVersion {
    /// Filter Encoding 1.0 (WFS 1.0)
    V100,
    /// Filter Encoding 1.1 (WFS 1.1)
    V110,
    /// FES 2.0 (WFS 2.0)
    V200,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComparisonOp {
    EqualTo,
    NotEqualTo,
    LessThan,
    GreaterThan,
    LessThanOrEqualTo,
    GreaterThanOrEqualTo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MatchAction {
    #[default]
    Any,
    All,
    One,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpatialOp {
    BBox,
    Equals,
    Disjoint,
    Touches,
    Within,
    Overlaps,
    Crosses,
    Intersects,
    Contains,
    DWithin,
    Beyond,
}

impl SpatialOp {
    pub const ALL: [SpatialOp; 11] = [
        SpatialOp::BBox,
        SpatialOp::Equals,
        SpatialOp::Disjoint,
        SpatialOp::Touches,
        SpatialOp::Within,
        SpatialOp::Overlaps,
        SpatialOp::Crosses,
        SpatialOp::Intersects,
        SpatialOp::Contains,
        SpatialOp::DWithin,
        SpatialOp::Beyond,
    ];
    pub fn name(&self) -> &'static str {
        match self {
            SpatialOp::BBox => "BBOX",
            SpatialOp::Equals => "Equals",
            SpatialOp::Disjoint => "Disjoint",
            SpatialOp::Touches => "Touches",
            SpatialOp::Within => "Within",
            SpatialOp::Overlaps => "Overlaps",
            SpatialOp::Crosses => "Crosses",
            SpatialOp::Intersects => "Intersects",
            SpatialOp::Contains => "Contains",
            SpatialOp::DWithin => "DWithin",
            SpatialOp::Beyond => "Beyond",
        }
    }
    fn from_name(name: &str) -> Option<SpatialOp> {
        SpatialOp::ALL
            .into_iter()
            .find(|op| op.name().eq_ignore_ascii_case(name))
            .or(if name == "Intersect" {
                Some(SpatialOp::Intersects)
            } else {
                None
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemporalOp {
    After,
    Before,
    Begins,
    BegunBy,
    TContains,
    During,
    EndedBy,
    Ends,
    TEquals,
    Meets,
    MetBy,
    TOverlaps,
    OverlappedBy,
    AnyInteracts,
}

impl TemporalOp {
    pub const ALL: [TemporalOp; 14] = [
        TemporalOp::After,
        TemporalOp::Before,
        TemporalOp::Begins,
        TemporalOp::BegunBy,
        TemporalOp::TContains,
        TemporalOp::During,
        TemporalOp::EndedBy,
        TemporalOp::Ends,
        TemporalOp::TEquals,
        TemporalOp::Meets,
        TemporalOp::MetBy,
        TemporalOp::TOverlaps,
        TemporalOp::OverlappedBy,
        TemporalOp::AnyInteracts,
    ];
    pub fn name(&self) -> &'static str {
        match self {
            TemporalOp::After => "After",
            TemporalOp::Before => "Before",
            TemporalOp::Begins => "Begins",
            TemporalOp::BegunBy => "BegunBy",
            TemporalOp::TContains => "TContains",
            TemporalOp::During => "During",
            TemporalOp::EndedBy => "EndedBy",
            TemporalOp::Ends => "Ends",
            TemporalOp::TEquals => "TEquals",
            TemporalOp::Meets => "Meets",
            TemporalOp::MetBy => "MetBy",
            TemporalOp::TOverlaps => "TOverlaps",
            TemporalOp::OverlappedBy => "OverlappedBy",
            TemporalOp::AnyInteracts => "AnyInteracts",
        }
    }
    fn from_name(name: &str) -> Option<TemporalOp> {
        TemporalOp::ALL.into_iter().find(|op| op.name() == name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
}

/// Literal geometry with its CRS (EPSG code) and axis order already normalized to x/y
#[derive(Clone, Debug, PartialEq)]
pub struct GeometryLiteral {
    pub geometry: Geometry<f64>,
    /// EPSG code of srsName, None if not given (native CRS assumed)
    pub srid: Option<u16>,
    /// Original srsName
    pub srs_name: Option<String>,
}

/// Expression
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Property(PropertyPath),
    Literal(String),
    /// Literal with XML content (e.g. a geometry or envelope)
    GeometryLiteral(GeometryLiteral),
    Function {
        name: String,
        args: Vec<Expr>,
    },
    Arith {
        op: ArithOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
}

/// Temporal operand literal
#[derive(Clone, Debug, PartialEq)]
pub enum TemporalLiteral {
    Instant(String),
    Period { begin: String, end: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum TemporalOperand {
    Literal(TemporalLiteral),
    Expr(Expr),
}

/// Version selector of a resource id (FES 2.0 version navigation)
#[derive(Clone, Debug, PartialEq)]
pub enum VersionSpec {
    First,
    Last,
    Previous,
    Next,
    All,
    Index(u32),
    Timestamp(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResourceId {
    pub rid: String,
    pub version: Option<VersionSpec>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
}

/// Spatial operand: literal geometry or (for joins) a property of another type
#[derive(Clone, Debug, PartialEq)]
pub enum SpatialOperand {
    Geometry(GeometryLiteral),
    Expr(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Filter {
    And(Vec<Filter>),
    Or(Vec<Filter>),
    Not(Box<Filter>),
    Comparison {
        op: ComparisonOp,
        left: Expr,
        right: Expr,
        match_case: bool,
        match_action: MatchAction,
    },
    Like {
        expr: Expr,
        pattern: String,
        wild_card: char,
        single_char: char,
        escape_char: char,
        match_case: bool,
    },
    IsNull(Expr),
    IsNil {
        expr: Expr,
        nil_reason: Option<String>,
    },
    Between {
        expr: Expr,
        lower: Expr,
        upper: Expr,
    },
    Spatial {
        op: SpatialOp,
        /// None: any geometry property (BBOX without ValueReference)
        property: Option<Expr>,
        operand: SpatialOperand,
        /// Distance in the given unit (DWithin, Beyond)
        distance: Option<(f64, String)>,
    },
    Temporal {
        op: TemporalOp,
        expr: Expr,
        operand: TemporalOperand,
    },
    ResourceIds(Vec<ResourceId>),
    /// Boolean valued function used as predicate (extension operator)
    Function(Expr),
    /// fes:Include / fes:Exclude style constants
    Constant(bool),
}

impl Filter {
    /// All resource ids, if the filter is an identifier filter
    pub fn resource_ids(&self) -> Option<&[ResourceId]> {
        match self {
            Filter::ResourceIds(ids) => Some(ids),
            _ => None,
        }
    }
    /// Visit all property expressions
    pub fn properties(&self) -> Vec<&PropertyPath> {
        let mut props = Vec::new();
        self.visit_exprs(&mut |e| collect_props(e, &mut props));
        props
    }
    fn visit_exprs<'a>(&'a self, f: &mut dyn FnMut(&'a Expr)) {
        match self {
            Filter::And(fs) | Filter::Or(fs) => fs.iter().for_each(|x| x.visit_exprs(f)),
            Filter::Not(x) => x.visit_exprs(f),
            Filter::Comparison { left, right, .. } => {
                f(left);
                f(right);
            }
            Filter::Like { expr, .. } | Filter::IsNull(expr) | Filter::IsNil { expr, .. } => {
                f(expr)
            }
            Filter::Between { expr, lower, upper } => {
                f(expr);
                f(lower);
                f(upper);
            }
            Filter::Spatial {
                property, operand, ..
            } => {
                if let Some(p) = property {
                    f(p);
                }
                if let SpatialOperand::Expr(e) = operand {
                    f(e);
                }
            }
            Filter::Temporal { expr, operand, .. } => {
                f(expr);
                if let TemporalOperand::Expr(e) = operand {
                    f(e);
                }
            }
            Filter::Function(e) => f(e),
            Filter::ResourceIds(_) | Filter::Constant(_) => {}
        }
    }
}

/// Rewrite all property paths of an expression
pub fn map_expr_paths(e: &Expr, f: &dyn Fn(&PropertyPath) -> PropertyPath) -> Expr {
    match e {
        Expr::Property(p) => Expr::Property(f(p)),
        Expr::Function { name, args } => Expr::Function {
            name: name.clone(),
            args: args.iter().map(|a| map_expr_paths(a, f)).collect(),
        },
        Expr::Arith { op, left, right } => Expr::Arith {
            op: *op,
            left: Box::new(map_expr_paths(left, f)),
            right: Box::new(map_expr_paths(right, f)),
        },
        other => other.clone(),
    }
}

impl Filter {
    /// Rewrite all property paths
    pub fn map_paths(&self, f: &dyn Fn(&PropertyPath) -> PropertyPath) -> Filter {
        let m = |e: &Expr| map_expr_paths(e, f);
        match self {
            Filter::And(fs) => Filter::And(fs.iter().map(|x| x.map_paths(f)).collect()),
            Filter::Or(fs) => Filter::Or(fs.iter().map(|x| x.map_paths(f)).collect()),
            Filter::Not(x) => Filter::Not(Box::new(x.map_paths(f))),
            Filter::Comparison {
                op,
                left,
                right,
                match_case,
                match_action,
            } => Filter::Comparison {
                op: *op,
                left: m(left),
                right: m(right),
                match_case: *match_case,
                match_action: *match_action,
            },
            Filter::Like {
                expr,
                pattern,
                wild_card,
                single_char,
                escape_char,
                match_case,
            } => Filter::Like {
                expr: m(expr),
                pattern: pattern.clone(),
                wild_card: *wild_card,
                single_char: *single_char,
                escape_char: *escape_char,
                match_case: *match_case,
            },
            Filter::IsNull(e) => Filter::IsNull(m(e)),
            Filter::IsNil { expr, nil_reason } => Filter::IsNil {
                expr: m(expr),
                nil_reason: nil_reason.clone(),
            },
            Filter::Between { expr, lower, upper } => Filter::Between {
                expr: m(expr),
                lower: m(lower),
                upper: m(upper),
            },
            Filter::Spatial {
                op,
                property,
                operand,
                distance,
            } => Filter::Spatial {
                op: *op,
                property: property.as_ref().map(m),
                operand: match operand {
                    SpatialOperand::Expr(e) => SpatialOperand::Expr(m(e)),
                    g => g.clone(),
                },
                distance: distance.clone(),
            },
            Filter::Temporal { op, expr, operand } => Filter::Temporal {
                op: *op,
                expr: m(expr),
                operand: match operand {
                    TemporalOperand::Expr(e) => TemporalOperand::Expr(m(e)),
                    l => l.clone(),
                },
            },
            Filter::Function(e) => Filter::Function(m(e)),
            other => other.clone(),
        }
    }
}

fn collect_props<'a>(e: &'a Expr, props: &mut Vec<&'a PropertyPath>) {
    match e {
        Expr::Property(p) => props.push(p),
        Expr::Function { args, .. } => args.iter().for_each(|a| collect_props(a, props)),
        Expr::Arith { left, right, .. } => {
            collect_props(left, props);
            collect_props(right, props);
        }
        Expr::Literal(_) | Expr::GeometryLiteral(_) => {}
    }
}

/// Filter errors with the matching OWS exception code
#[derive(thiserror::Error, Debug, PartialEq)]
pub enum FilterError {
    /// Malformed filter (OperationParsingFailed / InvalidParameterValue)
    #[error("invalid filter: {0}")]
    Parse(String),
    /// Reference to a property not defined for the feature type (InvalidParameterValue)
    #[error("unknown property `{0}`")]
    UnknownProperty(String),
    /// Invalid operand for an operator (OperationProcessingFailed)
    #[error("filter processing failed: {0}")]
    Processing(String),
    /// Unsupported CRS (InvalidParameterValue)
    #[error("unsupported CRS `{0}`")]
    Crs(String),
}
