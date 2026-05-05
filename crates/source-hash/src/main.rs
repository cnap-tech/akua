//! `source-hash <workspace_root> <output_file> <input1> [<input2> ...]`
//!
//! Computes a stable content hash of the listed source roots and writes
//! the hex digest (no trailing newline) to `output_file`. Used by
//! `task build:render-worker` to record the inputs the .wasm was built
//! from, alongside the .wasm artifact.

use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let workspace = args
        .next()
        .unwrap_or_else(|| die("missing <workspace_root>"));
    let output = args.next().unwrap_or_else(|| die("missing <output_file>"));
    let inputs: Vec<PathBuf> = args.map(PathBuf::from).collect();
    if inputs.is_empty() {
        die("at least one <input> path required");
    }

    let workspace = PathBuf::from(workspace);
    let hash = source_hash::compute(&inputs, &workspace);
    std::fs::write(&output, &hash).unwrap_or_else(|e| {
        eprintln!("source-hash: write {output}: {e}");
        std::process::exit(1);
    });
}

fn die(msg: &str) -> ! {
    eprintln!("source-hash: {msg}");
    eprintln!("usage: source-hash <workspace_root> <output_file> <input1> [<input2> ...]");
    std::process::exit(1);
}
