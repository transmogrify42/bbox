//! XML assertions for WFS responses: XPath evaluation and XSD validation.

use std::path::PathBuf;
use std::process::Command;
use sxd_document::parser;
use sxd_xpath::{Context, Factory, Value};

pub const NAMESPACES: &[(&str, &str)] = &[
    ("wfs", "http://www.opengis.net/wfs"),
    ("wfs2", "http://www.opengis.net/wfs/2.0"),
    ("gml", "http://www.opengis.net/gml"),
    ("gml32", "http://www.opengis.net/gml/3.2"),
    ("ogc", "http://www.opengis.net/ogc"),
    ("fes", "http://www.opengis.net/fes/2.0"),
    ("ows", "http://www.opengis.net/ows"),
    ("ows11", "http://www.opengis.net/ows/1.1"),
    ("xlink", "http://www.w3.org/1999/xlink"),
    ("xs", "http://www.w3.org/2001/XMLSchema"),
    ("xsi", "http://www.w3.org/2001/XMLSchema-instance"),
    ("cdf", "http://www.opengis.net/cite/data"),
    ("cgf", "http://www.opengis.net/cite/geometry"),
    ("ccf", "http://www.opengis.net/cite/complex"),
    ("sf", "http://cite.opengeospatial.org/gmlsf"),
    ("bbox", "http://www.bbox.earth/wfs"),
    ("app", "http://example.com/app"),
];

/// Parsed XML response with XPath helpers
pub struct Xml {
    pub text: String,
}

impl Xml {
    pub fn new(text: impl Into<String>) -> Self {
        let xml = Xml { text: text.into() };
        // fail early on malformed XML
        if let Err(e) = parser::parse(&xml.text) {
            panic!("malformed XML ({e:?}):\n{}", xml.text);
        }
        xml
    }

    fn eval<F: FnOnce(Value) -> R, R>(&self, xpath: &str, f: F) -> R {
        let package = parser::parse(&self.text).expect("well-formed XML");
        let document = package.as_document();
        let factory = Factory::new();
        let xpath_expr = factory
            .build(xpath)
            .unwrap_or_else(|e| panic!("invalid XPath `{xpath}`: {e:?}"))
            .expect("non-empty XPath");
        let mut context = Context::new();
        for (prefix, ns) in NAMESPACES {
            context.set_namespace(prefix, ns);
        }
        let value = xpath_expr
            .evaluate(&context, document.root())
            .unwrap_or_else(|e| panic!("XPath `{xpath}` failed: {e:?}"));
        f(value)
    }

    pub fn string(&self, xpath: &str) -> String {
        self.eval(xpath, |v| v.string())
    }

    pub fn number(&self, xpath: &str) -> f64 {
        self.eval(xpath, |v| v.number())
    }

    pub fn boolean(&self, xpath: &str) -> bool {
        self.eval(xpath, |v| v.boolean())
    }

    pub fn count(&self, xpath: &str) -> usize {
        self.number(&format!("count({xpath})")) as usize
    }

    /// String values of all nodes matching the XPath
    pub fn strings(&self, xpath: &str) -> Vec<String> {
        self.eval(xpath, |v| match v {
            Value::Nodeset(nodes) => nodes
                .document_order()
                .iter()
                .map(|n| n.string_value())
                .collect(),
            other => vec![other.string()],
        })
    }

    /// Prefixed names (using the test prefixes) of all child elements of the first node matched
    pub fn child_names(&self, xpath: &str) -> Vec<String> {
        let doc = roxmltree::Document::parse(&self.text).unwrap();
        let first = self.strings(&format!(
            "count({xpath}/preceding::*) + count({xpath}/ancestor::*)"
        ));
        let pos: usize = first
            .first()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(-1.0) as usize;
        let node = doc
            .descendants()
            .filter(|n| n.is_element())
            .nth(pos)
            .expect("node");
        node.children()
            .filter(|c| c.is_element())
            .map(|c| {
                let ns = c.tag_name().namespace().unwrap_or("");
                let prefix = NAMESPACES
                    .iter()
                    .find(|(_, u)| *u == ns)
                    .map(|(p, _)| *p)
                    .unwrap_or("");
                if prefix.is_empty() {
                    c.tag_name().name().to_string()
                } else {
                    format!("{prefix}:{}", c.tag_name().name())
                }
            })
            .collect()
    }

    #[track_caller]
    pub fn assert(&self, xpath: &str) {
        assert!(
            self.boolean(xpath),
            "XPath `{xpath}` is false for:\n{}",
            self.text
        );
    }

    #[track_caller]
    pub fn assert_not(&self, xpath: &str) {
        assert!(
            !self.boolean(xpath),
            "XPath `{xpath}` is true for:\n{}",
            self.text
        );
    }

    #[track_caller]
    pub fn assert_count(&self, xpath: &str, expected: usize) {
        let count = self.count(xpath);
        assert_eq!(
            count, expected,
            "count({xpath}) = {count}, expected {expected}:\n{}",
            self.text
        );
    }

