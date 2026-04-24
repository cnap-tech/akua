//! Shared multi-document YAML parsing for engine-callable output.
//!
//! Every Kubernetes-shaped rendering engine produces a multi-doc YAML
//! stream — one document per resource, separated by `---`. Parsing it
//! back into typed values is identical across callers (helm today,
//! kustomize next), so the logic lives here.

use serde_json::Value;

/// Parse a multi-document YAML byte slice into one `Value` per doc.
/// Empty separator docs (between resources) are dropped so callers
/// can splat the result directly into `resources`.
///
/// `plugin_name` prefixes error strings so a failure inside
/// `helm.template` looks different from one inside `kustomize.build`
/// when surfaced to a Package author.
pub(crate) fn parse(bytes: &[u8], plugin_name: &str) -> Result<Vec<Value>, String> {
    use serde::de::Deserialize;

    let text =
        std::str::from_utf8(bytes).map_err(|e| format!("{plugin_name}: output not utf-8: {e}"))?;

    let mut out = Vec::new();
    for doc in serde_yaml::Deserializer::from_str(text) {
        let value = Value::deserialize(doc)
            .map_err(|e| format!("{plugin_name}: parsing output as YAML: {e}"))?;
        if is_empty_doc(&value) {
            continue;
        }
        out.push(value);
    }
    Ok(out)
}

fn is_empty_doc(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Object(m) => m.is_empty(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multi_doc_into_resource_list() {
        let text = br#"
apiVersion: v1
kind: ConfigMap
metadata:
  name: first
---
apiVersion: v1
kind: Service
metadata:
  name: second
"#;
        let docs = parse(text, "test").expect("parse");
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0]["kind"], "ConfigMap");
        assert_eq!(docs[1]["kind"], "Service");
    }

    #[test]
    fn drops_empty_separator_docs() {
        let text = b"---\napiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: x\n---\n---\n";
        let docs = parse(text, "test").expect("parse");
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0]["metadata"]["name"], "x");
    }

    #[test]
    fn empty_input_produces_empty_list() {
        assert_eq!(parse(b"", "test").unwrap(), Vec::<Value>::new());
        assert_eq!(parse(b"---\n", "test").unwrap(), Vec::<Value>::new());
    }

    #[test]
    fn invalid_utf8_surfaces_prefixed_error() {
        let e = parse(&[0xff, 0xfe, 0xfd], "pluginX").unwrap_err();
        assert!(e.starts_with("pluginX:"), "got: {e}");
        assert!(e.contains("not utf-8"));
    }
}
