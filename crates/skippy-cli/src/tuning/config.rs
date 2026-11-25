use std::{io::Write, path::Path};

use anyhow::{Context, Result, ensure};
use skippy_api::tuning::{FlashAttention, SettingsPatch, Speculation};
use toml::{Table, Value};

pub fn options(patch: &SettingsPatch) -> Vec<(&'static str, &'static str, Value)> {
    let flash = match patch.flash_attention {
        FlashAttention::Auto => "auto",
        FlashAttention::Enabled => "enabled",
        FlashAttention::Disabled => "disabled",
    };
    let (strategy, draft) = match &patch.speculative {
        Speculation::Auto => ("auto", None),
        Speculation::Disabled => ("disabled", None),
        Speculation::NativeMtp => ("native-mtp", None),
        Speculation::Ngram => ("ngram", None),
        Speculation::DraftModel { path } => ("draft-model", Some(path)),
        Speculation::Dflash { path } => ("dflash", Some(path)),
    };
    let mut values = vec![
        ("kv", "ctx_size", Value::from(i64::from(patch.context))),
        (
            "execution",
            "batch_size",
            Value::from(i64::from(patch.batch)),
        ),
        (
            "execution",
            "microbatch_size",
            Value::from(i64::from(patch.microbatch)),
        ),
        ("model", "mmap", Value::from(patch.mmap)),
        ("model", "mlock", Value::from(patch.mlock)),
        ("kv", "flash_attention", Value::from(flash)),
        ("speculative", "speculative_strategy", Value::from(strategy)),
    ];
    if let Some(draft) = draft {
        values.push((
            "speculative",
            "draft_model_path",
            Value::from(draft.to_string_lossy().as_ref()),
        ));
    }
    values
}

pub fn apply(path: &Path, patch: &SettingsPatch) -> Result<()> {
    let mut original = std::fs::File::open(path)
        .with_context(|| format!("open serving configuration {}", path.display()))?;
    let file_metadata = original.metadata()?;
    ensure!(
        file_metadata.is_file() && !std::fs::symlink_metadata(path)?.file_type().is_symlink(),
        "apply requires a regular serving config file"
    );
    let mut document = String::new();
    std::io::Read::read_to_string(&mut original, &mut document)?;
    let mut table: Table = toml::from_str(&document)?;
    patch_table(&mut table, patch)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(toml::to_string_pretty(&table)?.as_bytes())?;
    temporary
        .as_file()
        .set_permissions(file_metadata.permissions())?;
    temporary.as_file().sync_all()?;
    ensure!(
        std::fs::read_to_string(path)? == document,
        "serving configuration changed during tuning apply"
    );
    temporary.persist(path).map_err(|failure| failure.error)?;
    Ok(())
}

fn patch_table(table: &mut Table, patch: &SettingsPatch) -> Result<()> {
    ensure!(
        table
            .get("version")
            .is_none_or(|version| version.as_integer() == Some(1)),
        "apply requires a version 1 standalone serving configuration"
    );
    ensure!(
        !table.contains_key("models"),
        "apply requires a single-model serving configuration"
    );
    if let Some(speculative) = table.get_mut("speculative").and_then(Value::as_table_mut) {
        // Drop previously chosen speculative settings that would override the
        // selected strategy or keep a different draft active.
        speculative.retain(|key, _| {
            !matches!(
                key.replace('-', "_").as_str(),
                "speculative_strategy"
                    | "speculative_config"
                    | "native_mtp"
                    | "draft_model_path"
                    | "native_mtp_draft_model_path"
            )
        });
    }
    for (section, name, value) in options(patch) {
        let section = table
            .entry(section.to_owned())
            .or_insert_with(|| Value::Table(Table::new()))
            .as_table_mut()
            .context("serving configuration section must be a table")?;
        section.remove(&name.replace('_', "-"));
        section.insert(name.to_owned(), value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_replaces_alias_spellings_and_previous_strategy() {
        let original = r#"
version = 1
[api]
model_id = "kept"
[execution]
batch-size = 99
[speculative]
draft_model_path = "obsolete"
native-mtp = true
"#;
        let mut table: Table = toml::from_str(original).unwrap();
        let patch = SettingsPatch {
            context: 1024,
            batch: 128,
            microbatch: 64,
            mmap: false,
            mlock: false,
            flash_attention: FlashAttention::Disabled,
            speculative: Speculation::Disabled,
        };
        patch_table(&mut table, &patch).unwrap();
        assert_eq!(table["execution"]["batch_size"].as_integer(), Some(128));
        assert!(table["execution"].get("batch-size").is_none());
        assert!(table["speculative"].get("native-mtp").is_none());
        assert!(table["speculative"].get("draft_model_path").is_none());
        assert_eq!(table["api"]["model_id"].as_str(), Some("kept"));
    }
}
