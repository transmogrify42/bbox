//! Filter functions (FES 2.0 Functions conformance class, Filter 1.x ogc:Function).

use super::eval::Val;
use super::FilterError;
use geo::{Area, BoundingRect, Centroid, Distance, Euclidean, Intersects, Length, Relate};
use geo::{CoordsIter, Geometry};

/// Function signature for capabilities documents
#[derive(Debug, Clone, Copy)]
pub struct FunctionSignature {
    pub name: &'static str,
    /// (argument name, xsd/gml type)
    pub args: &'static [(&'static str, &'static str)],
    pub returns: &'static str,
}

const S: &str = "xs:string";
const D: &str = "xs:double";
const I: &str = "xs:integer";
const B: &str = "xs:boolean";
const G: &str = "gml:AbstractGeometryType";
const DT: &str = "xs:dateTime";

macro_rules! sig {
    ($name:expr, [$($arg:expr => $t:expr),*], $ret:expr) => {
        FunctionSignature { name: $name, args: &[$(($arg, $t)),*], returns: $ret }
    };
}

/// All supported functions
pub const FUNCTIONS: &[FunctionSignature] = &[
    // strings
    sig!("strConcat", ["a" => S, "b" => S], S),
    sig!("strToUpperCase", ["s" => S], S),
    sig!("strToLowerCase", ["s" => S], S),
    sig!("strLength", ["s" => S], I),
    sig!("strSubstring", ["s" => S, "begin" => I, "end" => I], S),
    sig!("strSubstringStart", ["s" => S, "begin" => I], S),
    sig!("strTrim", ["s" => S], S),
    sig!("strStartsWith", ["s" => S, "prefix" => S], B),
    sig!("strEndsWith", ["s" => S, "suffix" => S], B),
    sig!("strIndexOf", ["s" => S, "sub" => S], I),
    sig!("strReplace", ["s" => S, "pattern" => S, "replacement" => S, "all" => B], S),
    sig!("strEqualsIgnoreCase", ["a" => S, "b" => S], B),
    sig!("strCapitalize", ["s" => S], S),
    sig!("strAbbreviate", ["s" => S, "lower" => I, "upper" => I, "append" => S], S),
    // math
    sig!("abs", ["x" => D], D),
    sig!("ceil", ["x" => D], D),
    sig!("floor", ["x" => D], D),
    sig!("round", ["x" => D], I),
    sig!("sqrt", ["x" => D], D),
    sig!("pow", ["base" => D, "exponent" => D], D),
    sig!("exp", ["x" => D], D),
    sig!("log", ["x" => D], D),
    sig!("sin", ["x" => D], D),
    sig!("cos", ["x" => D], D),
    sig!("tan", ["x" => D], D),
    sig!("min", ["a" => D, "b" => D], D),
    sig!("max", ["a" => D, "b" => D], D),
    sig!("parseDouble", ["s" => S], D),
    sig!("parseInt", ["s" => S], I),
    sig!("parseBoolean", ["s" => S], B),
    // geometry
    sig!("area", ["geometry" => G], D),
    sig!("geomLength", ["geometry" => G], D),
    sig!("numPoints", ["geometry" => G], I),
    sig!("numGeometries", ["geometry" => G], I),
    sig!("geometryType", ["geometry" => G], S),
    sig!("isEmpty", ["geometry" => G], B),
    sig!("centroid", ["geometry" => G], G),
    sig!("envelope", ["geometry" => G], G),
    sig!("getX", ["point" => G], D),
    sig!("getY", ["point" => G], D),
    sig!("distance", ["a" => G, "b" => G], D),
    sig!("intersects", ["a" => G, "b" => G], B),
    sig!("contains", ["a" => G, "b" => G], B),
    sig!("within", ["a" => G, "b" => G], B),
    sig!("disjoint", ["a" => G, "b" => G], B),
    sig!("touches", ["a" => G, "b" => G], B),
    sig!("crosses", ["a" => G, "b" => G], B),
    sig!("overlaps", ["a" => G, "b" => G], B),
    sig!("equalsExact", ["a" => G, "b" => G], B),
    // comparison / logic
    sig!("isNull", ["value" => S], B),
    sig!("equalTo", ["a" => S, "b" => S], B),
    sig!("notEqualTo", ["a" => S, "b" => S], B),
    sig!("in", ["value" => S, "candidate1" => S, "candidate2" => S], B),
    sig!("if_then_else", ["condition" => B, "then" => S, "else" => S], S),
    sig!("not", ["value" => B], B),
    sig!("strMatches", ["string" => S, "regex" => S], B),
    sig!("random", [], D),
    // temporal
    sig!("dateParse", ["format" => S, "date" => S], DT),
    sig!("now", [], DT),
];

