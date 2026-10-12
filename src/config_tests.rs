use super::*;
use std::{fs, process::Command as Process};

const EXAMPLE: &str = include_str!("../config.example.toml");

fn parse(text: &str) -> ConfigLayer {
    toml::from_str(text).unwrap()
}

fn cli(args: &[&str]) -> Cli {
    // Explicitly clear the file selector supplied by the parent environment.
    let command = if args
        .iter()
        .any(|arg| matches!(*arg, "run" | "serve" | "worker" | "migrate"))
    {
        None
    } else {
        Some("run")
    };
    let mut cli = Cli::try_parse_from(
        std::iter::once("booru-thingy")
            .chain(args.iter().copied())
            .chain(command),
    )
    .unwrap();
    if !args.contains(&"--config") {
        cli.config = None;
    }
    cli
}

#[test]
fn startup_requires_an_explicit_command_and_help_lists_run() {
    use clap::{CommandFactory, error::ErrorKind};

    let error = Cli::try_parse_from(["booru-thingy"]).err().unwrap();
    assert_eq!(
        error.kind(),
        ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    );
    assert!(error.to_string().contains("run"));

    let error = Cli::try_parse_from(["booru-thingy", "--config", "missing.toml"])
        .err()
        .unwrap();
    assert_eq!(error.kind(), ErrorKind::MissingSubcommand);
    assert_eq!(cli(&["run"]).command, Some(Command::Run));

    let help = Cli::command().render_help().to_string();
    assert!(help.contains("Apply migrations, then run the API and upload workers in one process"));
    assert!(help.contains("<COMMAND>"));
}

#[test]
fn example_parses_and_validates_for_every_mode() {
    for command in [
        None,
        Some(Command::Run),
        Some(Command::Serve),
        Some(Command::Worker),
        Some(Command::Migrate),
    ] {
        let config =
            Config::from_layer(parse(EXAMPLE).with_fallback(ConfigLayer::default_values()))
                .unwrap();
        config.validate(command).unwrap();
    }
    let example = format!("{EXAMPLE}\ns3_public_endpoint = 'https://media.example.com'\n");
    Config::from_layer(parse(&example))
        .unwrap()
        .validate(None)
        .unwrap();
}

#[test]
fn all_sources_merge_per_field_in_priority_order() {
    let dir = tempfile::tempdir().unwrap();
    let xdg = dir.path().join("xdg.toml");
    fs::write(&xdg, EXAMPLE).unwrap();
    let cwd = dir.path().join("config.toml");
    fs::write(
        &cwd,
        "worker_concurrency = 3\nbase_url = 'https://cwd.example.com'",
    )
    .unwrap();
    let explicit = dir.path().join("explicit.toml");
    fs::write(&explicit, "worker_concurrency = 4\nmax_upload_bytes = 1234").unwrap();

    let config =
        Config::load_layers(&cli(&[]), ConfigLayer::empty(), dir.path(), Some(&xdg)).unwrap();
    assert_eq!(config.worker_concurrency, 3);
    let args = ["--config", "explicit.toml"];
    let config =
        Config::load_layers(&cli(&args), ConfigLayer::empty(), dir.path(), Some(&xdg)).unwrap();
    assert_eq!(config.worker_concurrency, 4);
    let environment = "worker_concurrency = 5\nlisten_addr = '127.0.0.1:8081'\naws_secret_access_key = 'env-secret'";
    let config =
        Config::load_layers(&cli(&args), parse(environment), dir.path(), Some(&xdg)).unwrap();
    assert_eq!(config.worker_concurrency, 5);
    let args = [
        "--config",
        "explicit.toml",
        "serve",
        "--worker-concurrency",
        "6",
        "--base-url",
        "https://cli.example.com",
        "--aws-secret-access-key",
        "cli-secret",
    ];
    let config =
        Config::load_layers(&cli(&args), parse(environment), dir.path(), Some(&xdg)).unwrap();
    assert_eq!(config.worker_concurrency, 6);
    assert_eq!(config.base_url, "https://cli.example.com");
    assert_eq!(
        config.listen_addr,
        "127.0.0.1:8081".parse::<SocketAddr>().unwrap()
    );
    assert_eq!(config.max_upload_bytes, 1234);
    assert_eq!(config.s3_bucket, "booru");
    assert_eq!(config.aws_secret_access_key, "cli-secret");
    config.validate(Some(Command::Serve)).unwrap();

    fs::remove_file(cwd).unwrap();
    let config =
        Config::load_layers(&cli(&[]), ConfigLayer::empty(), dir.path(), Some(&xdg)).unwrap();
    assert_eq!(config.worker_concurrency, 2);
}

