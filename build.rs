fn main() {
    println!("cargo:rerun-if-changed=assets/smart-switch.rc");
    println!("cargo:rerun-if-changed=assets/smart-switch.ico");
    embed_resource::compile("assets/smart-switch.rc", embed_resource::NONE)
        .manifest_required()
        .expect("failed to embed the Smart Switch icon");
}
