// Embeds res/app.rc (icon, manifest, version info) into the exe. The version
// shown in the exe's properties is taken from Cargo.toml.
fn main() {
    println!("cargo:rerun-if-changed=res/app.rc");
    println!("cargo:rerun-if-changed=res/app.ico");
    println!("cargo:rerun-if-changed=res/app.manifest");
    let macros = [
        format!("VER_MAJOR={}", env!("CARGO_PKG_VERSION_MAJOR")),
        format!("VER_MINOR={}", env!("CARGO_PKG_VERSION_MINOR")),
        format!("VER_PATCH={}", env!("CARGO_PKG_VERSION_PATCH")),
    ];
    embed_resource::compile("res/app.rc", &macros)
        .manifest_required()
        .unwrap();
}
