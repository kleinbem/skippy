use super::*;
use anyhow::{Context, Result};
use skippy_protocol::{MessageBase, StageConfig, StageKvCacheConfig, StageKvCacheMode};
use skippy_runtime::{MemoryCacheCapabilities, ModelStateKind};

fn config(capacity: u32) -> StageConfig {
    StageConfig {
        ctx_size: capacity,
        lane_count: 1,
        layer_end: 2,
        n_batch: Some(512),
        n_ubatch: Some(64),
        n_gpu_layers: 0,
        kv_cache: Some(StageKvCacheConfig {
            mode: StageKvCacheMode::LookupRecord,
            max_entries: 32,
            max_bytes: 0,
            l2_max_bytes: 0,
            min_tokens: 1,
            shared_prefix_stride_tokens: 32,
            shared_prefix_record_limit: 32,
            exact_max_bytes: None,
            codec: skippy_protocol::StageKvCacheCodec::Native,
            prefill_admission: Default::default(),
            retention: Default::default(),
        }),
        ..StageConfig::default()
    }
}

fn integration(config: &StageConfig) -> Result<KvStageIntegration> {
    KvStageIntegration::from_loaded_model(
        config,
        Some(ModelStateKind::Dense),
        Some(MemoryCacheCapabilities {
            resident: true,
            resident_checkpoint: false,
            resident_attention: true,
            kv_recurrent: true,
            full_state: true,
        }),
        None,
    )?
    .context("resident KV integration required")
}

#[test]
fn prefill_keeps_a_healthy_resident_cache() -> Result<()> {
    let config = config(512);
    let kv = integration(&config)?;
    let seq_id = lock_resident_sequences(&kv.resident_sequences).allocate()?;
    kv.radix.lock().unwrap().insert_resident(
        "stage",
        &[1; 64],
        64,
        RadixResidentEntry {
            page_id: "prefix".into(),
            seq_id,
            token_count: 64,
            recompute_cost: 64,
        },
    )?;
    let mut runtime =
        crate::runtime_state::RuntimeState::new_modelless_with_capacity_for_test(1, 512);
    let result = kv.evict_resident_prefix_for_prefill(&mut runtime, "request", 128)?;
    assert_eq!(result.evicted_entries, 0);
    assert_eq!(kv.radix.lock().unwrap().stats().resident_entries, 1);
    Ok(())
}

#[test]
fn prefill_can_fill_the_pool_without_a_decode_batch_reserve() -> Result<()> {
    let kv = integration(&config(512))?;
    let mut runtime =
        crate::runtime_state::RuntimeState::new_modelless_with_capacity_for_test(1, 512);
    let result = kv.evict_resident_prefix_for_prefill(&mut runtime, "request", 512)?;
    assert_eq!(result.evicted_entries, 0);
    Ok(())
}

#[test]
fn prefill_rejects_demand_that_cannot_fit_the_pool() -> Result<()> {
    let kv = integration(&config(512))?;
    let mut runtime =
        crate::runtime_state::RuntimeState::new_modelless_with_capacity_for_test(1, 512);
    let error = kv
        .evict_resident_prefix_for_prefill(&mut runtime, "request", 513)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("insufficient physical KV capacity")
    );
    Ok(())
}

#[test]
#[ignore = "requires SKIPPY_RESIDENT_PREFILL_MODEL pointing to the two-layer native Qwen3 dense fixture"]
fn shared_checkpoint_aliases_release_physical_capacity_before_prefill() -> Result<()> {
    let mut config = config(512);
    config.model_path = Some(std::env::var("SKIPPY_RESIDENT_PREFILL_MODEL").context(
        "set SKIPPY_RESIDENT_PREFILL_MODEL to the two-layer native Qwen3 dense fixture",
    )?);
    let runtime = crate::runtime_state::load_runtime(&config)?.context("native model required")?;
    let mut runtime = runtime.lock().unwrap();
    let kv = integration(&config)?;
    let base = MessageBase {
        schema_version: 1,
        run_id: "capacity".into(),
        request_id: "warm".into(),
        session_id: "warm".into(),
        stage_id: config.stage_id.clone(),
        stage_index: 0,
        topology_id: config.topology_id.clone(),
        model_id: None,
        tokenizer_id: None,
        chat_template_id: None,
        seq: None,
    };
    let tokens = vec![7; 256];
    runtime.prefill("warm", &tokens)?;
    for length in (32..=256).step_by(32) {
        let identity = kv.prefill_identity(&config, &base, 0, &tokens[..length]);
        kv.record_resident_prefix(&mut runtime, "warm", &identity, &tokens[..length])?
            .context("checkpoint must be stored")?;
    }
    runtime.drop_session_timed("warm")?;
    runtime.ensure_session_active("fresh")?;
    assert_eq!(runtime.memory_used_cells("fresh")?, 256);
    // Logical checkpoint depths can be evicted without releasing the cells
    // still referenced by the longest checkpoint.
    kv.evict_resident_prefix_for_tokens(&mut runtime, "fresh", 384)?;
    assert!(runtime.memory_used_cells("fresh")? + 384 > 512);
    kv.evict_resident_prefix_for_prefill(&mut runtime, "fresh", 384)?;
    assert!(runtime.memory_used_cells("fresh")? + 384 <= 512);
    runtime.prefill("fresh", &vec![8; 384])?;
    runtime.drop_session_timed("fresh")?;
    Ok(())
}
