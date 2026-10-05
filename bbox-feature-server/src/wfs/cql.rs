//! CQL / ECQL filters (GeoServer CQL_FILTER vendor parameter), translated into the filter model.

use crate::wfs::filter::*;

pub fn parse_cql(text: &str, ctx: &ParseContext) -> Result<Filter, FilterError> {
    let tokens = lex(text)?;
    let mut p = Parser {
        text,
        tokens,
        pos: 0,
        ctx,
    };
    let f = p.filter()?;
    if p.pos < p.tokens.len() {
        return Err(p.error("unexpected input"));
    }
    Ok(f)
}

/// Parse a WKT geometry (2D)
pub fn parse_wkt(text: &str) -> Result<geo::Geometry<f64>, FilterError> {
    let ctx = ParseContext::new(FilterVersion::V200);
    let tokens = lex(text)?;
    let mut p = Parser {
        text,
        tokens,
        pos: 0,
        ctx: &ctx,
    };
    let g = p.wkt()?;
    if p.pos < p.tokens.len() {
        return Err(p.error("unexpected input"));
    }
    Ok(g)
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Quoted(String),
    Str(String),
    Num(String),
    Time(String),
    LParen,
    RParen,
    Comma,
    Op(String),
}

/// Token with byte offsets in the source
#[derive(Clone, Debug)]
struct Token {
    tok: Tok,
    start: usize,
    end: usize,
}

fn lex(text: &str) -> Result<Vec<Token>, FilterError> {
    let bytes: Vec<char> = text.chars().collect();
    let mut offsets = Vec::with_capacity(bytes.len() + 1);
    let mut o = 0;
    for c in &bytes {
        offsets.push(o);
        o += c.len_utf8();
    }
    offsets.push(o);
    let mut tokens = Vec::new();
    let mut i = 0;
    let err = |msg: &str| FilterError::Parse(format!("CQL: {msg}"));
    while i < bytes.len() {
        let c = bytes[i];
        let start = i;
        let tok = if c.is_whitespace() {
            i += 1;
            continue;
        } else if c == '(' {
            i += 1;
            Tok::LParen
        } else if c == ')' {
            i += 1;
            Tok::RParen
        } else if c == ',' {
            i += 1;
            Tok::Comma
        } else if c == '\'' {
            let mut s = String::new();
            i += 1;
            loop {
                match bytes.get(i) {
                    None => return Err(err("unterminated string")),
                    Some('\'') if bytes.get(i + 1) == Some(&'\'') => {
                        s.push('\'');
                        i += 2;
                    }
                    Some('\'') => {
                        i += 1;
                        break;
                    }
                    Some(ch) => {
                        s.push(*ch);
                        i += 1;
                    }
                }
            }
            Tok::Str(s)
        } else if c == '"' {
            let mut s = String::new();
            i += 1;
            loop {
                match bytes.get(i) {
                    None => return Err(err("unterminated quoted identifier")),
                    Some('"') => {
                        i += 1;
                        break;
                    }
                    Some(ch) => {
                        s.push(*ch);
                        i += 1;
                    }
                }
            }
            Tok::Quoted(s)
        } else if c.is_ascii_digit()
            || (c == '.'
                && bytes
                    .get(i + 1)
                    .map(|d| d.is_ascii_digit())
                    .unwrap_or(false))
        {
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], '.' | ':' | '-' | '+'))
            {
                // a sign belongs to the number only after an exponent or within a date
                if matches!(bytes[i], '+' | '-') {
                    let prev = bytes[i - 1];
                    let is_date = bytes[start..i].contains(&'-')
                        || bytes[start..i].contains(&'T')
                        || i - start == 4;
                    if !(prev == 'e' || prev == 'E' || is_date) {
                        break;
                    }
                }
                i += 1;
            }
            let s: String = bytes[start..i].iter().collect();
            if s.contains('T') || (s.len() >= 10 && s.chars().filter(|c| *c == '-').count() >= 2) {
                Tok::Time(s)
            } else {
                Tok::Num(s)
            }
        } else if c.is_alphabetic() || c == '_' {
            while i < bytes.len()
                && (bytes[i].is_alphanumeric() || matches!(bytes[i], '_' | ':' | '.'))
            {
                i += 1;
            }
            Tok::Ident(bytes[start..i].iter().collect())
        } else {
            let two: String = bytes[i..(i + 2).min(bytes.len())].iter().collect();
            if matches!(two.as_str(), "<>" | "<=" | ">=" | "!=") {
                i += 2;
                Tok::Op(two)
            } else if matches!(c, '=' | '<' | '>' | '+' | '-' | '*' | '/') {
                i += 1;
                Tok::Op(c.to_string())
            } else {
                return Err(err(&format!("unexpected character `{c}`")));
            }
        };
        tokens.push(Token {
            tok,
            start: offsets[start],
            end: offsets[i],
        });
    }
    Ok(tokens)
}

