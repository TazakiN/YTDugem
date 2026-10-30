fn main() {
    println!("cargo:rerun-if-changed=assets/windows.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    embed_resource::compile("assets/windows.rc", embed_resource::NONE)
        .manifest_required()
        .expect("Could not embed YTDugem's Windows icon");
}
