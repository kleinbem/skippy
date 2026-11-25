use std::{ffi::OsString, path::Path};

use super::*;
use crate::{cli::Command, model_source};

fn write_gguf(path: &Path) {
    let mut bytes = Vec::from(*b"GGUF");
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.extend_from_slice(&4u64.to_le_bytes());
    let append_string = |bytes: &mut Vec<u8>, value: &str| {
        bytes.extend_from_slice(&(value.len() as u64).to_le_bytes());
        bytes.extend_from_slice(value.as_bytes());
    };
    append_string(&mut bytes, "general.architecture");
    bytes.extend_from_slice(&8u32.to_le_bytes());
    append_string(&mut bytes, "llama");
    for (key, value) in [
        ("llama.block_count", 2u32),
        ("llama.embedding_length", 128),
        ("llama.context_length", 4096),
    ] {
        append_string(&mut bytes, key);
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

#[tokio::test]
async fn cached_hub_reference_and_local_path_get_the_same_ids_as_single_serve() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tiny.gguf");
    write_gguf(&path);
    let reference = "skippy-serving-fixture/tiny-GGUF@abc123:tiny.gguf";
    skippy_model_hf::store::local::remember_model_ref_path(reference, &path);
    let settings = directory.path().join("serve.toml");
    std::fs::write(
        &settings,
        format!(
            "version = 1\n[[models]]\nmodel = {reference:?}\n\
             [[models]]\nmodel = './tiny.gguf'\n\
             [[models]]\nid = 'custom'\nmodel_path = 'tiny.gguf'\n"
        ),
    )
    .unwrap();
    let cli = crate::serve_settings::parse_with_environment(
        [
            OsString::from("skippy"),
            OsString::from("serve"),
            OsString::from("--settings"),
            settings.into_os_string(),
        ],
        |_| None,
    )
    .unwrap();
    let Command::Serve(mut args) = cli.command else {
        panic!("expected serve");
    };
    for model in &mut args.local_models {
        model_source::resolve(model).await.unwrap();
        assert_eq!(model.model_path.as_deref(), Some(path.as_path()));
    }

    let cli = crate::serve_settings::parse_with_environment(
        [
            OsString::from("skippy"),
            OsString::from("serve"),
            OsString::from("--model"),
            path.into_os_string(),
        ],
        |_| None,
    )
    .unwrap();
    let Command::Serve(mut single) = cli.command else {
        panic!("expected serve");
    };
    model_source::resolve(&mut single.public).await.unwrap();
    let single = local_openai_options(single.public).unwrap();
    let prepared = prepare_models(args.local_models).unwrap();
    assert_eq!(prepared[0].id(), reference);
    assert_eq!(prepared[1].id(), single.config.model_id);
    assert_eq!(prepared[1].id(), "tiny");
    assert_eq!(prepared[2].id(), "custom");
}