pub fn function_names() -> impl Iterator<Item = &'static str> {
    FUNCTIONS.iter().map(|f| f.name)
}

fn arg(args: &[Val], i: usize) -> &Val {
    args.get(i).unwrap_or(&Val::Null)
}

fn num(args: &[Val], i: usize) -> Option<f64> {
    arg(args, i).as_f64()
}

fn string(args: &[Val], i: usize) -> Option<String> {
    arg(args, i).as_string()
}

fn geom(args: &[Val], i: usize) -> Option<&Geometry<f64>> {
    arg(args, i).as_geom()
}

fn f64_val(v: Option<f64>) -> Val {
    v.map(Val::Num).unwrap_or(Val::Null)
}

fn geom_type(g: &Geometry<f64>) -> &'static str {
    match g {
        Geometry::Point(_) => "Point",
        Geometry::Line(_) | Geometry::LineString(_) => "LineString",
        Geometry::Polygon(_) | Geometry::Rect(_) | Geometry::Triangle(_) => "Polygon",
        Geometry::MultiPoint(_) => "MultiPoint",
        Geometry::MultiLineString(_) => "MultiLineString",
        Geometry::MultiPolygon(_) => "MultiPolygon",
        Geometry::GeometryCollection(_) => "GeometryCollection",
    }
}

/// Whole string regular expression match (cached compiled expressions)
fn regex_match(re: &str, s: &str) -> Result<bool, FilterError> {
    use std::cell::RefCell;
    use std::collections::HashMap;
    thread_local! {
        static CACHE: RefCell<HashMap<String, regex::Regex>> = RefCell::new(HashMap::new());
    }
    CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        if cache.len() > 256 {
            cache.clear();
        }
        if !cache.contains_key(re) {
            let compiled = regex::Regex::new(&format!("^(?:{re})$")).map_err(|e| {
                FilterError::Parse(format!("invalid regular expression `{re}`: {e}"))
            })?;
            cache.insert(re.to_string(), compiled);
        }
        Ok(cache[re].is_match(s))
    })
}

/// Uniform random number in [0, 1)
fn random() -> f64 {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    (h.finish() >> 11) as f64 / (1u64 << 53) as f64
}

