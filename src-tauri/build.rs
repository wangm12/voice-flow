fn main() {
    // Cargo test/check/clippy run without Tauri's sidecar packaging command.
    // Keep the resource directory available for those debug builds; the real
    // executable and Metal bundles are still produced by package.sh.
    if std::env::var("PROFILE").as_deref() == Ok("debug") {
        let stage =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/mlx-sidecar/stage");
        std::fs::create_dir_all(stage).expect("failed to create MLX resource staging directory");
    }
    tauri_build::build()
}