#[test]
fn absent_discovered_files_use_defaults_but_explicit_files_are_required() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("absent.toml");
    let config =
        Config::load_layers(&cli(&[]), ConfigLayer::empty(), dir.path(), Some(&missing)).unwrap();
    assert_eq!(config.worker_concurrency, 2);
    assert_eq!(config.process_timeout_seconds, 300);
    assert!(config.s3_public_endpoint.is_none());
    let error = Config::load_layers(
        &cli(&["--config", "absent.toml"]),
        ConfigLayer::empty(),
        dir.path(),
        None,
    )
    .err()
    .unwrap();
    assert!(
        error
            .to_string()
            .contains("could not read configuration file")
    );
}

#[test]
fn invalid_discovered_files_are_not_hidden_by_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    for text in [
        "typo = 'sensitive-value'",
        "owner_api_key = ['sensitive-value']",
        "owner_api_key = 'sensitive-value",
    ] {
        fs::write(&path, text).unwrap();
        let error = Config::load_layers(
            &cli(&["--worker-concurrency", "4"]),
            parse(EXAMPLE),
            dir.path(),
            None,
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("invalid TOML"));
        assert!(!format!("{error:#}").contains("sensitive-value"));
    }
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(Config::load_layers(&cli(&[]), ConfigLayer::empty(), dir.path(), None).is_err());
}

#[test]
fn xdg_discovery_uses_absolute_xdg_or_home_fallback() {
    assert_eq!(
        xdg_config_path(Some("/xdg".into()), Some("/home/user".into())),
        Some("/xdg/booru-thingy/config.toml".into())
    );
    for xdg in [None, Some("".into()), Some("relative".into())] {
        assert_eq!(
            xdg_config_path(xdg, Some("/home/user".into())),
            Some("/home/user/.config/booru-thingy/config.toml".into())
        );
    }
    assert!(xdg_config_path(None, None).is_none());
    assert!(xdg_config_path(None, Some("".into())).is_none());
}

#[test]
fn config_selector_and_overrides_work_before_or_after_subcommands() {
    for args in [
        vec![
            "--config",
            "custom.toml",
            "--listen-addr",
            "127.0.0.1:9000",
            "serve",
        ],
        vec![
            "serve",
            "--config",
            "custom.toml",
            "--listen-addr",
            "127.0.0.1:9000",
        ],
    ] {
        let cli = cli(&args);
        assert_eq!(cli.config.unwrap(), PathBuf::from("custom.toml"));
        assert_eq!(cli.command, Some(Command::Serve));
        assert_eq!(
            cli.overrides.listen_addr.unwrap(),
            "127.0.0.1:9000".parse::<SocketAddr>().unwrap()
        );
        assert!(cli.overrides.owner_api_key.is_none());
    }
    let cli = cli(&[
        "--base-url",
        "https://example.com",
        "--max-upload-bytes",
        "0",
        "--worker-concurrency",
        "0",
        "--process-timeout-seconds",
        "0",
        "--rust-log",
        "warn",
        "--s3-path-style",
        "false",
        "--allow-private-source-urls",
        "true",
    ]);
    assert_eq!(cli.overrides.max_upload_bytes, Some(0));
    assert_eq!(cli.overrides.worker_concurrency, Some(0));
    assert_eq!(cli.overrides.process_timeout_seconds, Some(0));
    assert_eq!(cli.overrides.rust_log.as_deref(), Some("warn"));
    assert_eq!(cli.overrides.s3_path_style, Some(false));
    assert_eq!(cli.overrides.allow_private_source_urls, Some(true));
    for flag in ["--help", "--version"] {
        let error = Cli::try_parse_from(["booru-thingy", "--config", "missing.toml", flag])
            .err()
            .unwrap();
        assert!(matches!(
            error.kind(),
            clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
        ));
    }
}

