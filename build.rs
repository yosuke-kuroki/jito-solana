use std::env;
use std::fs;

fn main() {
    println!("cargo:rerun-if-changed=target/perf-libs");
    println!("cargo:rerun-if-changed=build.rs");

    // Ensure target/perf-libs/ exists.  It's been observed that
    // a cargo:rerun-if-changed= directive with a non-existent
    // directory triggers a rebuild on every |cargo build| invocation
    fs::create_dir("target/perf-libs").unwrap_or_else(|err| {
        if err.kind() != std::io::ErrorKind::AlreadyExists {
            panic!("Unable to create target/perf-libs: {:?}", err);
        }
    });

    let cuda = !env::var("CARGO_FEATURE_CUDA").is_err();
    let erasure = !env::var("CARGO_FEATURE_ERASURE").is_err();

    if cuda || erasure {
        println!("cargo:rustc-link-search=native=target/perf-libs");
    }
    if cuda {
        println!("cargo:rustc-link-lib=static=cuda_verify_ed25519");
        println!("cargo:rustc-link-search=native=/usr/local/cuda/lib64");
        println!("cargo:rustc-link-lib=dylib=cudart");
        println!("cargo:rustc-link-lib=dylib=cuda");
        println!("cargo:rustc-link-lib=dylib=cudadevrt");
    }
    if erasure {
        println!("cargo:rustc-link-lib=dylib=Jerasure");
        println!("cargo:rustc-link-lib=dylib=gf_complete");
    }
}
