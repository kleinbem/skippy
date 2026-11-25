//! Layered serving defaults and per-model operator overrides.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::{Serialize, Serializer};
use serde_json::Value;
use skippy_protocol::{
    FlashAttentionType, StageConfig, StageDevice, StageKvCacheConfig, StageKvCacheMode,
};
use skippy_serving::{
    SpeculativeDecodeConfig, frontend::EmbeddedOpenAiRequestDefaults, settings::ServingTuning,
};

/// Keep an inherited value, set a value, or restore the base profile's value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Setting<T> {
    #[default]
    Inherit,
    Set(T),
    Reset,
}

impl<T> Setting<T> {
    fn inherited(&self) -> bool {
        matches!(self, Self::Inherit)
    }
}

// Only used to map the typed StageOverrides onto the same stage names the CLI
// accepts. Omission means inherit; null selects the original base value.
impl<T: Serialize> Serialize for Setting<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Set(value) => value.serialize(serializer),
            Self::Reset | Self::Inherit => serializer.serialize_none(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct StageOverrides {
    #[serde(skip_serializing_if = "Setting::inherited")]
    pub ctx_size: Setting<u32>,
    #[serde(skip_serializing_if = "Setting::inherited")]
    pub lane_count: Setting<u32>,
    #[serde(skip_serializing_if = "Setting::inherited")]
    pub n_gpu_layers: Setting<i32>,
    #[serde(skip_serializing_if = "Setting::inherited")]
    pub n_batch: Setting<u32>,
    #[serde(skip_serializing_if = "Setting::inherited")]
    pub n_ubatch: Setting<u32>,
    #[serde(skip_serializing_if = "Setting::inherited")]
    pub mmap: Setting<bool>,
    #[serde(skip_serializing_if = "Setting::inherited")]
    pub mlock: Setting<bool>,
    #[serde(skip_serializing_if = "Setting::inherited")]
    pub selected_device: Setting<StageDevice>,
    #[serde(skip_serializing_if = "Setting::inherited")]
    pub main_gpu: Setting<u32>,
    #[serde(skip_serializing_if = "Setting::inherited")]
    pub flash_attn_type: Setting<FlashAttentionType>,
    #[serde(skip_serializing_if = "Setting::inherited")]
    pub kv_offload: Setting<bool>,
    #[serde(skip_serializing_if = "Setting::inherited")]
    pub cache_type_k: Setting<String>,
    #[serde(skip_serializing_if = "Setting::inherited")]
    pub cache_type_v: Setting<String>,
    #[serde(skip_serializing_if = "Setting::inherited")]
    pub kv_cache: Setting<StageKvCacheConfig>,
    #[serde(skip_serializing_if = "Setting::inherited")]
    pub native_mtp_enabled: Setting<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct ProfileOverrides {
    pub stage: StageOverrides,
    pub n_threads: Setting<usize>,
    pub n_threads_batch: Setting<usize>,
    pub continuous_batching: Setting<bool>,
    pub pipeline_decode_groups: Setting<usize>,
    pub request_defaults: Setting<EmbeddedOpenAiRequestDefaults>,
    pub speculative: Setting<SpeculativeDecodeConfig>,
}

#[derive(Clone, Debug, Default)]
pub struct ServingProfile {
    pub stage: StageConfig,
    pub tuning: ServingTuning,
    pub speculative: SpeculativeDecodeConfig,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueLayer {
    Base,
    Shared,
    Model,
    SharedReset,
    ModelReset,
}

pub struct ResolvedServingProfile {
    pub profile: ServingProfile,
    /// Keys use StageConfig names or the documented frontend option names.
    pub sources: BTreeMap<String, ValueLayer>,
}

/// Resolve model overrides over shared defaults and the supplied base stage.
/// Settings absent in both layers retain the base. A reset uses the original
/// base even when the shared layer specified a value.
pub fn resolve(
    base: &ServingProfile,
    shared: &ProfileOverrides,
    model: &ProfileOverrides,
) -> Result<ResolvedServingProfile> {
    let mut sources = BTreeMap::new();
    let stage = resolve_stage(base, shared, model, &mut sources)?;
    let mut tuning = base.tuning.clone();
    macro_rules! tuning_option {
        ($field:ident) => {
            tuning.$field = resolve_value(
                tuning.$field,
                &shared.$field,
                &model.$field,
                Some,
                stringify!($field),
                &mut sources,
            );
        };
    }
    tuning_option!(n_threads);
    tuning_option!(n_threads_batch);
    tuning_option!(continuous_batching);
    tuning_option!(pipeline_decode_groups);
    tuning.request_defaults = resolve_value(
        tuning.request_defaults,
        &shared.request_defaults,
        &model.request_defaults,
        |value| value,
        "request_defaults",
        &mut sources,
    );
    tuning.validate()?;
    let speculative = resolve_value(
        base.speculative.clone(),
        &shared.speculative,
        &model.speculative,
        |value| value,
        "speculative",
        &mut sources,
    );
    speculative.validate()?;
    Ok(ResolvedServingProfile {
        profile: ServingProfile {
            stage,
            tuning,
            speculative,
        },
        sources,
    })
}

fn resolve_stage(
    base: &ServingProfile,
    shared: &ProfileOverrides,
    model: &ProfileOverrides,
    sources: &mut BTreeMap<String, ValueLayer>,
) -> Result<StageConfig> {
    let original = serde_json::to_value(&base.stage)?;
    let mut stage = original.clone();
    for (overrides, layers) in [
        (&shared.stage, (ValueLayer::Shared, ValueLayer::SharedReset)),
        (&model.stage, (ValueLayer::Model, ValueLayer::ModelReset)),
    ] {
        let overrides = serde_json::to_value(overrides)?;
        for (key, value) in overrides
            .as_object()
            .context("stage overrides must be an object")?
        {
            let (source, value) = if value.is_null() {
                (layers.1, original[key].clone())
            } else {
                (layers.0, value.clone())
            };
            stage[key] = value;
            sources.insert(key.clone(), source);
        }
    }
    if let Some(fields) = original.as_object() {
        for key in fields.keys() {
            sources.entry(key.clone()).or_insert(ValueLayer::Base);
        }
    }
    let stage: StageConfig = serde_json::from_value(stage).context("invalid runtime settings")?;
    validate_stage(&stage)?;
    Ok(stage)
}

fn resolve_value<T: Clone, V>(
    base: V,
    shared: &Setting<T>,
    model: &Setting<T>,
    convert: impl FnOnce(T) -> V,
    name: &str,
    sources: &mut BTreeMap<String, ValueLayer>,
) -> V {
    let (source, value) = match (model, shared) {
        (Setting::Set(value), _) => (ValueLayer::Model, convert(value.clone())),
        (Setting::Reset, _) => (ValueLayer::ModelReset, base),
        (_, Setting::Set(value)) => (ValueLayer::Shared, convert(value.clone())),
        (_, Setting::Reset) => (ValueLayer::SharedReset, base),
        _ => (ValueLayer::Base, base),
    };
    sources.insert(name.to_owned(), source);
    value
}

/// Apply the legacy standalone stage patch using the same stage validation
/// as typed profile resolution. The caller resolves CLI, environment and TOML
/// precedence before invoking this boundary.
pub fn apply_stage_overrides(base: &StageConfig, patch: Value) -> Result<StageConfig> {
    let mut base = base.clone();
    if patch.get("kv_cache").is_some() && base.kv_cache.is_none() {
        base.kv_cache = crate::family_policy::family_policy_for_stage_config(&base)
            .stage_kv_cache_config_for_stage(&base);
    }
    let mut value = serde_json::to_value(base)?;
    merge(&mut value, patch);
    let config = serde_json::from_value(value).context("invalid runtime settings")?;
    validate_stage(&config)?;
    Ok(config)
}

pub fn merge(base: &mut Value, patch: Value) {
    if let (Some(base), Value::Object(patch)) = (base.as_object_mut(), &patch) {
        for (key, value) in patch {
            merge(
                base.entry(key.clone()).or_insert(Value::Null),
                value.clone(),
            );
        }
    } else {
        *base = patch;
    }
}
fn validate_stage(config: &StageConfig) -> Result<()> {
    skippy_runtime::parse_cache_type(&config.cache_type_k)?;
    skippy_runtime::parse_cache_type(&config.cache_type_v)?;
    anyhow::ensure!(
        config.n_batch != Some(0) && config.n_ubatch != Some(0),
        "batch and microbatch sizes must be positive"
    );
    anyhow::ensure!(
        config.n_gpu_layers >= -1,
        "n-gpu-layers must be -1 or nonnegative"
    );
    anyhow::ensure!(
        config
            .image_min_tokens
            .zip(config.image_max_tokens)
            .is_none_or(|(min, max)| min <= max),
        "image-min-tokens must not exceed image-max-tokens"
    );
    anyhow::ensure!(
        config
            .activation_codec_policy
            .compatible(config.activation_codec),
        "auto-lossless-v1 requires raw-f32-v1 as its fallback codec"
    );
    if let Some(cache) = &config.kv_cache {
        anyhow::ensure!(
            cache.mode == StageKvCacheMode::Disabled || cache.max_entries > 0,
            "prefix-cache-max-entries must be positive; use --prefix-cache off to disable caching"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