#[test]
fn every_setting_has_consistent_names_and_round_trips_through_cli() {
    use clap::CommandFactory;

    let example = format!("{EXAMPLE}\ns3_public_endpoint = 'https://media.example.com'\n");
    let values = serde_json::to_value(parse(&example)).unwrap();
    let command = Cli::command();
    let mut args = vec!["booru-thingy".to_owned(), "serve".to_owned()];
    for field in Config::META.fields {
        let name = field.name;
        let flag = name.replace('_', "-");
        let argument = command
            .get_arguments()
            .find(|arg| arg.get_id().as_str() == name)
            .unwrap();
        assert_eq!(argument.get_long(), Some(flag.as_str()));
        match field.kind {
            confique::meta::FieldKind::Leaf { env, .. } => {
                assert_eq!(env, Some(name.to_uppercase().as_str()));
            }
            _ => panic!("expected a flat configuration setting"),
        }
        args.push(format!("--{flag}"));
        args.push(match &values[name] {
            serde_json::Value::String(value) => value.clone(),
            value => value.to_string(),
        });
    }
    let cli = Cli::try_parse_from(args).unwrap();
    assert_eq!(serde_json::to_value(&cli.overrides).unwrap(), values);
    Config::from_layer(cli.overrides)
        .unwrap()
        .validate(cli.command)
        .unwrap();
    // The file selector is a process option, not an application configuration key.
    assert!(toml::from_str::<ConfigLayer>("config = 'other.toml'").is_err());
}

#[test]
fn migration_requirements_and_validation_remain_mode_aware() {
    let layer = parse("database_url = 'postgres://localhost/booru'");
    let config = Config::from_layer(layer.with_fallback(ConfigLayer::default_values())).unwrap();
    config.clone().validate(Some(Command::Migrate)).unwrap();
    for mode in [
        Some(Command::Run),
        Some(Command::Serve),
        Some(Command::Worker),
    ] {
        assert!(config.clone().validate(mode).is_err());
    }
    assert!(Config::default().validate(Some(Command::Migrate)).is_err());
    for text in [
        "worker_concurrency = 0",
        "worker_concurrency = 33",
        "media_url_ttl_seconds = 604801",
        "upload_ticket_ttl_seconds = 0",
        "upload_ticket_ttl_seconds = 3601",
        "max_upload_bytes = 0",
        "process_timeout_seconds = 0",
        "s3_public_endpoint = ''",
        "base_url = 'https://user:sensitive-value@example.com'",
        "rust_log = '[sensitive-value'",
    ] {
        let config = Config::from_layer(parse(text).with_fallback(parse(EXAMPLE))).unwrap();
        let error = config.validate(None).err().unwrap();
        assert!(!format!("{error:#}").contains("sensitive-value"));
    }
    for text in [
        "s3_access_key = 'old'",
        "s3_secret_key = 'old'",
        "media_url_ttl = 1",
        "process_timeout = 1",
        "log_filter = 'info'",
    ] {
        assert!(toml::from_str::<ConfigLayer>(text).is_err());
    }
}

