fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/nextdude.proto");
    println!("cargo:rerun-if-env-changed=PROTOC");

    if std::env::var_os("PROTOC").is_none() {
        if let Ok(protoc_path) = protoc_bin_vendored::protoc_bin_path() {
            std::env::set_var("PROTOC", protoc_path);
        }
        if std::env::var_os("PROTOC_INCLUDE").is_none() {
            if let Ok(inc_path) = protoc_bin_vendored::include_path() {
                std::env::set_var("PROTOC_INCLUDE", inc_path);
            }
        }
    }

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/nextdude.proto"], &["proto"])?;
    Ok(())
}
