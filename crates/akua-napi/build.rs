// napi-rs's build helper sets the right linker flags per platform
// (`-undefined dynamic_lookup` on macOS, `/FORCE:UNRESOLVED` on
// Windows) so node's symbols get resolved at addon load time. Also
// emits the per-target build markers that `@napi-rs/cli` reads
// when it stages binaries into platform-suffixed npm packages.

extern crate napi_build;

fn main() {
    napi_build::setup();
}
