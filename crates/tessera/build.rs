use std::{env, fs, path::PathBuf};

#[path = "build/icons.rs"]
mod icons;

fn main() {
    icons::compile();
    println!("cargo:rerun-if-changed=resources/windows/app.manifest");
    println!("cargo:rerun-if-changed=assets/icons/Tessera.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let quoted_path = |path: PathBuf| path.to_string_lossy().replace('\\', "/");
    let resource = output.join("app.rc");
    fs::write(
        &resource,
        format!(
            "#pragma code_page(65001)\n1 ICON \"{}\"\n1 24 \"{}\"\n",
            quoted_path(root.join("assets/icons/Tessera.ico")),
            quoted_path(root.join("resources/windows/app.manifest")),
        ),
    )
    .expect("cannot write Windows resources");
    embed_resource::compile_for_everything(&resource, embed_resource::NONE)
        .manifest_required()
        .expect("Windows SDK resource compiler is required");
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // Our resource already carries the complete manifest.
        println!("cargo:rustc-link-arg=/MANIFEST:NO");
    }
}
