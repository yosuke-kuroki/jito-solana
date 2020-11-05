use clap::{
    crate_description, crate_name, crate_version, value_t, value_t_or_exit, values_t, App, Arg,
};
use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::exit,
    process::Command,
};

struct Config {
    bpf_out_dir: Option<PathBuf>,
    bpf_sdk: PathBuf,
    dump: bool,
    features: Vec<String>,
    manifest_path: Option<PathBuf>,
    no_default_features: bool,
    verbose: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bpf_sdk: env::current_exe()
                .expect("Unable to get current executable")
                .parent()
                .expect("Unable to get parent directory")
                .to_path_buf()
                .join("sdk/bpf"),
            bpf_out_dir: None,
            dump: false,
            features: vec![],
            manifest_path: None,
            no_default_features: false,
            verbose: false,
        }
    }
}

fn spawn<I, S>(program: &Path, args: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args = args.into_iter().collect::<Vec<_>>();
    print!("Running: {}", program.display());
    for arg in args.iter() {
        print!(" {}", arg.as_ref().to_str().unwrap_or("?"));
    }
    println!();

    let mut child = Command::new(program)
        .args(&args)
        .spawn()
        .unwrap_or_else(|err| {
            eprintln!("Failed to execute {}: {}", program.display(), err);
            exit(1);
        });

    let exit_status = child.wait().expect("failed to wait on child");
    if !exit_status.success() {
        exit(1);
    }
}

