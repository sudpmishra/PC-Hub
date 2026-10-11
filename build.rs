fn main() {
    // Translations of the interface text, from lang/<language>/LC_MESSAGES/pc-hub.po.
    let config = slint_build::CompilerConfiguration::new()
        .with_bundled_translations("lang")
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    println!("cargo:rerun-if-changed=lang");
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI failed to compile");

    // Embed the icon in the .exe so Explorer and shortcuts show it.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=ui/icon.ico");
        winresource::WindowsResource::new()
            .set_icon("ui/icon.ico")
            .compile()
            .expect("failed to embed the app icon");
    }
}
