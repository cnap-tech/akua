//! Value-merging operations for Helm values and umbrella chart assembly.
//!
//! Three public functions, all ported from TypeScript references:
//!
//! - [`deep_merge_values`] — immutable deep merge; arrays replaced, objects
//!   merged recursively. Port of `deepMergeValues` from `set-nested-value.ts`.
//! - [`set_nested_value`] — set a value at a dot-notation path with optional
//!   array index notation (`items[0].name`). Port of `setNestedValue` from
//!   `set-nested-value.ts`.
//! - [`merge_helm_source_values`] — merge values from multiple helm sources
//!   into a single object, nesting each source under its alias or chart name
//!   (Helm/OCI) or at the root (Git). Port of `mergeHelmSourceValues` from
//!   `chart-generation.utils.ts`.

use serde_json::{Map, Value};

use crate::source::{extract_chart_name_from_oci, get_source_alias, HelmSource};

/// Deep-merge `source` into `target`, returning a new object.
///
/// Arrays are replaced (not concatenated). Nested objects are merged
/// recursively. Non-object source values overwrite the target.
///
/// This is the immutable variant. The target is not mutated.
pub fn deep_merge_values(target: &Value, source: &Value) -> Value {
    match (target, source) {
        (Value::Object(t), Value::Object(s)) => {
            let mut out = t.clone();
            for (k, v) in s {
                let existing = out.get(k).cloned().unwrap_or(Value::Null);
                let merged = if v.is_object() && existing.is_object() {
                    deep_merge_values(&existing, v)
                } else if v.is_object() {
                    // New key gets a deep clone
                    v.clone()
                } else {
                    v.clone()
                };
                out.insert(k.clone(), merged);
            }
            Value::Object(out)
        }
        // If source is not an object, it replaces target.
        (_, _) => source.clone(),
    }
}

/// Mutating deep-merge: merge `source` into `target` in place.
///
/// Used for the umbrella-chart values merge where we're accumulating into a
/// single result object. Arrays replace, nested objects merge recursively.
pub fn deep_merge_into(target: &mut Map<String, Value>, source: &Map<String, Value>) {
    for (k, v) in source {
        match v {
            Value::Object(src_obj) => {
                match target.get_mut(k) {
                    Some(Value::Object(tgt_obj)) => {
                        deep_merge_into(tgt_obj, src_obj);
                    }
                    _ => {
                        // Key missing or not an object; clone source.
                        target.insert(k.clone(), Value::Object(src_obj.clone()));
                    }
                }
            }
            _ => {
                target.insert(k.clone(), v.clone());
            }
        }
    }
}

/// A parsed path segment: either a string property or an array index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSegment {
    Key(String),
    Index(usize),
}

/// Parse a dot-notation path with optional array indices.
///
/// `"httpRoute.hostnames[0]"` → `[Key("httpRoute"), Key("hostnames"), Index(0)]`
/// `"config.adminEmail"` → `[Key("config"), Key("adminEmail")]`
pub fn parse_path(path: &str) -> Vec<PathSegment> {
    let mut out = Vec::new();
    for part in path.split('.') {
        // Look for `key[N]`
        if let Some(bracket_idx) = part.find('[') {
            if let Some(close_idx) = part.rfind(']') {
                if close_idx > bracket_idx {
                    let key = &part[..bracket_idx];
                    let idx_str = &part[bracket_idx + 1..close_idx];
                    if !key.is_empty() {
                        if let Ok(idx) = idx_str.parse::<usize>() {
                            out.push(PathSegment::Key(key.to_string()));
                            out.push(PathSegment::Index(idx));
                            continue;
                        }
                    }
                }
            }
        }
        out.push(PathSegment::Key(part.to_string()));
    }
    out
}

