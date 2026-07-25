//! KDL document model — thin adapter over the official `kdl` crate.
//!
//! This wraps the real KDL parser/emitter with a simplified node type that
//! the config module uses for round-trip parse/emit of config.kdl.

use kdl::{KdlDocument, KdlNode, KdlValue};

/// A single KDL node: `name arg1 arg2 key1=val1 key2=val2 { children }`
#[derive(Debug, Clone)]
pub struct Node {
    pub name: String,
    pub args: Vec<Value>,
    pub props: Vec<(String, Value)>,
    pub children: Vec<Node>,
}

/// A KDL value — wraps the real KdlValue with a simplified enum.
#[derive(Debug, Clone)]
pub enum Value {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Float(f) => Some(*f as i64),
            _ => None,
        }
    }
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Convert to KdlValue for emission.
    fn to_kdl(&self) -> KdlValue {
        match self {
            Value::Str(s) => KdlValue::String(s.clone()),
            Value::Int(i) => KdlValue::Integer(*i as i128),
            Value::Float(f) => KdlValue::Float(*f),
            Value::Bool(b) => KdlValue::Bool(*b),
        }
    }
}

impl Node {
    pub fn new(name: impl Into<String>) -> Self {
        Node {
            name: name.into(),
            args: Vec::new(),
            props: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Get a property value by key.
    pub fn prop(&self, key: &str) -> Option<&Value> {
        self.props
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    }

    pub fn prop_str(&self, key: &str, default: &str) -> String {
        self.prop(key)
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| default.to_string())
    }

    pub fn prop_int(&self, key: &str, default: i64) -> i64 {
        self.prop(key).and_then(|v| v.as_int()).unwrap_or(default)
    }

    pub fn prop_float(&self, key: &str, default: f64) -> f64 {
        self.prop(key).and_then(|v| v.as_float()).unwrap_or(default)
    }

    pub fn prop_bool(&self, key: &str, default: bool) -> bool {
        self.prop(key).and_then(|v| v.as_bool()).unwrap_or(default)
    }

    /// Find children by name.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    pub fn set_str(&mut self, key: &str, val: impl Into<String>) {
        let v = Value::Str(val.into());
        if let Some(slot) = self.props.iter_mut().find(|(k, _)| k == key) {
            slot.1 = v;
        } else {
            self.props.push((key.to_string(), v));
        }
    }

    pub fn set_int(&mut self, key: &str, val: i64) {
        let v = Value::Int(val);
        if let Some(slot) = self.props.iter_mut().find(|(k, _)| k == key) {
            slot.1 = v;
        } else {
            self.props.push((key.to_string(), v));
        }
    }

    pub fn set_bool(&mut self, key: &str, val: bool) {
        let v = Value::Bool(val);
        if let Some(slot) = self.props.iter_mut().find(|(k, _)| k == key) {
            slot.1 = v;
        } else {
            self.props.push((key.to_string(), v));
        }
    }

    pub fn set_float(&mut self, key: &str, val: f64) {
        let v = Value::Float(val);
        if let Some(slot) = self.props.iter_mut().find(|(k, _)| k == key) {
            slot.1 = v;
        } else {
            self.props.push((key.to_string(), v));
        }
    }
}

/// A KDL document is just a list of top-level nodes.
#[derive(Debug, Clone, Default)]
pub struct Document {
    pub nodes: Vec<Node>,
}

// ── Parse using the official kdl crate ──────────────────────────────────

/// Parse a KDL string into a Document using the official kdl crate.
pub fn parse(input: &str) -> Result<Document, String> {
    let doc: KdlDocument = input.parse().map_err(|e| format!("{}", e))?;
    Ok(convert_from_kdl(&doc))
}

fn convert_from_kdl(doc: &KdlDocument) -> Document {
    let nodes = doc.nodes().iter().map(convert_node_from_kdl).collect();
    Document { nodes }
}

fn convert_node_from_kdl(node: &KdlNode) -> Node {
    let name = node.name().value().to_string();

    let mut args = Vec::new();
    let mut props = Vec::new();

    for entry in node.iter() {
        match entry.name() {
            Some(id) => {
                // Property: key=value
                props.push((
                    id.value().to_string(),
                    convert_value_from_kdl(entry.value()),
                ));
            }
            None => {
                // Arg (positional)
                args.push(convert_value_from_kdl(entry.value()));
            }
        }
    }

    let children = node
        .children()
        .map(|d| d.nodes().iter().map(convert_node_from_kdl).collect())
        .unwrap_or_default();

    Node {
        name,
        args,
        props,
        children,
    }
}

fn convert_value_from_kdl(val: &KdlValue) -> Value {
    match val {
        KdlValue::String(s) => Value::Str(s.clone()),
        KdlValue::Integer(i) => Value::Int(*i as i64),
        KdlValue::Float(f) => Value::Float(*f),
        KdlValue::Bool(b) => Value::Bool(*b),
        KdlValue::Null => Value::Str(String::new()),
    }
}

// ── Emit using the official kdl crate ───────────────────────────────────

/// Emit a Document back to KDL text using the official kdl crate.
pub fn dump(doc: &Document) -> String {
    let mut kdl_doc = KdlDocument::new();
    for node in &doc.nodes {
        kdl_doc.nodes_mut().push(convert_node_to_kdl(node));
    }
    kdl_doc.to_string()
}

fn convert_node_to_kdl(node: &Node) -> KdlNode {
    let mut kdl_node = KdlNode::new(node.name.as_str());

    for arg in &node.args {
        kdl_node.push(kdl::KdlEntry::new(arg.to_kdl()));
    }

    for (k, v) in &node.props {
        kdl_node.push(kdl::KdlEntry::new_prop(k.as_str(), v.to_kdl()));
    }

    if !node.children.is_empty() {
        let mut child_doc = KdlDocument::new();
        for child in &node.children {
            child_doc.nodes_mut().push(convert_node_to_kdl(child));
        }
        kdl_node.set_children(child_doc);
    }

    kdl_node
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_emit_roundtrip() {
        let input = r#"settings defaultDelay=50 darkMode=#true toggleKey="Delete"
active game=SEBNS class=Destoryer spec=Reaper
game SEBNS path="C:/Games/Client.exe" {
    class Destoryer {
        spec Reaper {
            macro name=rotation hotkey=rbutton delay=5 mode=toggle enabled=#true {
                key t
            }
        }
    }
}
"#;
        let doc = parse(input).unwrap();
        assert_eq!(doc.nodes.len(), 3);
        assert_eq!(doc.nodes[0].name, "settings");
        assert_eq!(doc.nodes[0].prop_int("defaultDelay", 0), 50);
        assert_eq!(doc.nodes[0].prop_bool("darkMode", false), true);
        assert_eq!(doc.nodes[1].prop_str("game", ""), "SEBNS");

        // Emit should produce parseable KDL
        let out = dump(&doc);
        let doc2 = parse(&out).unwrap();
        assert_eq!(doc2.nodes.len(), 3);
    }

    #[test]
    fn parse_v1_bools() {
        let input = "macro name=test enabled=#true holdMode=#false";
        let doc = parse(input).unwrap();
        assert_eq!(doc.nodes[0].prop_bool("enabled", false), true);
        assert_eq!(doc.nodes[0].prop_bool("holdMode", true), false);
    }
}
