use std::{env, path::Path};

fn main() {
    build_actions_protocol();

    let frontend_build = Path::new("frontend/build");
    println!("cargo:rerun-if-changed={}", frontend_build.display());

    if env::var("PROFILE").is_ok_and(|profile| profile == "release")
        && !frontend_build.join("index.html").is_file()
    {
        panic!(
            "production frontend assets are missing; run `bun --cwd frontend run build` before `cargo build --release`"
        );
    }
}

fn build_actions_protocol() {
    const ROOT: &str = "proto/forgejo-actions/proto";
    const PROTOS: &[&str] = &[
        "proto/forgejo-actions/proto/ping/v1/messages.proto",
        "proto/forgejo-actions/proto/ping/v1/services.proto",
        "proto/forgejo-actions/proto/runner/v1/messages.proto",
        "proto/forgejo-actions/proto/runner/v1/services.proto",
        "proto/forgejo-actions/proto/artifact/v4/artifact.proto",
    ];

    println!("cargo:rerun-if-changed=proto/forgejo-actions");
    let protoc = protoc_bin_vendored::protoc_bin_path()
        .expect("vendored protoc must be available for Actions protocol generation");
    let descriptor =
        Path::new(&env::var_os("OUT_DIR").expect("OUT_DIR is set")).join("forgejo_actions.bin");

    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc);
    config.file_descriptor_set_path(&descriptor);
    config.compile_well_known_types();
    config.extern_path(".google.protobuf", "::pbjson_types");
    config
        .compile_protos(PROTOS, &[ROOT])
        .expect("Forgejo Actions protobuf schemas must compile");

    pbjson_build::Builder::new()
        .register_descriptors(
            std::fs::read(descriptor)
                .expect("Actions descriptor set must be readable")
                .as_slice(),
        )
        .expect("Actions descriptors must be valid")
        .build(&[".ping.v1", ".runner.v1", ".github.actions.results.api.v1"])
        .expect("Actions protobuf JSON adapters must compile");
}
