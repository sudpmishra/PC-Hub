fn main() {
    slint_build::compile("ui/app.slint").expect("Slint UI failed to compile");

    // Embed the icon in the .exe so Explorer and shortcuts show it.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=ui/icon.ico");
        winresource::WindowsResource::new()
            .set_icon("ui/icon.ico")
            .compile()
            .expect("failed to embed the app icon");
    }
}