    /// Validate against an XML schema (URL under schemas.opengis.net or local path).
    /// Skipped with a warning if the OGC schema catalog is not available.
    #[track_caller]
    pub fn assert_valid(&self, schema: &str) {
        validate(&self.text, &[schema]);
    }

    /// Validate against multiple schemas combined via a wrapper schema
    #[track_caller]
    pub fn assert_valid_all(&self, schemas: &[&str]) {
        validate(&self.text, schemas);
    }

    /// Validate a feature collection against a WFS schema plus an application schema document
    /// (e.g. the DescribeFeatureType response of the server)
    #[track_caller]
    pub fn assert_valid_with_schema_doc(&self, wfs_schema: &str, app_schema: &str) {
        let Some(dir) = schema_dir() else {
            eprintln!("WARNING: skipping XSD validation - run bbox-feature-server/tests/fetch-ogc-schemas.sh");
            return;
        };
        let local = app_schema
            .replace(
                "http://schemas.opengis.net/",
                &format!("file://{}/", dir.display()),
            )
            .replace(
                "https://schemas.opengis.net/",
                &format!("file://{}/", dir.display()),
            );
        let id = format!("{}-{:?}", std::process::id(), std::thread::current().id())
            .replace(|c: char| !c.is_alphanumeric() && c != '-', "");
        let path = scratch_dir().join(format!("app-{id}.xsd"));
        std::fs::write(&path, local).unwrap();
        let p = path.to_string_lossy().to_string();
        validate(&self.text, &[wfs_schema, &p]);
    }
}

/// Directory with OGC schemas, created by `tests/fetch-ogc-schemas.sh`
pub fn schema_dir() -> Option<PathBuf> {
    let dir = std::env::var("BBOX_OGC_SCHEMAS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/ogc-schemas")
        });
    if dir.join("catalog.xml").exists() {
        Some(dir.canonicalize().unwrap_or(dir))
    } else {
        None
    }
}

fn scratch_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/wfs-test-validation");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[track_caller]
fn validate(xml: &str, schemas: &[&str]) {
    let Some(dir) = schema_dir() else {
        eprintln!(
            "WARNING: skipping XSD validation - run bbox-feature-server/tests/fetch-ogc-schemas.sh"
        );
        return;
    };
    let id = format!("{}-{:?}", std::process::id(), std::thread::current().id())
        .replace(|c: char| !c.is_alphanumeric() && c != '-', "");
    let scratch = scratch_dir();
    let doc_path = scratch.join(format!("doc-{id}.xml"));
    std::fs::write(&doc_path, strip_schema_location(xml)).unwrap();
    let local = |s: &str| -> String {
        match s.strip_prefix("http://schemas.opengis.net/") {
            Some(rel) => format!("file://{}/{rel}", dir.display()),
            None => s.to_string(),
        }
    };
    let schema_path = if schemas.len() == 1 {
        local(schemas[0])
    } else {
        // Wrapper schema importing all schemas by namespace
        let mut imports = String::new();
        for s in schemas {
            let ns = target_namespace(&dir, s);
            imports.push_str(&format!(
                r#"<xs:import namespace="{ns}" schemaLocation="{}"/>"#,
                local(s)
            ));
        }
        let wrapper = format!(
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">{imports}</xs:schema>"#
        );
        let path = scratch.join(format!("wrapper-{id}.xsd"));
        std::fs::write(&path, wrapper).unwrap();
        path.to_string_lossy().to_string()
    };
    let output = Command::new("xmllint")
        .env("XML_CATALOG_FILES", dir.join("catalog.xml"))
        .args(["--nonet", "--noout", "--schema", &schema_path])
        .arg(&doc_path)
        .output()
        .expect("xmllint available");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "XSD validation against {schemas:?} failed:\n{stderr}\nDocument:\n{xml}"
    );
    std::fs::remove_file(&doc_path).ok();
}

/// Remove xsi:schemaLocation hints (validation uses the given schemas only)
fn strip_schema_location(xml: &str) -> String {
    let mut out = xml.to_string();
    while let Some(start) = out.find("xsi:schemaLocation=\"") {
        let rest = &out[start + 20..];
        let Some(end) = rest.find('"') else { break };
        out.replace_range(start..start + 20 + end + 1, "");
    }
    out
}

/// Read targetNamespace of a schema given by URL or path
fn target_namespace(dir: &std::path::Path, schema: &str) -> String {
    let path = if let Some(rel) = schema.strip_prefix("http://schemas.opengis.net/") {
        dir.join(rel)
    } else if let Some(rel) = schema.strip_prefix("file://") {
        PathBuf::from(rel)
    } else {
        PathBuf::from(schema)
    };
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read schema {}: {e}", path.display()));
    let doc = roxmltree::Document::parse(&text).unwrap();
    doc.root_element()
        .attribute("targetNamespace")
        .unwrap_or("")
        .to_string()
}
