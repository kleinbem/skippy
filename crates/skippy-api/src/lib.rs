//! Shared model preparation for standalone and embedded Skippy serving.
pub mod family_policy;
pub mod hardware;
pub mod kv_cache;
pub mod speculative;
pub mod stage;
pub use stage::{SingleStageOptions, StageSourceIdentity, single_stage_config};

pub mod hash_cache;
pub mod package;
pub mod placement;

pub mod source;
pub mod source_registry;

pub mod materialization;
pub mod materialized_cache;
pub mod split_execution;
pub mod stage_admission;
pub mod stage_load;

pub mod native_runtime;
pub mod serving;
pub mod serving_profile;

#[cfg(feature = "dynamic-native-runtime")]
pub mod execution;

pub mod tuning;