fn build_bpf(config: Config) {
    let mut metadata_command = cargo_metadata::MetadataCommand::new();
    if let Some(manifest_path) = config.manifest_path {
        metadata_command.manifest_path(manifest_path);
    }

    let metadata = metadata_command.exec().unwrap_or_else(|err| {
        eprintln!("Failed to obtain package metadata: {}", err);
        exit(1);
    });

    let root_package = metadata.root_package().unwrap_or_else(|| {
        eprintln!(
            "Workspace does not have a root package: {}",
            metadata.workspace_root.display()
        );
        exit(1);
    });

    let program_name = {
        let cdylib_targets = root_package
            .targets
            .iter()
            .filter_map(|target| {
                if target.crate_types.contains(&"cdylib".to_string()) {
                    Some(&target.name)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        match cdylib_targets.len() {
            0 => {
                println!(
                    "Note: {} crate does not contain a cdylib target",
                    root_package.name
                );
                None
            }
            1 => Some(cdylib_targets[0].replace("-", "_")),
            _ => {
                eprintln!(
                    "{} crate contains multiple cdylib targets: {:?}",
                    root_package.name, cdylib_targets
                );
                exit(1);
            }
        }
    };

    let legacy_program_feature_present = root_package.name == "solana-sdk";
    let root_package_dir = &root_package.manifest_path.parent().unwrap_or_else(|| {
        eprintln!(
            "Unable to get directory of {}",
            root_package.manifest_path.display()
        );
        exit(1);
    });

    let bpf_out_dir = config
        .bpf_out_dir
        .unwrap_or_else(|| metadata.target_directory.join("deploy"));

    let target_build_directory = metadata
        .target_directory
        .join("bpfel-unknown-unknown/release");

    env::set_current_dir(&root_package_dir).unwrap_or_else(|err| {
        eprintln!(
            "Unable to set current directory to {}: {}",
            root_package_dir.display(),
            err
        );
        exit(1);
    });

    println!("BPF SDK: {}", config.bpf_sdk.display());
    if config.no_default_features {
        println!("No default features");
    }
    if !config.features.is_empty() {
        println!("Features: {}", config.features.join(" "));
    }
    if legacy_program_feature_present {
        println!("Legacy program feature detected");
    }

    let xargo_build = config.bpf_sdk.join("rust/xargo-build.sh");
    let mut xargo_build_args = vec![];

    if config.no_default_features {
        xargo_build_args.push("--no-default-features");
    }
    for feature in &config.features {
        xargo_build_args.push("--features");
        xargo_build_args.push(feature);
    }
    if legacy_program_feature_present {
        if !config.no_default_features {
            xargo_build_args.push("--no-default-features");
        }
        xargo_build_args.push("--features=program");
    }
    if config.verbose {
        xargo_build_args.push("--verbose");
    }
    spawn(&config.bpf_sdk.join(xargo_build), &xargo_build_args);

    if let Some(program_name) = program_name {
        let program_unstripped_so = target_build_directory.join(&format!("{}.so", program_name));
        let program_dump = bpf_out_dir.join(&format!("{}-dump.txt", program_name));
        let program_so = bpf_out_dir.join(&format!("{}.so", program_name));

        fn file_older_or_missing(prerequisite_file: &Path, target_file: &Path) -> bool {
            let prerequisite_metadata = fs::metadata(prerequisite_file).unwrap_or_else(|err| {
                eprintln!(
                    "Unable to get file metadata for {}: {}",
                    prerequisite_file.display(),
                    err
                );
                exit(1);
            });

            if let Ok(target_metadata) = fs::metadata(target_file) {
                use std::time::UNIX_EPOCH;
                prerequisite_metadata.modified().unwrap_or(UNIX_EPOCH)
                    > target_metadata.modified().unwrap_or(UNIX_EPOCH)
            } else {
                true
            }
        }

        if file_older_or_missing(&program_unstripped_so, &program_so) {
            spawn(
                &config.bpf_sdk.join("scripts/strip.sh"),
                &[&program_unstripped_so, &program_so],
            );
        }

        if config.dump && file_older_or_missing(&program_unstripped_so, &program_dump) {
            spawn(
                &config.bpf_sdk.join("scripts/dump.sh"),
                &[&program_unstripped_so, &program_dump],
            );
        }

        println!();
        println!("To deploy this program:");
        println!("  $ solana deploy {}", program_so.display());
    } else if config.dump {
        println!("Note: --dump is only available for crates with a cdylib target");
    }
}

fn main() {
    let default_config = Config::default();
    let default_bpf_sdk = format!("{}", default_config.bpf_sdk.display());

    let mut args = env::args().collect::<Vec<_>>();
    // When run as a cargo subcommand, the first program argument is the subcommand name.
    // Remove it
    if let Some(arg1) = args.get(1) {
        if arg1 == "build-bpf" {
            args.remove(1);
        }
    }

    let matches = App::new(crate_name!())
        .about(crate_description!())
        .version(crate_version!())
        .arg(
            Arg::with_name("bpf_sdk")
                .long("bpf-sdk")
                .value_name("PATH")
                .takes_value(true)
                .default_value(&default_bpf_sdk)
                .help("Path to the Solana BPF SDK"),
        )
        .arg(
            Arg::with_name("dump")
                .long("dump")
                .takes_value(false)
                .help("Dump ELF information to a text file on success"),
        )
        .arg(
            Arg::with_name("verbose")
                .short("v")
                .long("verbose")
                .takes_value(false)
                .help("Use verbose output"),
        )
        .arg(
            Arg::with_name("features")
                .long("features")
                .value_name("FEATURES")
                .takes_value(true)
                .multiple(true)
                .help("Space-separated list of features to activate"),
        )
        .arg(
            Arg::with_name("no_default_features")
                .long("no-default-features")
                .takes_value(false)
                .help("Do not activate the `default` feature"),
        )
        .arg(
            Arg::with_name("manifest_path")
                .long("manifest-path")
                .value_name("PATH")
                .takes_value(true)
                .help("Path to Cargo.toml"),
        )
        .arg(
            Arg::with_name("bpf_out_dir")
                .long("bpf-out-dir")
                .value_name("DIRECTORY")
                .takes_value(true)
                .help("Place final BPF build artifacts in this directory"),
        )
        .get_matches_from(args);

    let bpf_sdk = value_t_or_exit!(matches, "bpf_sdk", PathBuf);
    let bpf_out_dir = value_t!(matches, "bpf_out_dir", PathBuf).ok();

    let config = Config {
        bpf_sdk: fs::canonicalize(&bpf_sdk).unwrap_or_else(|err| {
            eprintln!(
                "BPF SDK path does not exist: {}: {}",
                bpf_sdk.display(),
                err
            );
            exit(1);
        }),
        bpf_out_dir: bpf_out_dir.map(|bpf_out_dir| {
            if bpf_out_dir.is_absolute() {
                bpf_out_dir
            } else {
                env::current_dir()
                    .expect("Unable to get current working directory")
                    .join(bpf_out_dir)
            }
        }),
        dump: matches.is_present("dump"),
        features: values_t!(matches, "features", String)
            .ok()
            .unwrap_or_else(Vec::new),
        manifest_path: value_t!(matches, "manifest_path", PathBuf).ok(),
        no_default_features: matches.is_present("no_default_features"),
        verbose: matches.is_present("verbose"),
    };
    build_bpf(config);
}
