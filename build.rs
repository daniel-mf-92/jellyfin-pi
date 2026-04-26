fn main() {
    slint_build::compile_with_config(
        "ui/app.slint",
        slint_build::CompilerConfiguration::new()
            .with_style("fluent-dark".into())
            // Embed referenced resources (incl. assets/MaterialIcons-Regular.ttf
            // imported by ui/icon.slint) directly into the binary so the app is
            // self-contained at runtime — no external font lookup required.
            .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles),
    )
    .unwrap();
}
