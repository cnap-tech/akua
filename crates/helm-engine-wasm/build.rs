use std::path::PathBuf;

fn main() {
    let wasm = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("helm-engine.wasm");
    println!("cargo:rerun-if-changed={}", wasm.display());
    if !wasm.is_file() {
        eprintln!(
            "\nerror: helm-engine.wasm is missing at {}\n\
             \n\
             Build it once before `cargo build`:\n\
             \n\
             \ttask build:helm-engine-wasm\n\
             \n\
             Requires Go 1.25+. CI downloads the pre-built artifact instead.\n",
            wasm.display()
        );
        std::process::exit(1);
    }
}
