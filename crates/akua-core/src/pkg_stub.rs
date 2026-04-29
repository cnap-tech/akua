//! Synthesize import-only stubs for Akua-package deps.
//!
//! `import upstream` against a real Akua package runs upstream's
//! module-level body at import time — `input: Input = ctx.input()`
//! reads the consumer's `option("input")` against upstream's schema
//! and panics inside KCL's `type_pack_and_check` when shapes diverge.
//!
//! Mirroring how `import charts.webapp` reaches a synthesized
//! `Chart`/`Values` shape, we emit a stub `<alias>.k` per dep
//! containing only the upstream's `import` and `schema` declarations.
//! Stubs mount at `/akua-pkgs` inside the worker; the consumer writes
//! `import pkgs.<alias>` to reach the typed schemas without firing
//! upstream's body.
//!
//! `pkg.render` itself is unaffected — its handler still loads the
//! real `package.k` from disk and renders it through `PackageK::render`.
//! The stub is for compile-time type reach only.

/// Textually extract schema declarations from a `package.k` source.
///
/// Keeps every top-level `import` line (schemas may reference imported
/// types) and every `schema NAME:` block (body recognised by
/// indentation; the block ends at the next non-blank non-indented
/// non-comment line). Drops top-level assignments and free expressions
/// — those are the bodies that would otherwise execute at import time.
///
/// Best-effort; does not parse KCL. Relies on the indentation
/// convention every Package.k follows. The resulting stub still goes
/// through KCL's parser when the consumer imports it; malformed input
/// surfaces as a normal compile error.
pub fn extract_schemas(source: &str) -> String {
    let mut out = String::new();
    let mut in_schema = false;

    for line in source.lines() {
        let trimmed_start = line.trim_start();
        let is_blank = trimmed_start.is_empty();
        let is_indented = line.starts_with(' ') || line.starts_with('\t');
        let is_comment = trimmed_start.starts_with('#');

        if in_schema {
            if is_blank || is_indented || is_comment {
                out.push_str(line);
                out.push('\n');
                continue;
            }
            in_schema = false;
        }

        if is_blank {
            // Compress runs of blank lines into one separator.
            if !out.ends_with("\n\n") {
                out.push('\n');
            }
            continue;
        }

        if !is_indented {
            if trimmed_start.starts_with("import ") {
                out.push_str(line);
                out.push('\n');
                continue;
            }
            if trimmed_start.starts_with("schema ") || trimmed_start.starts_with("protocol ") {
                in_schema = true;
                out.push_str(line);
                out.push('\n');
                continue;
            }
        }
        // Top-level assignments, expressions, decorator-only lines:
        // drop. Schemas + imports are the only things that survive.
    }

    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_imports_and_schema_blocks() {
        let src = r#"
import akua.ctx

schema Input:
    """The thing."""
    name: str
    replicas: int = 2

input: Input = ctx.input()

resources = [{"foo": "bar"}]
"#;
        let stub = extract_schemas(src);
        assert!(stub.contains("import akua.ctx"));
        assert!(stub.contains("schema Input:"));
        assert!(stub.contains("name: str"));
        assert!(stub.contains("replicas: int = 2"));
        assert!(!stub.contains("ctx.input"));
        assert!(!stub.contains("resources"));
    }

    #[test]
    fn handles_check_blocks() {
        let src = r#"
schema Input:
    replicas: int = 1

    check:
        replicas >= 1, "at least one"

input: Input = {}
"#;
        let stub = extract_schemas(src);
        assert!(stub.contains("check:"));
        assert!(stub.contains("replicas >= 1"));
        assert!(!stub.contains("input: Input"));
    }

    #[test]
    fn keeps_multiple_schemas() {
        let src = r#"
schema A:
    a: str

schema B:
    b: int

something = A {a = "x"}
"#;
        let stub = extract_schemas(src);
        assert!(stub.contains("schema A:"));
        assert!(stub.contains("schema B:"));
        assert!(!stub.contains("something"));
    }

    #[test]
    fn empty_or_body_only_source_yields_blank_stub() {
        let stub = extract_schemas("resources = []\n");
        assert_eq!(stub.trim(), "");
    }
}