/// Set a value at a dot-notation path, creating intermediate objects and
/// arrays as needed.
///
/// ```
/// use akua_core::values::set_nested_value;
/// use serde_json::{json, Value};
///
/// let mut obj = Value::Object(Default::default());
/// set_nested_value(&mut obj, "httpRoute.hostnames[0]", json!("example.com"));
/// assert_eq!(obj, json!({ "httpRoute": { "hostnames": ["example.com"] } }));
/// ```
pub fn set_nested_value(obj: &mut Value, path: &str, value: Value) {
    let segments = parse_path(path);
    if segments.is_empty() {
        return;
    }

    // Ensure root is an object or array as appropriate; we only accept object roots for the top level.
    if !obj.is_object() {
        *obj = Value::Object(Map::new());
    }

    set_at_segments(obj, &segments, value);
}

fn set_at_segments(current: &mut Value, segments: &[PathSegment], value: Value) {
    if segments.is_empty() {
        return;
    }

    if segments.len() == 1 {
        match &segments[0] {
            PathSegment::Key(k) => {
                if let Value::Object(map) = current {
                    map.insert(k.clone(), value);
                }
            }
            PathSegment::Index(i) => {
                if let Value::Array(arr) = current {
                    while arr.len() <= *i {
                        arr.push(Value::Null);
                    }
                    arr[*i] = value;
                }
            }
        }
        return;
    }

    let seg = &segments[0];
    let next_seg = &segments[1];

    match seg {
        PathSegment::Key(k) => {
            // Ensure `current[k]` exists as the right container type for the next segment.
            if let Value::Object(map) = current {
                let needs_array = matches!(next_seg, PathSegment::Index(_));
                let missing_or_wrong = match map.get(k) {
                    None | Some(Value::Null) => true,
                    Some(v) => {
                        if needs_array {
                            !v.is_array()
                        } else {
                            !v.is_object()
                        }
                    }
                };
                if missing_or_wrong {
                    map.insert(
                        k.clone(),
                        if needs_array {
                            Value::Array(Vec::new())
                        } else {
                            Value::Object(Map::new())
                        },
                    );
                }
                let child = map
                    .get_mut(k)
                    .expect("just inserted or pre-existing");
                set_at_segments(child, &segments[1..], value);
            }
        }
        PathSegment::Index(i) => {
            if let Value::Array(arr) = current {
                while arr.len() <= *i {
                    arr.push(Value::Null);
                }
                let needs_array = matches!(next_seg, PathSegment::Index(_));
                let missing_or_wrong = match &arr[*i] {
                    Value::Null => true,
                    v => {
                        if needs_array {
                            !v.is_array()
                        } else {
                            !v.is_object()
                        }
                    }
                };
                if missing_or_wrong {
                    arr[*i] = if needs_array {
                        Value::Array(Vec::new())
                    } else {
                        Value::Object(Map::new())
                    };
                }
                set_at_segments(&mut arr[*i], &segments[1..], value);
            }
        }
    }
}

