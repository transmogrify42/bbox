//! Property paths (minimum XPath subset of FES 2.0 and Filter 1.x PropertyName).

use super::FilterError;

/// One location step, with resolved namespace
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathStep {
    /// Namespace URI, None if unprefixed
    pub ns: Option<String>,
    pub prefix: Option<String>,
    pub local: String,
    /// 1-based position predicate
    pub index: Option<usize>,
    /// Attribute step (`@name`)
    pub attribute: bool,
    /// Predicate comparing a child element or attribute with a literal: (step, value)
    pub predicate: Option<(Box<PathStep>, String)>,
    /// `schema-element(name)` step
    pub schema_element: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyPath {
    /// Original text
    pub text: String,
    pub steps: Vec<PathStep>,
}

impl PropertyPath {
    /// Parse a path with a namespace resolver for prefixes
    pub fn parse(
        text: &str,
        resolve: &dyn Fn(&str) -> Option<String>,
    ) -> Result<PropertyPath, FilterError> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(FilterError::Parse("empty property name".into()));
        }
        let trimmed = trimmed.strip_prefix("./").unwrap_or(trimmed);
        let mut steps = Vec::new();
        for raw in split_steps(trimmed) {
            if raw.is_empty() {
                return Err(FilterError::Parse(format!("invalid path `{text}`")));
            }
            steps.push(parse_step(raw, resolve, text)?);
        }
        Ok(PropertyPath {
            text: text.trim().to_string(),
            steps,
        })
    }

    /// Single unqualified/qualified step without predicates
    pub fn is_simple(&self) -> bool {
        self.steps.len() == 1
            && self.steps[0].index.is_none()
            && self.steps[0].predicate.is_none()
            && !self.steps[0].attribute
    }

    pub fn first(&self) -> &PathStep {
        &self.steps[0]
    }
}

/// Split at `/` outside of predicates and parentheses
fn split_steps(text: &str) -> Vec<&str> {
    let mut steps = Vec::new();
    let mut depth = 0;
    let mut quote: Option<char> = None;
    let mut start = 0;
    for (i, c) in text.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, '[' | '(') => depth += 1,
            (None, ']' | ')') => depth -= 1,
            (None, '/') if depth == 0 => {
                steps.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    steps.push(&text[start..]);
    steps
}

fn parse_qname(
    name: &str,
    resolve: &dyn Fn(&str) -> Option<String>,
    full: &str,
) -> Result<(Option<String>, Option<String>, String), FilterError> {
    match name.split_once(':') {
        Some((prefix, local)) => {
            let ns = resolve(prefix);
            if ns.is_none() && prefix != "gml" {
                // keep unresolved prefix for later lookup by prefix
            }
            if local.is_empty() {
                return Err(FilterError::Parse(format!("invalid name in `{full}`")));
            }
            Ok((ns, Some(prefix.to_string()), local.to_string()))
        }
        None => Ok((None, None, name.to_string())),
    }
}

fn parse_step(
    raw: &str,
    resolve: &dyn Fn(&str) -> Option<String>,
    full: &str,
) -> Result<PathStep, FilterError> {
    let raw = raw.trim();
    let raw = raw.strip_prefix("child::").unwrap_or(raw);
    if let Some(inner) = raw
        .strip_prefix("schema-element(")
        .and_then(|r| r.strip_suffix(')'))
    {
        let (ns, prefix, local) = parse_qname(inner.trim(), resolve, full)?;
        return Ok(PathStep {
            ns,
            prefix,
            local,
            index: None,
            attribute: false,
            predicate: None,
            schema_element: true,
        });
    }
    let (name_part, pred_part) = match raw.find('[') {
        Some(i) => {
            if !raw.ends_with(']') {
                return Err(FilterError::Parse(format!("invalid predicate in `{full}`")));
            }
            (&raw[..i], Some(&raw[i + 1..raw.len() - 1]))
        }
        None => (raw, None),
    };
    let (attribute, name_part) = if let Some(n) = name_part.strip_prefix('@') {
        (true, n)
    } else if let Some(n) = name_part.strip_prefix("attribute::") {
        (true, n)
    } else {
        (false, name_part)
    };
    if name_part.is_empty()
        || name_part
            .chars()
            .any(|c| c.is_whitespace() || "()[]=,'\"".contains(c))
    {
        return Err(FilterError::Parse(format!("invalid path `{full}`")));
    }
    let (ns, prefix, local) = parse_qname(name_part, resolve, full)?;
    let mut step = PathStep {
        ns,
        prefix,
        local,
        index: None,
        attribute,
        predicate: None,
        schema_element: false,
    };
    if let Some(pred) = pred_part {
        let pred = pred.trim();
        if let Ok(n) = pred.parse::<usize>() {
            if n == 0 {
                return Err(FilterError::Parse(format!("invalid index in `{full}`")));
            }
            step.index = Some(n);
        } else if let Some((lhs, rhs)) = pred.split_once('=') {
            let value = rhs
                .trim()
                .trim_matches(|c| c == '\'' || c == '"')
                .to_string();
            let lhs_step = parse_step(lhs.trim(), resolve, full)?;
            step.predicate = Some((Box::new(lhs_step), value));
        } else {
            return Err(FilterError::Parse(format!(
                "unsupported predicate `{pred}` in `{full}`"
            )));
        }
    }
    Ok(step)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(prefix: &str) -> Option<String> {
        match prefix {
            "app" => Some("http://example.com/app".to_string()),
            "gml" => Some("http://www.opengis.net/gml/3.2".to_string()),
            _ => None,
        }
    }

    fn parse(text: &str) -> PropertyPath {
        PropertyPath::parse(text, &resolver).unwrap()
    }

    #[test]
    fn simple_names() {
        let p = parse("app:name");
        assert!(p.is_simple());
        assert_eq!(p.first().local, "name");
        assert_eq!(p.first().ns.as_deref(), Some("http://example.com/app"));
        let p = parse("name");
        assert_eq!(p.first().ns, None);
        let p = parse("  gml:name ");
        assert_eq!(p.text, "gml:name");
    }

    #[test]
    fn multi_step_and_predicates() {
        let p = parse("app:Road/app:lanes[2]/@app:width");
        assert_eq!(p.steps.len(), 3);
        assert_eq!(p.steps[1].index, Some(2));
        assert!(p.steps[2].attribute);
        let p = parse("ccf:address/ccf:Address/ccf:street/@number");
        assert_eq!(p.steps.len(), 4);
        assert_eq!(p.steps[0].prefix.as_deref(), Some("ccf"));
        assert_eq!(p.steps[0].ns, None);
        let p = parse("app:lane[app:kind='bus']");
        let (pred, value) = p.first().predicate.as_ref().unwrap();
        assert_eq!(pred.local, "kind");
        assert_eq!(value, "bus");
        let p = parse("@gml:id");
        assert!(p.first().attribute);
        assert_eq!(p.first().local, "id");
        let p = parse("schema-element(app:AbstractRoad)/app:name");
        assert!(p.steps[0].schema_element);
    }

    #[test]
    fn invalid_paths() {
        for text in ["", "app:", "a//b", "a[0]", "a[", "a b", "a[foo]"] {
            assert!(
                PropertyPath::parse(text, &resolver).is_err(),
                "`{text}` should be invalid"
            );
        }
    }
}