struct Parser<'a> {
    text: &'a str,
    tokens: Vec<Token>,
    pos: usize,
    ctx: &'a ParseContext,
}

const SPATIAL: &[&str] = &[
    "INTERSECTS",
    "DISJOINT",
    "CONTAINS",
    "WITHIN",
    "TOUCHES",
    "CROSSES",
    "OVERLAPS",
    "EQUALS",
];
const WKT: &[&str] = &[
    "POINT",
    "LINESTRING",
    "POLYGON",
    "MULTIPOINT",
    "MULTILINESTRING",
    "MULTIPOLYGON",
    "GEOMETRYCOLLECTION",
    "ENVELOPE",
];

impl Parser<'_> {
    fn error(&self, msg: &str) -> FilterError {
        let at = self
            .tokens
            .get(self.pos)
            .map(|t| t.start)
            .unwrap_or(self.text.len());
        FilterError::Parse(format!("CQL: {msg} at position {at} in `{}`", self.text))
    }
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos).map(|t| &t.tok)
    }
    fn peek_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(s)) if s.eq_ignore_ascii_case(kw))
    }
    fn peek_kw_at(&self, offset: usize, kw: &str) -> bool {
        matches!(self.tokens.get(self.pos + offset).map(|t| &t.tok), Some(Tok::Ident(s)) if s.eq_ignore_ascii_case(kw))
    }
    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.peek_kw(kw) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, t: Tok) -> Result<(), FilterError> {
        if self.peek() == Some(&t) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.error(&format!("expected {t:?}")))
        }
    }
    fn expect_kw(&mut self, kw: &str) -> Result<(), FilterError> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            Err(self.error(&format!("expected {kw}")))
        }
    }

    fn filter(&mut self) -> Result<Filter, FilterError> {
        let mut parts = vec![self.and()?];
        while self.eat_kw("OR") {
            parts.push(self.and()?);
        }
        Ok(if parts.len() == 1 {
            parts.pop().expect("one")
        } else {
            Filter::Or(parts)
        })
    }

    fn and(&mut self) -> Result<Filter, FilterError> {
        let mut parts = vec![self.not()?];
        while self.eat_kw("AND") {
            parts.push(self.not()?);
        }
        Ok(if parts.len() == 1 {
            parts.pop().expect("one")
        } else {
            Filter::And(parts)
        })
    }

    fn not(&mut self) -> Result<Filter, FilterError> {
        if self.eat_kw("NOT") {
            return Ok(Filter::Not(Box::new(self.not()?)));
        }
        self.predicate()
    }

    fn predicate(&mut self) -> Result<Filter, FilterError> {
        // parenthesised filter (backtrack if it is an expression)
        if self.peek() == Some(&Tok::LParen) {
            let save = self.pos;
            self.pos += 1;
            if let Ok(f) = self.filter() {
                if self.peek() == Some(&Tok::RParen) {
                    self.pos += 1;
                    return Ok(f);
                }
            }
            self.pos = save;
        }
        if self.eat_kw("INCLUDE") {
            return Ok(Filter::Constant(true));
        }
        if self.eat_kw("EXCLUDE") {
            return Ok(Filter::Constant(false));
        }
        if self.peek_kw("IN")
            && self
                .tokens
                .get(self.pos + 1)
                .map(|t| t.tok == Tok::LParen)
                .unwrap_or(false)
        {
            self.pos += 1;
            let ids = self.literal_list()?;
            return Ok(Filter::ResourceIds(
                ids.into_iter()
                    .map(|rid| ResourceId {
                        rid,
                        version: None,
                        start_date: None,
                        end_date: None,
                    })
                    .collect(),
            ));
        }
        if let Some(Tok::Ident(name)) = self.peek().cloned() {
            let upper = name.to_ascii_uppercase();
            let is_call = self
                .tokens
                .get(self.pos + 1)
                .map(|t| t.tok == Tok::LParen)
                .unwrap_or(false);
            if is_call
                && (SPATIAL.contains(&upper.as_str())
                    || matches!(upper.as_str(), "BBOX" | "DWITHIN" | "BEYOND"))
            {
                return self.spatial(&upper);
            }
        }
        let left = self.expr()?;
        let negated = self.eat_kw("NOT");
        if self.eat_kw("BETWEEN") {
            let lower = self.expr()?;
            self.expect_kw("AND")?;
            let upper = self.expr()?;
            let f = Filter::Between {
                expr: left,
                lower,
                upper,
            };
            return Ok(if negated { Filter::Not(Box::new(f)) } else { f });
        }
        for (kw, case) in [("LIKE", true), ("ILIKE", false)] {
            if self.eat_kw(kw) {
                let pattern = match self.peek().cloned() {
                    Some(Tok::Str(s)) => {
                        self.pos += 1;
                        s
                    }
                    _ => return Err(self.error("expected pattern string")),
                };
                let f = Filter::Like {
                    expr: left,
                    pattern,
                    wild_card: '%',
                    single_char: '_',
                    escape_char: '\\',
                    match_case: case,
                };
                return Ok(if negated { Filter::Not(Box::new(f)) } else { f });
            }
        }
        if self.eat_kw("IN") {
            self.expect(Tok::LParen)?;
            let mut alts = Vec::new();
            loop {
                let v = self.expr()?;
                alts.push(Filter::Comparison {
                    op: ComparisonOp::EqualTo,
                    left: left.clone(),
                    right: v,
                    match_case: true,
                    match_action: MatchAction::Any,
                });
                if self.peek() == Some(&Tok::Comma) {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            self.expect(Tok::RParen)?;
            let f = if alts.len() == 1 {
                alts.pop().expect("one")
            } else {
                Filter::Or(alts)
            };
            return Ok(if negated { Filter::Not(Box::new(f)) } else { f });
        }
        if negated {
            return Err(self.error("expected BETWEEN, LIKE or IN after NOT"));
        }
        if self.eat_kw("IS") {
            let not = self.eat_kw("NOT");
            self.expect_kw("NULL")?;
            let f = Filter::IsNull(left);
            return Ok(if not { Filter::Not(Box::new(f)) } else { f });
        }
        // temporal
        for (kw, op) in [
            ("BEFORE", TemporalOp::Before),
            ("AFTER", TemporalOp::After),
            ("DURING", TemporalOp::During),
            ("TEQUALS", TemporalOp::TEquals),
        ] {
            if self.peek_kw(kw) {
                self.pos += 1;
                // combined forms: BEFORE OR DURING, DURING OR AFTER
                let mut ops = vec![op];
                if self.peek_kw("OR")
                    && (self.peek_kw_at(1, "DURING") || self.peek_kw_at(1, "AFTER"))
                {
                    self.pos += 1;
                    let second = if self.eat_kw("DURING") {
                        TemporalOp::During
                    } else {
                        self.expect_kw("AFTER")?;
                        TemporalOp::After
                    };
                    ops.push(second);
                }
                let operand = self.temporal_operand()?;
                let mut fs: Vec<Filter> = ops
                    .into_iter()
                    .map(|op| Filter::Temporal {
                        op,
                        expr: left.clone(),
                        operand: TemporalOperand::Literal(operand.clone()),
                    })
                    .collect();
                return Ok(if fs.len() == 1 {
                    fs.pop().expect("one")
                } else {
                    Filter::Or(fs)
                });
            }
        }
        let op = match self.peek().cloned() {
            Some(Tok::Op(o)) => o,
            _ => return Err(self.error("expected comparison operator")),
        };
        let op = match op.as_str() {
            "=" => ComparisonOp::EqualTo,
            "<>" | "!=" => ComparisonOp::NotEqualTo,
            "<" => ComparisonOp::LessThan,
            ">" => ComparisonOp::GreaterThan,
            "<=" => ComparisonOp::LessThanOrEqualTo,
            ">=" => ComparisonOp::GreaterThanOrEqualTo,
            _ => return Err(self.error("expected comparison operator")),
        };
        self.pos += 1;
        let right = self.expr()?;
        Ok(Filter::Comparison {
            op,
            left,
            right,
            match_case: true,
            match_action: MatchAction::Any,
        })
    }

    fn temporal_operand(&mut self) -> Result<TemporalLiteral, FilterError> {
        let t1 = match self.peek().cloned() {
            Some(Tok::Time(t)) | Some(Tok::Str(t)) => {
                self.pos += 1;
                t
            }
            _ => return Err(self.error("expected date/time")),
        };
        if self.peek() == Some(&Tok::Op("/".into())) {
            self.pos += 1;
            let t2 = match self.peek().cloned() {
                Some(Tok::Time(t)) | Some(Tok::Str(t)) => {
                    self.pos += 1;
                    t
                }
                _ => return Err(self.error("expected period end")),
            };
            return Ok(TemporalLiteral::Period { begin: t1, end: t2 });
        }
        Ok(TemporalLiteral::Instant(t1))
    }

    fn literal_list(&mut self) -> Result<Vec<String>, FilterError> {
        self.expect(Tok::LParen)?;
        let mut items = Vec::new();
        loop {
            match self.peek().cloned() {
                Some(Tok::Str(s)) | Some(Tok::Num(s)) | Some(Tok::Ident(s)) => {
                    self.pos += 1;
                    items.push(s);
                }
                _ => return Err(self.error("expected literal")),
            }
            if self.peek() == Some(&Tok::Comma) {
                self.pos += 1;
            } else {
                break;
            }
        }
        self.expect(Tok::RParen)?;
        Ok(items)
    }

    fn number(&mut self) -> Result<f64, FilterError> {
        let neg = if self.peek() == Some(&Tok::Op("-".into())) {
            self.pos += 1;
            true
        } else {
            false
        };
        match self.peek().cloned() {
            Some(Tok::Num(n)) => {
                self.pos += 1;
                let v: f64 = n.parse().map_err(|_| self.error("invalid number"))?;
                Ok(if neg { -v } else { v })
            }
            _ => Err(self.error("expected number")),
        }
    }

    fn spatial(&mut self, upper: &str) -> Result<Filter, FilterError> {
        self.pos += 1;
        self.expect(Tok::LParen)?;
        let property = self.expr()?;
        self.expect(Tok::Comma)?;
        if upper == "BBOX" {
            let mut nums = Vec::new();
            for i in 0..4 {
                if i > 0 {
                    self.expect(Tok::Comma)?;
                }
                nums.push(self.number()?);
            }
            let mut srs = None;
            if self.peek() == Some(&Tok::Comma) {
                self.pos += 1;
                match self.peek().cloned() {
                    Some(Tok::Str(s)) => {
                        self.pos += 1;
                        srs = Some(s);
                    }
                    _ => return Err(self.error("expected CRS string")),
                }
            }
            self.expect(Tok::RParen)?;
            let (srid, swap) = match &srs {
                Some(name) => {
                    let c = crate::wfs::crs::Crs::parse(name)
                        .map_err(|_| FilterError::Crs(name.clone()))?;
                    (Some(c.epsg), c.swap_xy())
                }
                // without CRS: axis order of the request (like filter encoding literals)
                None => (self.ctx.literal_srid, self.ctx.default_swap_xy),
            };
            let rect = if swap {
                geo::Rect::new((nums[1], nums[0]), (nums[3], nums[2]))
            } else {
                geo::Rect::new((nums[0], nums[1]), (nums[2], nums[3]))
            };
            return Ok(Filter::Spatial {
                op: SpatialOp::BBox,
                property: Some(property),
                operand: SpatialOperand::Geometry(GeometryLiteral {
                    geometry: geo::Geometry::Polygon(rect.to_polygon()),
                    srid,
                    srs_name: srs,
                }),
                distance: None,
            });
        }
        let mut geometry = self.wkt()?;
        if self.ctx.default_swap_xy {
            crate::wfs::gml::swap_xy(&mut geometry);
        }
        let mut distance = None;
        if matches!(upper, "DWITHIN" | "BEYOND") {
            self.expect(Tok::Comma)?;
            let d = self.number()?;
            self.expect(Tok::Comma)?;
            let units = match self.peek().cloned() {
                Some(Tok::Ident(u)) | Some(Tok::Str(u)) => {
                    self.pos += 1;
                    u
                }
                _ => return Err(self.error("expected distance units")),
            };
            distance = Some((d, units));
        }
        self.expect(Tok::RParen)?;
        let op = match upper {
            "INTERSECTS" => SpatialOp::Intersects,
            "DISJOINT" => SpatialOp::Disjoint,
            "CONTAINS" => SpatialOp::Contains,
            "WITHIN" => SpatialOp::Within,
            "TOUCHES" => SpatialOp::Touches,
            "CROSSES" => SpatialOp::Crosses,
            "OVERLAPS" => SpatialOp::Overlaps,
            "EQUALS" => SpatialOp::Equals,
            "DWITHIN" => SpatialOp::DWithin,
            _ => SpatialOp::Beyond,
        };
        Ok(Filter::Spatial {
            op,
            property: Some(property),
            operand: SpatialOperand::Geometry(GeometryLiteral {
                geometry,
                srid: self.ctx.literal_srid,
                srs_name: None,
            }),
            distance,
        })
    }

    /// WKT geometry literal (or ECQL ENVELOPE(minx, maxx, maxy, miny))
    fn wkt(&mut self) -> Result<geo::Geometry<f64>, FilterError> {
        let Some(Tok::Ident(kw)) = self.peek().cloned() else {
            return Err(self.error("expected WKT geometry"));
        };
        let upper = kw.to_ascii_uppercase();
        if !WKT.contains(&upper.as_str()) {
            return Err(self.error("expected WKT geometry"));
        }
        let start = self.tokens[self.pos].start;
        self.pos += 1;
        // find matching parenthesis
        let mut depth = 0;
        let mut end = None;
        while let Some(t) = self.tokens.get(self.pos) {
            match t.tok {
                Tok::LParen => depth += 1,
                Tok::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(t.end);
                        self.pos += 1;
                        break;
                    }
                }
                _ => {}
            }
            self.pos += 1;
        }
        let end = end.ok_or_else(|| self.error("unbalanced WKT"))?;
        let wkt = &self.text[start..end];
        if upper == "ENVELOPE" {
            let inner = &wkt[wkt.find('(').unwrap_or(0) + 1..wkt.len() - 1];
            let n: Vec<f64> = inner
                .split(',')
                .map(|v| v.trim().parse())
                .collect::<Result<_, _>>()
                .map_err(|_| self.error("invalid ENVELOPE"))?;
            if n.len() != 4 {
                return Err(self.error("invalid ENVELOPE"));
            }
            return Ok(geo::Geometry::Polygon(
                geo::Rect::new((n[0], n[3]), (n[1], n[2])).to_polygon(),
            ));
        }
        use geozero::ToGeo;
        geozero::wkt::Wkt(wkt)
            .to_geo()
            .map_err(|e| FilterError::Parse(format!("CQL: invalid WKT `{wkt}`: {e}")))
    }

    fn expr(&mut self) -> Result<Expr, FilterError> {
        let mut left = self.term()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) if o == "+" => ArithOp::Add,
                Some(Tok::Op(o)) if o == "-" => ArithOp::Sub,
                _ => break,
            };
            self.pos += 1;
            let right = self.term()?;
            left = Expr::Arith {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn term(&mut self) -> Result<Expr, FilterError> {
        let mut left = self.factor()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) if o == "*" => ArithOp::Mul,
                Some(Tok::Op(o)) if o == "/" => ArithOp::Div,
                _ => break,
            };
            self.pos += 1;
            let right = self.factor()?;
            left = Expr::Arith {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn factor(&mut self) -> Result<Expr, FilterError> {
        match self.peek().cloned() {
            Some(Tok::Num(n)) => {
                self.pos += 1;
                Ok(Expr::Literal(n))
            }
            Some(Tok::Op(o)) if o == "-" => {
                self.pos += 1;
                match self.peek().cloned() {
                    Some(Tok::Num(n)) => {
                        self.pos += 1;
                        Ok(Expr::Literal(format!("-{n}")))
                    }
                    _ => Err(self.error("expected number")),
                }
            }
            Some(Tok::Str(s)) | Some(Tok::Time(s)) => {
                self.pos += 1;
                Ok(Expr::Literal(s))
            }
            Some(Tok::LParen) => {
                self.pos += 1;
                let e = self.expr()?;
                self.expect(Tok::RParen)?;
                Ok(e)
            }
            Some(Tok::Quoted(name)) => {
                self.pos += 1;
                self.property(&name)
            }
            Some(Tok::Ident(name)) => {
                let upper = name.to_ascii_uppercase();
                let is_call = self
                    .tokens
                    .get(self.pos + 1)
                    .map(|t| t.tok == Tok::LParen)
                    .unwrap_or(false);
                if is_call && WKT.contains(&upper.as_str()) {
                    let mut g = self.wkt()?;
                    if self.ctx.default_swap_xy {
                        crate::wfs::gml::swap_xy(&mut g);
                    }
                    return Ok(Expr::GeometryLiteral(GeometryLiteral {
                        geometry: g,
                        srid: self.ctx.literal_srid,
                        srs_name: None,
                    }));
                }
                if matches!(upper.as_str(), "TRUE" | "FALSE") {
                    self.pos += 1;
                    return Ok(Expr::Literal(upper.to_lowercase()));
                }
                if matches!(
                    upper.as_str(),
                    "AND" | "OR" | "NOT" | "LIKE" | "BETWEEN" | "IS" | "NULL"
                ) {
                    return Err(self.error("unexpected keyword"));
                }
                self.pos += 1;
                if is_call {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if self.peek() != Some(&Tok::RParen) {
                        loop {
                            args.push(self.expr()?);
                            if self.peek() == Some(&Tok::Comma) {
                                self.pos += 1;
                            } else {
                                break;
                            }
                        }
                    }
                    self.expect(Tok::RParen)?;
                    return Ok(Expr::Function { name, args });
                }
                self.property(&name)
            }
            _ => Err(self.error("expected expression")),
        }
    }

    fn property(&self, name: &str) -> Result<Expr, FilterError> {
        let ns = &self.ctx.namespaces;
        PropertyPath::parse(name, &|p| ns.get(p).cloned()).map(Expr::Property)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wfs::model::*;

    fn ctx() -> ParseContext {
        let mut ctx = ParseContext::new(FilterVersion::V200);
        ctx.namespaces
            .insert("app".into(), "http://example.com/app".into());
        ctx
    }

    fn parse(s: &str) -> Filter {
        parse_cql(s, &ctx()).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    fn ft() -> FeatureTypeDef {
        let q = |l: &str| QName::new("http://example.com/app", "app", l);
        FeatureTypeDef {
            name: q("obs"),
            title: None,
            abstract_: None,
            keywords: vec![],
            properties: vec![
                PropertyDef::simple(q("name"), "name", ValueType::String),
                PropertyDef::simple(q("cloud"), "cloud", ValueType::Double),
                PropertyDef::simple(q("when"), "when", ValueType::DateTime),
                PropertyDef::simple(q("geom"), "geom", ValueType::Geometry(GeomType::Point)),
            ],
            srid: 4326,
            wgs84_bbox: None,
            schema_xsd: None,
            supertypes: vec![],
        }
    }

    fn feature(name: &str, cloud: f64, when: &str, x: f64, y: f64) -> Feature {
        Feature {
            id: "obs.1".into(),
            values: vec![
                Value::String(name.into()),
                Value::Double(cloud),
                Value::DateTime(when.into()),
                Value::Geometry(GeomValue::new(geo::point!(x: x, y: y).into())),
            ],
        }
    }

    fn eval(cql: &str, f: &Feature) -> bool {
        let ft = ft();
        let filter = prepare(&parse(cql), &[&ft]).unwrap();
        evaluate(
            &filter,
            &FeatureSource {
                feature_type: &ft,
                feature: f,
            },
            &EvalContext { srid: 4326 },
        )
        .unwrap()
    }

    #[test]
    fn comparisons_and_logic() {
        let f = feature("Bern", 12.5, "2020-05-01T10:00:00Z", 7.4, 46.9);
        for (cql, expected) in [
            ("name = 'Bern'", true),
            ("name <> 'Bern'", false),
            ("cloud > 10", true),
            ("cloud >= 12.5 AND cloud <= 12.5", true),
            ("cloud < 10 OR name = 'Bern'", true),
            ("NOT (cloud < 10)", true),
            ("cloud BETWEEN 10 AND 20", true),
            ("cloud NOT BETWEEN 10 AND 20", false),
            ("name LIKE 'Be%'", true),
            ("name LIKE 'be%'", false),
            ("name ILIKE 'be%'", true),
            ("name NOT LIKE 'X%'", true),
            ("name IN ('Zurich', 'Bern')", true),
            ("name NOT IN ('Zurich')", true),
            ("name IS NULL", false),
            ("name IS NOT NULL", true),
            ("app:name = 'Bern'", true),
            ("\"name\" = 'Bern'", true),
            ("cloud + 2.5 = 15", true),
            ("strToUpperCase(name) = 'BERN'", true),
            ("INCLUDE", true),
            ("EXCLUDE", false),
            ("name = 'O''Brien'", false),
        ] {
            assert_eq!(eval(cql, &f), expected, "{cql}");
        }
    }

    #[test]
    fn spatial_and_temporal() {
        let f = feature("Bern", 12.5, "2020-05-01T10:00:00Z", 7.4, 46.9);
        for (cql, expected) in [
            ("BBOX(geom, 5, 45, 10, 48)", true),
            ("BBOX(geom, 5, 45, 10, 48, 'EPSG:4326')", true),
            (
                "BBOX(geom, 45, 5, 48, 10, 'urn:ogc:def:crs:EPSG::4326')",
                true,
            ),
            (
                "INTERSECTS(geom, POLYGON((5 45, 10 45, 10 48, 5 48, 5 45)))",
                true,
            ),
            ("DISJOINT(geom, POINT(0 0))", true),
            (
                "WITHIN(geom, POLYGON((5 45, 10 45, 10 48, 5 48, 5 45)))",
                true,
            ),
            ("DWITHIN(geom, POINT(7.4 47), 20, kilometers)", true),
            ("BEYOND(geom, POINT(7.4 47), 20, kilometers)", false),
            ("when AFTER 2020-01-01T00:00:00Z", true),
            ("when BEFORE 2020-01-01T00:00:00Z", false),
            (
                "when DURING 2020-04-01T00:00:00Z/2020-06-01T00:00:00Z",
                true,
            ),
            ("when TEQUALS 2020-05-01T10:00:00Z", true),
            (
                "when BEFORE OR DURING 2020-04-01T00:00:00Z/2020-06-01T00:00:00Z",
                true,
            ),
            ("IN ('obs.1', 'obs.7')", true),
            ("IN ('obs.2')", false),
        ] {
            assert_eq!(eval(cql, &f), expected, "{cql}");
        }
    }

    #[test]
    fn errors() {
        for cql in [
            "name =",
            "name = 'x",
            "(name = 'x'",
            "BBOX(geom, 1, 2)",
            "name LIKE 5 AND",
            "FOO BAR",
        ] {
            assert!(parse_cql(cql, &ctx()).is_err(), "{cql}");
        }
    }
}
