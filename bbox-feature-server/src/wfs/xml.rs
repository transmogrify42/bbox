//! XML helpers.

/// Escape text for XML element content and attribute values
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// Minimal streaming XML writer
#[derive(Default)]
pub struct XmlWriter {
    pub out: String,
}

impl XmlWriter {
    pub fn new() -> Self {
        XmlWriter {
            out: String::with_capacity(16 * 1024),
        }
    }
    pub fn decl(&mut self) -> &mut Self {
        self.out
            .push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
        self
    }
    fn attrs(&mut self, attrs: &[(&str, &str)]) {
        for (k, v) in attrs {
            self.out.push(' ');
            self.out.push_str(k);
            self.out.push_str("=\"");
            self.out.push_str(&escape(v));
            self.out.push('"');
        }
    }
    pub fn open(&mut self, name: &str, attrs: &[(&str, &str)]) -> &mut Self {
        self.out.push('<');
        self.out.push_str(name);
        self.attrs(attrs);
        self.out.push('>');
        self
    }
    pub fn close(&mut self, name: &str) -> &mut Self {
        self.out.push_str("</");
        self.out.push_str(name);
        self.out.push('>');
        self
    }
    pub fn empty(&mut self, name: &str, attrs: &[(&str, &str)]) -> &mut Self {
        self.out.push('<');
        self.out.push_str(name);
        self.attrs(attrs);
        self.out.push_str("/>");
        self
    }
    /// Element with text content
    pub fn elem(&mut self, name: &str, text: &str) -> &mut Self {
        self.elem_attrs(name, &[], text)
    }
    pub fn elem_attrs(&mut self, name: &str, attrs: &[(&str, &str)], text: &str) -> &mut Self {
        self.open(name, attrs);
        self.out.push_str(&escape(text));
        self.close(name)
    }
    pub fn text(&mut self, text: &str) -> &mut Self {
        self.out.push_str(&escape(text));
        self
    }
    pub fn raw(&mut self, xml: &str) -> &mut Self {
        self.out.push_str(xml);
        self
    }
}

/// Serialize an element as standalone XML fragment. All namespaces in scope of the element are
/// declared on its root (prefixes may be used in text and attribute values, e.g. QNames).
pub fn serialize_node(node: roxmltree::Node) -> String {
    let mut out = String::new();
    write_node(&mut out, node, None);
    out
}

fn ns_decl(prefix: Option<&str>, uri: &str) -> Option<String> {
    if uri == "http://www.w3.org/XML/1998/namespace" {
        return None;
    }
    Some(match prefix {
        Some(p) => format!(r#" xmlns:{p}="{}""#, escape(uri)),
        None => format!(r#" xmlns="{}""#, escape(uri)),
    })
}

fn prefix_for(node: &roxmltree::Node, ns: &str) -> Option<String> {
    node.namespaces()
        .find(|n| n.uri() == ns && n.name().is_some())
        .and_then(|n| n.name().map(str::to_string))
}

fn write_node(out: &mut String, node: roxmltree::Node, parent: Option<roxmltree::Node>) {
    if node.is_text() {
        out.push_str(&escape(node.text().unwrap_or("")));
        return;
    }
    if !node.is_element() {
        return;
    }
    let tag = node.tag_name();
    // element name: default namespace if in scope, else a bound prefix
    let default_ns = node
        .namespaces()
        .find(|n| n.name().is_none())
        .map(|n| n.uri());
    let name = match tag.namespace() {
        Some(ns) if default_ns == Some(ns) => tag.name().to_string(),
        Some(ns) => match prefix_for(&node, ns) {
            Some(p) => format!("{p}:{}", tag.name()),
            None => tag.name().to_string(),
        },
        None => tag.name().to_string(),
    };
    out.push('<');
    out.push_str(&name);
    // namespace declarations: all in scope at the fragment root, new ones below
    for ns in node.namespaces() {
        let inherited = parent
            .map(|p| {
                p.namespaces()
                    .any(|pn| pn.name() == ns.name() && pn.uri() == ns.uri())
            })
            .unwrap_or(false);
        if !inherited {
            if let Some(d) = ns_decl(ns.name(), ns.uri()) {
                out.push_str(&d);
            }
        }
    }
    if parent.is_none() && tag.namespace().is_none() && default_ns.is_some() {
        out.push_str(r#" xmlns="""#);
    }
    for a in node.attributes() {
        let aname = match a.namespace() {
            Some("http://www.w3.org/XML/1998/namespace") => format!("xml:{}", a.name()),
            Some(ns) => match prefix_for(&node, ns) {
                Some(p) => format!("{p}:{}", a.name()),
                None => a.name().to_string(),
            },
            None => a.name().to_string(),
        };
        out.push_str(&format!(r#" {aname}="{}""#, escape(a.value())));
    }
    if node.has_children() {
        out.push('>');
        for c in node.children() {
            write_node(out, c, Some(node));
        }
        out.push_str("</");
        out.push_str(&name);
        out.push('>');
    } else {
        out.push_str("/>");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_with_inherited_namespaces() {
        let xml = r#"<a:root xmlns:a="urn:a" xmlns:b="urn:b"><b:child b:attr="1" plain="x&amp;y">t<a:inner>v</a:inner></b:child></a:root>"#;
        let doc = roxmltree::Document::parse(xml).unwrap();
        let child = doc.root_element().first_element_child().unwrap();
        let s = serialize_node(child);
        assert_eq!(
            s,
            r#"<b:child xmlns:a="urn:a" xmlns:b="urn:b" b:attr="1" plain="x&amp;y">t<a:inner>v</a:inner></b:child>"#
        );
        // re-parse standalone
        let doc2 = roxmltree::Document::parse(&s).unwrap();
        assert_eq!(doc2.root_element().tag_name().namespace(), Some("urn:b"));
    }

    #[test]
    fn serialize_keeps_namespaces_used_in_content() {
        // prefixes used only in text / attribute values (QNames) must stay declared
        let xml = r#"<w:GetFeature xmlns:w="urn:w"><w:Query xmlns:ns62="urn:app" typeNames="ns62:obs"><f:Filter xmlns:f="urn:f"><f:ValueReference xmlns:tns="urn:app">tns:cloud</f:ValueReference></f:Filter></w:Query></w:GetFeature>"#;
        let doc = roxmltree::Document::parse(xml).unwrap();
        let query = doc.root_element().first_element_child().unwrap();
        let s = serialize_node(query);
        let doc2 = roxmltree::Document::parse(&s).unwrap();
        let q = doc2.root_element();
        assert_eq!(q.lookup_namespace_uri(Some("ns62")), Some("urn:app"));
        let vr = q
            .descendants()
            .find(|n| n.tag_name().name() == "ValueReference")
            .unwrap();
        assert_eq!(vr.lookup_namespace_uri(Some("tns")), Some("urn:app"));
        // default namespace elements
        let xml = r#"<GetFeature xmlns="urn:w"><Query typeNames="a"/></GetFeature>"#;
        let doc = roxmltree::Document::parse(xml).unwrap();
        let s = serialize_node(doc.root_element().first_element_child().unwrap());
        assert_eq!(s, r#"<Query xmlns="urn:w" typeNames="a"/>"#);
    }

    #[test]
    fn escapes_markup() {
        assert_eq!(escape(r#"a<b>&"c'"#), "a&lt;b&gt;&amp;&quot;c&apos;");
    }
}
