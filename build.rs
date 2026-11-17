use std::{
    env,
    fs::{self, File},
    io::{self, BufWriter, Write},
    path::Path,
};

fn main() {
    build_actions_protocol();

    if env::var_os("CARGO_FEATURE_API_CONTRACT_EXPORT").is_some() {
        let output =
            Path::new(&env::var_os("OUT_DIR").expect("OUT_DIR is set")).join("frontend_assets.rs");
        fs::write(output, "static FRONTEND_ASSETS: &[(&str, &[u8])] = &[];\n")
            .expect("could not generate empty exporter asset table");
        return;
    }

    let frontend_build = Path::new("frontend/build");
    println!("cargo:rerun-if-changed={}", frontend_build.display());

    if env::var("PROFILE").is_ok_and(|profile| profile == "release")
        && !frontend_build.join("index.html").is_file()
    {
        panic!(
            "production frontend assets are missing; run `bun --cwd frontend run build` before `cargo build --release`"
        );
    }
    if env::var("PROFILE").is_ok_and(|profile| profile == "release") {
        let checked = fs::read_to_string(frontend_build.join(".api-contract-checked")).expect(
            "production packaging requires the checked frontend build; run just frontend-build",
        );
        assert_eq!(
            checked.trim(),
            contract_fingerprint(),
            "frontend types/contracts are stale; run just frontend-build before packaging"
        );
    }
    build_frontend_assets(frontend_build).expect("could not generate frontend asset lookup");
}

fn contract_fingerprint() -> String {
    use sha2::{Digest, Sha256};
    fn collect(path: &Path, files: &mut Vec<std::path::PathBuf>) {
        if path.is_dir() {
            for entry in fs::read_dir(path).expect("contract input directory is readable") {
                collect(
                    &entry.expect("contract input entry is readable").path(),
                    files,
                );
            }
        } else {
            files.push(path.to_owned());
        }
    }
    let inputs = fs::read_to_string("api/inputs.txt").expect("contract input list exists");
    let mut files = Vec::new();
    for input in inputs.lines().filter(|s| !s.is_empty()) {
        println!("cargo:rerun-if-changed={input}");
        collect(Path::new(input), &mut files);
    }
    // Match JavaScript's full relative-path ordering, rather than Path's
    // component ordering (which differs for `name.rs` and `name/...`).
    files.sort_by(|left, right| left.as_os_str().cmp(right.as_os_str()));
    let mut digest = Sha256::new();
    for file in files {
        let bytes = fs::read(&file).expect("contract input is readable");
        digest.update(file.to_str().expect("contract path is UTF-8"));
        digest.update([0]);
        digest.update(bytes.len().to_string());
        digest.update([0]);
        digest.update(bytes);
    }
    format!("{:x}", digest.finalize())
}

fn build_frontend_assets(root: &Path) -> io::Result<()> {
    let mut assets = Vec::new();
    if root.try_exists()? {
        let root = root.canonicalize()?;
        let mut directories = vec![root.clone()];
        while let Some(directory) = directories.pop() {
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                let file_type = entry.file_type()?;
                let path = entry.path();
                if file_type.is_dir() {
                    directories.push(path);
                } else if file_type.is_file() {
                    let name = path
                        .strip_prefix(&root)
                        .expect("asset is inside frontend build")
                        .to_str()
                        .ok_or_else(|| io::Error::other("frontend asset path is not UTF-8"))?
                        .replace(std::path::MAIN_SEPARATOR, "/");
                    assets.push((name, path));
                } else {
                    return Err(io::Error::other(format!(
                        "frontend assets must be regular files or directories: {}",
                        path.display()
                    )));
                }
            }
        }
    }
    assets.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    let output =
        Path::new(&env::var_os("OUT_DIR").expect("OUT_DIR is set")).join("frontend_assets.rs");
    let mut output = BufWriter::new(File::create(output)?);
    writeln!(output, "static FRONTEND_ASSETS: &[(&str, &[u8])] = &[")?;
    for (name, path) in assets {
        writeln!(output, "({name:?}, include_bytes!({path:?})),")?;
    }
    writeln!(output, "];")?;
    output.flush()
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