/// Merge values from multiple helm sources into a single values object, using
/// CNAP's umbrella-chart aliasing rules.
///
/// - **Helm HTTP / OCI sources** (have a `chart.chart` or OCI URL): values
///   nest under the source's alias (if `id` is present) or chart name.
/// - **Git sources** (chart path, no chart name): values merge at root level.
/// - Sources without a `values` field are skipped entirely.
pub fn merge_helm_source_values(sources: &[HelmSource]) -> Value {
    let mut merged = Map::new();

    for source in sources {
        let values = match &source.values {
            Some(v) if v.is_object() => v,
            _ => continue,
        };
        let values_map = values.as_object().expect("checked above");

        // Chart name from explicit field or OCI URL.
        let chart_name = source
            .chart
            .chart
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| extract_chart_name_from_oci(&source.chart.repo_url));

        if let Some(chart_name) = chart_name {
            // Helm HTTP or OCI: nest under alias (preferred) or chart name.
            let alias = get_source_alias(source).unwrap_or(chart_name);
            let entry = merged
                .entry(alias)
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(nested) = entry {
                deep_merge_into(nested, values_map);
            }
        } else {
            // Git or raw manifests: root-level merge.
            deep_merge_into(&mut merged, values_map);
        }
    }

    Value::Object(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{ChartRef, HelmSource};
    use serde_json::json;

    fn src(
        id: Option<&str>,
        repo: &str,
        chart: Option<&str>,
        path: Option<&str>,
        values: Option<Value>,
    ) -> HelmSource {
        HelmSource {
            id: id.map(String::from),
            engine: None,
            chart: ChartRef {
                repo_url: repo.to_string(),
                chart: chart.map(String::from),
                target_revision: "1.0.0".to_string(),
                path: path.map(String::from),
            },
            values,
        }
    }

    // --- deep_merge_values ---

    #[test]
    fn deep_merge_flat_objects() {
        let a = json!({"a": 1});
        let b = json!({"b": 2});
        assert_eq!(deep_merge_values(&a, &b), json!({"a": 1, "b": 2}));
    }

    #[test]
    fn deep_merge_overwrites_scalars() {
        let a = json!({"a": 1});
        let b = json!({"a": 2});
        assert_eq!(deep_merge_values(&a, &b), json!({"a": 2}));
    }

    #[test]
    fn deep_merge_merges_nested_objects() {
        let target = json!({"httpRoute": {"hostnames": ["old.com"], "rules": [{"path": "/"}]}});
        let source = json!({"httpRoute": {"hostnames": ["new.com"]}});
        assert_eq!(
            deep_merge_values(&target, &source),
            json!({"httpRoute": {"hostnames": ["new.com"], "rules": [{"path": "/"}]}})
        );
    }

    #[test]
    fn deep_merge_replaces_arrays_not_concatenate() {
        let a = json!({"tags": ["a", "b"]});
        let b = json!({"tags": ["c"]});
        assert_eq!(deep_merge_values(&a, &b), json!({"tags": ["c"]}));
    }

    #[test]
    fn deep_merge_creates_new_nested_keys() {
        let a = json!({});
        let b = json!({"a": {"b": {"c": 1}}});
        assert_eq!(deep_merge_values(&a, &b), json!({"a": {"b": {"c": 1}}}));
    }

    #[test]
    fn deep_merge_does_not_mutate_target() {
        let target = json!({"a": {"b": 1}});
        let _ = deep_merge_values(&target, &json!({"a": {"c": 2}}));
        assert_eq!(target, json!({"a": {"b": 1}}));
    }

    // --- set_nested_value ---

    #[test]
    fn set_simple_top_level_key() {
        let mut obj = json!({});
        set_nested_value(&mut obj, "name", json!("hello"));
        assert_eq!(obj, json!({"name": "hello"}));
    }

    #[test]
    fn set_nested_dot_path() {
        let mut obj = json!({});
        set_nested_value(&mut obj, "config.adminEmail", json!("admin@example.com"));
        assert_eq!(obj, json!({"config": {"adminEmail": "admin@example.com"}}));
    }

    #[test]
    fn set_deeply_nested_path() {
        let mut obj = json!({});
        set_nested_value(&mut obj, "a.b.c.d", json!("deep"));
        assert_eq!(obj, json!({"a": {"b": {"c": {"d": "deep"}}}}));
    }

    #[test]
    fn set_array_index() {
        let mut obj = json!({});
        set_nested_value(&mut obj, "httpRoute.hostnames[0]", json!("example.com"));
        assert_eq!(obj, json!({"httpRoute": {"hostnames": ["example.com"]}}));
    }

    #[test]
    fn set_preserves_existing_values() {
        let mut obj = json!({"existing": "value", "config": {"keep": true}});
        set_nested_value(&mut obj, "config.adminEmail", json!("admin@example.com"));
        assert_eq!(
            obj,
            json!({"existing": "value", "config": {"keep": true, "adminEmail": "admin@example.com"}})
        );
    }

    #[test]
    fn set_overwrites_existing_value_at_path() {
        let mut obj = json!({"config": {"adminEmail": "old@example.com"}});
        set_nested_value(&mut obj, "config.adminEmail", json!("new@example.com"));
        assert_eq!(obj, json!({"config": {"adminEmail": "new@example.com"}}));
    }

    #[test]
    fn set_array_index_in_middle_of_path() {
        let mut obj = json!({});
        set_nested_value(&mut obj, "items[0].name", json!("first"));
        assert_eq!(obj, json!({"items": [{"name": "first"}]}));
    }

    #[test]
    fn set_multiple_array_indices() {
        let mut obj = json!({});
        set_nested_value(&mut obj, "matrix[0]", json!("a"));
        set_nested_value(&mut obj, "matrix[2]", json!("c"));
        assert_eq!(obj, json!({"matrix": ["a", null, "c"]}));
    }

    // --- merge_helm_source_values ---

    #[test]
    fn merge_nests_helm_http_source_under_alias() {
        let s = src(
            Some("thsrc_abc"),
            "https://charts.example.com",
            Some("redis"),
            None,
            Some(json!({"replicaCount": 3})),
        );
        let merged = merge_helm_source_values(&[s]);
        let obj = merged.as_object().unwrap();
        assert_eq!(obj.len(), 1);
        let (key, value) = obj.iter().next().unwrap();
        assert!(key.starts_with("redis-"));
        assert_eq!(value, &json!({"replicaCount": 3}));
    }

    #[test]
    fn merge_nests_oci_source_under_alias() {
        let s = src(
            Some("thsrc_def"),
            "oci://ghcr.io/org/postgres",
            None,
            None,
            Some(json!({"port": 5432})),
        );
        let merged = merge_helm_source_values(&[s]);
        let obj = merged.as_object().unwrap();
        let (key, value) = obj.iter().next().unwrap();
        assert!(key.starts_with("postgres-"));
        assert_eq!(value, &json!({"port": 5432}));
    }

    #[test]
    fn merge_puts_git_source_at_root() {
        let s = src(
            Some("thsrc_git"),
            "https://github.com/org/repo",
            None,
            Some("charts/app"),
            Some(json!({"globalSetting": true})),
        );
        let merged = merge_helm_source_values(&[s]);
        assert_eq!(merged, json!({"globalSetting": true}));
    }

    #[test]
    fn merge_handles_mixed_sources() {
        let git = src(
            Some("g1"),
            "https://github.com/org/repo",
            None,
            Some("base"),
            Some(json!({"global": true})),
        );
        let helm = src(
            Some("h1"),
            "https://charts.example.com",
            Some("redis"),
            None,
            Some(json!({"replicas": 2})),
        );
        let merged = merge_helm_source_values(&[git, helm]);
        let obj = merged.as_object().unwrap();
        assert_eq!(obj.get("global"), Some(&json!(true)));
        assert!(obj.keys().any(|k| k.starts_with("redis-")));
    }

    #[test]
    fn merge_skips_sources_without_values() {
        let s = src(
            Some("noval"),
            "https://charts.example.com",
            Some("redis"),
            None,
            None,
        );
        let merged = merge_helm_source_values(&[s]);
        assert_eq!(merged, json!({}));
    }

    #[test]
    fn merge_empty_sources_returns_empty_object() {
        let merged = merge_helm_source_values(&[]);
        assert_eq!(merged, json!({}));
    }

    #[test]
    fn merge_uses_chart_name_when_no_id() {
        // Without an ID, no alias → falls back to chart name as the key.
        let s = src(
            None,
            "https://charts.example.com",
            Some("redis"),
            None,
            Some(json!({"replicaCount": 1})),
        );
        let merged = merge_helm_source_values(&[s]);
        assert_eq!(merged, json!({"redis": {"replicaCount": 1}}));
    }

    #[test]
    fn merge_combines_values_for_same_chart_different_ids() {
        // Two sources with the same chart name but different IDs get different aliases.
        let s1 = src(
            Some("id_a"),
            "https://charts.example.com",
            Some("redis"),
            None,
            Some(json!({"port": 6379})),
        );
        let s2 = src(
            Some("id_b"),
            "https://charts.example.com",
            Some("redis"),
            None,
            Some(json!({"port": 6380})),
        );
        let merged = merge_helm_source_values(&[s1, s2]);
        let obj = merged.as_object().unwrap();
        // Two different alias keys, both starting with "redis-"
        assert_eq!(obj.len(), 2);
        for key in obj.keys() {
            assert!(key.starts_with("redis-"));
        }
    }
}
