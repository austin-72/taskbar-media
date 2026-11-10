// Embeds res/app.rc (icon, manifest, version info) into the exe.
fn main() {
    println!("cargo:rerun-if-changed=res/app.rc");
    println!("cargo:rerun-if-changed=res/app.ico");
    println!("cargo:rerun-if-changed=res/app.manifest");
    embed_resource::compile("res/app.rc", embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
