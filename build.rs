use std::{env, path::Path};

fn main() {
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