// Exercise the real Confique environment reader without altering global state.
// Each case runs this helper in a child test process with a clean environment.
#[test]
fn environment_child() {
    let Ok(case) = std::env::var("CONFIG_TEST_CASE") else {
        return;
    };
    match case.as_str() {
        "valid" => {
            let config =
                Config::from_layer(environment_layer().unwrap().with_fallback(parse(EXAMPLE)))
                    .unwrap();
            assert!(!config.s3_path_style);
            assert_eq!(config.worker_concurrency, 4);
            assert_eq!(config.aws_access_key_id, "0000123");
            assert_eq!(config.aws_secret_access_key, "true");
            assert_eq!(config.rust_log, "warn");
            assert_eq!(config.owner_api_key, "");
            assert!(config.validate(None).is_err());
        }
        "env-only" => {
            let config = Config::from_layer(
                environment_layer()
                    .unwrap()
                    .with_fallback(ConfigLayer::default_values()),
            )
            .unwrap();
            config.validate(None).unwrap();
        }
        "selector" => {
            assert_eq!(
                Cli::try_parse_from(["booru-thingy", "run"])
                    .unwrap()
                    .config
                    .unwrap(),
                PathBuf::from("env.toml")
            );
            assert_eq!(
                cli(&["--config", "cli.toml"]).config.unwrap(),
                PathBuf::from("cli.toml")
            );
        }
        "discovery" => {
            let config = Config::load(&cli(&[
                "--config",
                "explicit.toml",
                "serve",
                "--worker-concurrency",
                "6",
            ]))
            .unwrap();
            assert_eq!(config.worker_concurrency, 6);
            assert_eq!(config.max_upload_bytes, 1234);
            assert_eq!(config.base_url, "https://cwd.example.com");
            assert_eq!(config.rust_log, "warn");
            assert_eq!(config.aws_secret_access_key, "local-secret-key");
        }
        "error" => {
            let error = environment_layer().err().unwrap();
            assert!(!format!("{error:#}").contains("sensitive-value"));
        }
        other => panic!("unknown case {other}"),
    }
}

#[test]
fn real_loader_discovers_files_and_merges_with_environment_and_cli() {
    let dir = tempfile::tempdir().unwrap();
    let xdg = dir.path().join("xdg");
    fs::create_dir_all(xdg.join("booru-thingy")).unwrap();
    fs::write(xdg.join("booru-thingy/config.toml"), EXAMPLE).unwrap();
    fs::write(
        dir.path().join("config.toml"),
        "base_url = 'https://cwd.example.com'\nworker_concurrency = 3",
    )
    .unwrap();
    fs::write(
        dir.path().join("explicit.toml"),
        "max_upload_bytes = 1234\nworker_concurrency = 4",
    )
    .unwrap();
    let output = Process::new(std::env::current_exe().unwrap())
        .env_clear()
        .env("CONFIG_TEST_CASE", "discovery")
        .env("XDG_CONFIG_HOME", &xdg)
        .env("WORKER_CONCURRENCY", "5")
        .env("RUST_LOG", "warn")
        .current_dir(dir.path())
        .args(["--exact", "config::tests::environment_child", "--nocapture"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn environment_case(case: &str, vars: &[(&str, &str)]) {
    let output = Process::new(std::env::current_exe().unwrap())
        .env_clear()
        .env("CONFIG_TEST_CASE", case)
        .envs(vars.iter().copied())
        .args(["--exact", "config::tests::environment_child", "--nocapture"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn environment_parsing_preserves_types_and_rejects_invalid_values() {
    environment_case(
        "valid",
        &[
            ("S3_PATH_STYLE", "false"),
            ("WORKER_CONCURRENCY", "4"),
            ("OWNER_API_KEY", ""),
            ("AWS_ACCESS_KEY_ID", "0000123"),
            ("AWS_SECRET_ACCESS_KEY", "true"),
            ("RUST_LOG", "warn"),
        ],
    );
    environment_case(
        "env-only",
        &[
            ("DATABASE_URL", "postgres://localhost/booru"),
            ("OWNER_API_KEY", "test-key"),
            ("S3_ENDPOINT", "http://localhost:9000"),
            ("S3_BUCKET", "booru"),
            ("AWS_ACCESS_KEY_ID", "access"),
            ("AWS_SECRET_ACCESS_KEY", "secret"),
        ],
    );
    environment_case("selector", &[("BOORU_CONFIG", "env.toml")]);
    for key in ["WORKER_CONCURRENCY", "LISTEN_ADDR", "S3_PATH_STYLE"] {
        environment_case("error", &[(key, "")]);
        environment_case("error", &[(key, "sensitive-value")]);
    }
}
