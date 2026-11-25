use std::ffi::OsString;

use super::*;
use crate::{cli::Cli, serve_settings::parse_with_environment};

fn parse(
    contents: &str,
    overrides: &[&str],
    environment: impl Fn(&str) -> Option<String>,
) -> Result<(tempfile::TempDir, Cli)> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("serve.toml");
    std::fs::write(&path, contents)?;
    let args = [
        OsString::from("skippy"),
        OsString::from("serve"),
        OsString::from("--settings"),
        path.into_os_string(),
    ]
    .into_iter()
    .chain(overrides.iter().map(OsString::from));
    Ok((directory, parse_with_environment(args, environment)?))
}

#[test]
fn serve_resolves_multiple_local_models_from_one_settings_file() {
    let (directory, cli) = parse(
        r#"
version = 1
bind_addr = "127.0.0.1:8080"

[runtime]
runtime_bundle = ["runtime"]

[defaults]
ctx_size = 4096
threads = 2
temperature = 0.2

[[models]]
id = "small"
model_path = "models/small.gguf"

[models.settings]
ctx_size = 2048
temperature = 0.5

[[models]]
id = "large"
model_path = "models/large.gguf"
"#,
        &["--temperature", "0.8", "--print-effective-config"],
        |name| match name {
            "SKIPPY_SERVE_BIND_ADDR" => Some("127.0.0.1:0".into()),
            "SKIPPY_SERVE_THREADS" => Some("4".into()),
            _ => None,
        },
    )
    .unwrap();

    assert_eq!(
        cli.native_runtime.bundle_dirs,
        [directory.path().join("runtime")]
    );
    let Command::Serve(args) = cli.command else {
        panic!("expected serve");
    };
    assert!(args.print_effective_config);
    assert_eq!(args.public.bind_addr.unwrap().port(), 0);
    assert_eq!(args.local_models.len(), 2);
    for (model, id, ctx_size) in [
        (&args.local_models[0], "small", 2048),
        (&args.local_models[1], "large", 4096),
    ] {
        assert_eq!(model.model_id.as_deref(), Some(id));
        assert_eq!(
            model.model_path,
            Some(directory.path().join(format!("models/{id}.gguf")))
        );
        assert_eq!(model.ctx_size, Some(ctx_size));
        assert_eq!(model.settings.values["threads"], 4);
        assert_eq!(model.settings.values["temperature"], 0.8);
        assert_eq!(model.settings.sources["temperature"], "cli");
    }
}

#[test]
fn serve_rejects_duplicate_ids_and_conflicting_launch_modes() {
    let file = "version = 1\n[[models]]\nid = 'small'\nmodel_path = 'small.gguf'\n";
    let duplicate = format!("{file}[[models]]\nid = 'small'\nmodel_path = 'other.gguf'\n");
    let error = parse(&duplicate, &[], |_| None).err().unwrap();
    assert!(error.to_string().contains("duplicate served model ID"));

    for flags in [
        &["--model", "other.gguf"][..],
        &["--model-id", "other"],
        &["--config", "stage.json"],
        &["--prompt"],
    ] {
        let error = parse(file, flags, |_| None).err().unwrap();
        assert!(
            error
                .to_string()
                .contains("cannot be used with a multi-model settings file")
        );
    }
}

#[test]
fn serve_uses_the_same_model_values_in_both_settings_forms() {
    for reference in [
        "repo/Qwen3-6B:Q4_K_M",
        "repo/Qwen3-6B@revision:weights/Qwen3-6B-Q4_K_M.gguf",
        "repo/Qwen3-6B@revision:Qwen3-6B-Q4_K_M.gguf",
        "Qwen3-0.6B-Q4_K_M",
        "./local/qwen.gguf",
        "qwen.gguf",
    ] {
        for (section, multi_model) in [("[model]", false), ("[[models]]", true)] {
            let file = format!("version = 1\n{section}\nmodel = {reference:?}\n");
            let (directory, cli) = parse(&file, &[], |_| None).unwrap();
            let Command::Serve(args) = cli.command else {
                panic!("expected serve");
            };
            let model = if multi_model {
                &args.local_models[0]
            } else {
                &args.public
            };
            let expected = match reference {
                "./local/qwen.gguf" | "qwen.gguf" => directory.path().join(reference),
                _ => PathBuf::from(reference),
            };
            assert_eq!(model.model.as_deref(), expected.to_str());
            assert!(model.model_path.is_none());
            assert!(model.model_id.is_none());
        }
    }
}

#[test]
fn model_path_is_relative_to_either_settings_form() {
    for (section, multi_model) in [("[model]", false), ("[[models]]", true)] {
        let file = format!("version = 1\n{section}\nmodel_path = 'models/local.gguf'\n");
        let (directory, cli) = parse(&file, &[], |_| None).unwrap();
        let Command::Serve(args) = cli.command else {
            panic!("expected serve");
        };
        let model = if multi_model {
            &args.local_models[0]
        } else {
            &args.public
        };
        assert_eq!(
            model.model_path,
            Some(directory.path().join("models/local.gguf"))
        );
        assert!(model.model.is_none());
    }
}

#[test]
fn serve_rejects_ambiguous_or_legacy_multi_model_sources() {
    for (file, expected) in [
        (
            "[[models]]\nmodel = 'some/repo'\nmodel_path = 'local.gguf'",
            "exactly one of model or model_path",
        ),
        ("[[models]]\nid = 'a'", "exactly one of model or model_path"),
        ("[[models]]\nmodel = ' '", "must not be empty"),
        ("[[models]]\npath = 'local.gguf'", "unknown field `path`"),
        (
            "[model]\nmodel = 'some/repo'\n[[models]]\nmodel = 'other/repo'",
            "either [model] or [[models]]",
        ),
    ] {
        let file = format!("version = 1\n{file}");
        let error = parse(&file, &[], |_| None).err().unwrap();
        assert!(format!("{error:#}").contains(expected), "{error:#}");
    }

    let file = "version = 1\n[[models]]\nmodel = 'some/repo'";
    let error = parse(file, &[], |name| {
        (name == "SKIPPY_SERVE_MODEL").then(|| "other/repo".into())
    })
    .err()
    .unwrap();
    assert!(error.to_string().contains("--model cannot be used"));
}