/// Call a function by name
pub fn call(name: &str, args: &[Val]) -> Result<Val, FilterError> {
    let Some(sig) = FUNCTIONS.iter().find(|f| f.name.eq_ignore_ascii_case(name)) else {
        return Err(FilterError::Parse(format!("unknown function `{name}`")));
    };
    let min_args = match sig.name {
        "strReplace" => 3,
        "in" => 2,
        "strAbbreviate" => 3,
        _ => sig.args.len(),
    };
    if args.len() < min_args {
        return Err(FilterError::Parse(format!(
            "function `{}` requires {} arguments",
            sig.name,
            sig.args.len()
        )));
    }
    let binary_geom = |f: fn(&Geometry<f64>, &Geometry<f64>) -> bool| -> Val {
        match (geom(args, 0), geom(args, 1)) {
            (Some(a), Some(b)) => Val::Bool(f(a, b)),
            _ => Val::Null,
        }
    };
    Ok(match sig.name {
        "strConcat" => Val::Str(format!(
            "{}{}",
            string(args, 0).unwrap_or_default(),
            string(args, 1).unwrap_or_default()
        )),
        "strToUpperCase" => string(args, 0)
            .map(|s| Val::Str(s.to_uppercase()))
            .unwrap_or(Val::Null),
        "strToLowerCase" => string(args, 0)
            .map(|s| Val::Str(s.to_lowercase()))
            .unwrap_or(Val::Null),
        "strLength" => string(args, 0)
            .map(|s| Val::Int(s.chars().count() as i64))
            .unwrap_or(Val::Null),
        "strSubstring" => {
            let s: Vec<char> = string(args, 0).unwrap_or_default().chars().collect();
            let b = num(args, 1).unwrap_or(0.0).max(0.0) as usize;
            let e = (num(args, 2).unwrap_or(s.len() as f64).max(0.0) as usize).min(s.len());
            Val::Str(if b < e {
                s[b..e].iter().collect()
            } else {
                String::new()
            })
        }
        "strSubstringStart" => {
            let s: Vec<char> = string(args, 0).unwrap_or_default().chars().collect();
            let b = (num(args, 1).unwrap_or(0.0).max(0.0) as usize).min(s.len());
            Val::Str(s[b..].iter().collect())
        }
        "strTrim" => string(args, 0)
            .map(|s| Val::Str(s.trim().to_string()))
            .unwrap_or(Val::Null),
        "strStartsWith" => Val::Bool(
            string(args, 0)
                .unwrap_or_default()
                .starts_with(&string(args, 1).unwrap_or_default()),
        ),
        "strEndsWith" => Val::Bool(
            string(args, 0)
                .unwrap_or_default()
                .ends_with(&string(args, 1).unwrap_or_default()),
        ),
        "strIndexOf" => {
            let s = string(args, 0).unwrap_or_default();
            let sub = string(args, 1).unwrap_or_default();
            Val::Int(
                s.find(&sub)
                    .map(|byte| s[..byte].chars().count() as i64)
                    .unwrap_or(-1),
            )
        }
        "strReplace" => {
            let s = string(args, 0).unwrap_or_default();
            let pattern = string(args, 1).unwrap_or_default();
            let replacement = string(args, 2).unwrap_or_default();
            let all = arg(args, 3).as_bool().unwrap_or(true);
            Val::Str(if all {
                s.replace(&pattern, &replacement)
            } else {
                s.replacen(&pattern, &replacement, 1)
            })
        }
        "strEqualsIgnoreCase" => Val::Bool(
            string(args, 0).unwrap_or_default().to_lowercase()
                == string(args, 1).unwrap_or_default().to_lowercase(),
        ),
        "strCapitalize" => string(args, 0)
            .map(|s| {
                Val::Str(
                    s.split(' ')
                        .map(|w| {
                            let mut c = w.chars();
                            match c.next() {
                                Some(f) => {
                                    f.to_uppercase().collect::<String>()
                                        + &c.as_str().to_lowercase()
                                }
                                None => String::new(),
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            })
            .unwrap_or(Val::Null),
        "strAbbreviate" => {
            let s = string(args, 0).unwrap_or_default();
            let upper = num(args, 2).unwrap_or(s.len() as f64) as usize;
            let append = string(args, 3).unwrap_or_default();
            if s.chars().count() <= upper {
                Val::Str(s)
            } else {
                Val::Str(s.chars().take(upper).collect::<String>() + &append)
            }
        }
        "abs" => f64_val(num(args, 0).map(f64::abs)),
        "ceil" => f64_val(num(args, 0).map(f64::ceil)),
        "floor" => f64_val(num(args, 0).map(f64::floor)),
        "round" => num(args, 0)
            .map(|x| Val::Int(x.round() as i64))
            .unwrap_or(Val::Null),
        "sqrt" => f64_val(num(args, 0).map(f64::sqrt)),
        "pow" => f64_val(num(args, 0).zip(num(args, 1)).map(|(a, b)| a.powf(b))),
        "exp" => f64_val(num(args, 0).map(f64::exp)),
        "log" => f64_val(num(args, 0).map(f64::ln)),
        "sin" => f64_val(num(args, 0).map(f64::sin)),
        "cos" => f64_val(num(args, 0).map(f64::cos)),
        "tan" => f64_val(num(args, 0).map(f64::tan)),
        "min" => f64_val(num(args, 0).zip(num(args, 1)).map(|(a, b)| a.min(b))),
        "max" => f64_val(num(args, 0).zip(num(args, 1)).map(|(a, b)| a.max(b))),
        "parseDouble" => f64_val(num(args, 0)),
        "parseInt" => string(args, 0)
            .and_then(|s| s.trim().parse::<i64>().ok())
            .map(Val::Int)
            .unwrap_or(Val::Null),
        "parseBoolean" => arg(args, 0).as_bool().map(Val::Bool).unwrap_or(Val::Null),
        "area" => f64_val(geom(args, 0).map(|g| g.unsigned_area())),
        "geomLength" => f64_val(geom(args, 0).map(|g| match g {
            Geometry::LineString(l) => Euclidean.length(l),
            Geometry::MultiLineString(l) => Euclidean.length(l),
            Geometry::Polygon(p) => Euclidean.length(p.exterior()),
            Geometry::MultiPolygon(mp) => mp.iter().map(|p| Euclidean.length(p.exterior())).sum(),
            _ => 0.0,
        })),
        "numPoints" => geom(args, 0)
            .map(|g| Val::Int(g.coords_count() as i64))
            .unwrap_or(Val::Null),
        "numGeometries" => geom(args, 0)
            .map(|g| {
                Val::Int(match g {
                    Geometry::MultiPoint(m) => m.0.len(),
                    Geometry::MultiLineString(m) => m.0.len(),
                    Geometry::MultiPolygon(m) => m.0.len(),
                    Geometry::GeometryCollection(m) => m.0.len(),
                    _ => 1,
                } as i64)
            })
            .unwrap_or(Val::Null),
        "geometryType" => geom(args, 0)
            .map(|g| Val::Str(geom_type(g).to_string()))
            .unwrap_or(Val::Null),
        "isEmpty" => Val::Bool(geom(args, 0).map(|g| g.coords_count() == 0).unwrap_or(true)),
        "centroid" => geom(args, 0)
            .and_then(|g| g.centroid())
            .map(|p| Val::Geom(p.into()))
            .unwrap_or(Val::Null),
        "envelope" => geom(args, 0)
            .and_then(|g| g.bounding_rect())
            .map(|r| Val::Geom(Geometry::Polygon(r.to_polygon())))
            .unwrap_or(Val::Null),
        "getX" => match geom(args, 0) {
            Some(Geometry::Point(p)) => Val::Num(p.x()),
            _ => Val::Null,
        },
        "getY" => match geom(args, 0) {
            Some(Geometry::Point(p)) => Val::Num(p.y()),
            _ => Val::Null,
        },
        "distance" => match (geom(args, 0), geom(args, 1)) {
            (Some(a), Some(b)) => Val::Num(Euclidean.distance(a, b)),
            _ => Val::Null,
        },
        "intersects" => binary_geom(|a, b| a.intersects(b)),
        "disjoint" => binary_geom(|a, b| !a.intersects(b)),
        "contains" => binary_geom(|a, b| a.relate(b).is_contains()),
        "within" => binary_geom(|a, b| a.relate(b).is_within()),
        "touches" => binary_geom(|a, b| a.relate(b).is_touches()),
        "crosses" => binary_geom(|a, b| a.relate(b).is_crosses()),
        "overlaps" => binary_geom(|a, b| a.relate(b).is_overlaps()),
        "equalsExact" => binary_geom(|a, b| a == b),
        "isNull" => Val::Bool(matches!(arg(args, 0), Val::Null)),
        "equalTo" => Val::Bool(string(args, 0) == string(args, 1)),
        "notEqualTo" => Val::Bool(string(args, 0) != string(args, 1)),
        "in" => {
            let v = string(args, 0);
            Val::Bool(args[1..].iter().any(|a| a.as_string() == v))
        }
        "if_then_else" => {
            if arg(args, 0).as_bool().unwrap_or(false) {
                arg(args, 1).clone()
            } else {
                arg(args, 2).clone()
            }
        }
        "not" => arg(args, 0)
            .as_bool()
            .map(|b| Val::Bool(!b))
            .unwrap_or(Val::Null),
        "strMatches" => match (string(args, 0), string(args, 1)) {
            (Some(s), Some(re)) => Val::Bool(regex_match(&re, &s)?),
            _ => Val::Null,
        },
        "random" => Val::Num(random()),
        "dateParse" => {
            let format = string(args, 0).unwrap_or_default();
            let text = string(args, 1).unwrap_or_default();
            let parsed = chrono::NaiveDateTime::parse_from_str(&text, &java_to_chrono(&format))
                .ok()
                .or_else(|| {
                    chrono::NaiveDate::parse_from_str(&text, &java_to_chrono(&format))
                        .ok()
                        .and_then(|d| d.and_hms_opt(0, 0, 0))
                });
            parsed
                .map(|ndt| Val::Time(ndt.and_utc()))
                .unwrap_or(Val::Null)
        }
        "now" => Val::Time(chrono::Utc::now()),
        _ => unreachable!("function in table"),
    })
}

/// Convert common Java SimpleDateFormat patterns to chrono format strings
fn java_to_chrono(fmt: &str) -> String {
    fmt.replace("yyyy", "%Y")
        .replace("MM", "%m")
        .replace("dd", "%d")
        .replace("HH", "%H")
        .replace("mm", "%M")
        .replace("ss", "%S")
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{point, polygon};

    fn s(v: &str) -> Val {
        Val::Str(v.to_string())
    }

    #[test]
    fn string_functions() {
        assert_eq!(call("strConcat", &[s("a"), s("b")]).unwrap(), s("ab"));
        assert_eq!(call("strToUpperCase", &[s("abc")]).unwrap(), s("ABC"));
        assert_eq!(call("strLength", &[s("Géo")]).unwrap(), Val::Int(3));
        assert_eq!(
            call("strSubstring", &[s("abcdef"), s("1"), s("3")]).unwrap(),
            s("bc")
        );
        assert_eq!(
            call("strIndexOf", &[s("abcdef"), s("cd")]).unwrap(),
            Val::Int(2)
        );
        assert_eq!(
            call("strReplace", &[s("aXbX"), s("X"), s("-"), s("true")]).unwrap(),
            s("a-b-")
        );
        assert_eq!(
            call("strCapitalize", &[s("hello world")]).unwrap(),
            s("Hello World")
        );
        assert!(call("unknownFn", &[]).is_err());
        assert!(call("strConcat", &[s("a")]).is_err());
    }

    #[test]
    fn math_and_logic() {
        assert_eq!(call("abs", &[s("-3")]).unwrap(), Val::Num(3.0));
        assert_eq!(call("round", &[Val::Num(2.5)]).unwrap(), Val::Int(3));
        assert_eq!(
            call("max", &[Val::Int(2), Val::Num(3.5)]).unwrap(),
            Val::Num(3.5)
        );
        assert_eq!(
            call("in", &[s("b"), s("a"), s("b")]).unwrap(),
            Val::Bool(true)
        );
        assert_eq!(
            call("if_then_else", &[Val::Bool(false), s("x"), s("y")]).unwrap(),
            s("y")
        );
        assert_eq!(call("isNull", &[Val::Null]).unwrap(), Val::Bool(true));
    }

    #[test]
    fn geometry_functions() {
        let sq: Geometry<f64> = polygon![(x: 0., y: 0.), (x: 2., y: 0.), (x: 2., y: 2.), (x: 0., y: 2.), (x: 0., y: 0.)].into();
        assert_eq!(
            call("area", &[Val::Geom(sq.clone())]).unwrap(),
            Val::Num(4.0)
        );
        assert_eq!(
            call("geometryType", &[Val::Geom(sq.clone())]).unwrap(),
            s("Polygon")
        );
        assert_eq!(
            call(
                "contains",
                &[
                    Val::Geom(sq.clone()),
                    Val::Geom(point!(x: 1., y: 1.).into())
                ]
            )
            .unwrap(),
            Val::Bool(true)
        );
        assert_eq!(
            call("getX", &[Val::Geom(point!(x: 3., y: 4.).into())]).unwrap(),
            Val::Num(3.0)
        );
        assert_eq!(
            call(
                "distance",
                &[
                    Val::Geom(point!(x: 0., y: 0.).into()),
                    Val::Geom(point!(x: 3., y: 4.).into())
                ]
            )
            .unwrap(),
            Val::Num(5.0)
        );
    }
}
