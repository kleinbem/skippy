use super::*;

#[test]
fn shared_defaults_model_overrides_and_base_resets_preserve_false_and_zero() {
    let mut base = ServingProfile::default();
    base.stage.n_gpu_layers = -1;
    base.stage.n_batch = Some(64);
    let shared = ProfileOverrides {
        stage: StageOverrides {
            n_gpu_layers: Setting::Set(20),
            n_batch: Setting::Set(128),
            mlock: Setting::Set(true),
            ..Default::default()
        },
        n_threads: Setting::Set(4),
        continuous_batching: Setting::Set(true),
        ..Default::default()
    };
    let model = ProfileOverrides {
        stage: StageOverrides {
            n_gpu_layers: Setting::Set(0),
            n_batch: Setting::Reset,
            mlock: Setting::Set(false),
            ..Default::default()
        },
        continuous_batching: Setting::Set(false),
        ..Default::default()
    };
    let resolved = resolve(&base, &shared, &model).unwrap();
    assert_eq!(resolved.profile.stage.n_gpu_layers, 0);
    assert_eq!(resolved.profile.stage.n_batch, Some(64));
    assert!(!resolved.profile.stage.mlock);
    assert_eq!(resolved.profile.tuning.continuous_batching, Some(false));
    assert_eq!(resolved.profile.tuning.n_threads, Some(4));
    assert_eq!(resolved.sources["n_batch"], ValueLayer::ModelReset);
    assert_eq!(resolved.sources["mlock"], ValueLayer::Model);
    assert_eq!(resolved.sources["n_threads"], ValueLayer::Shared);
}

#[test]
fn legacy_and_typed_settings_agree_and_reject_invalid_batches() {
    let base = ServingProfile::default();
    let model = ProfileOverrides {
        stage: StageOverrides {
            n_batch: Setting::Set(128),
            mlock: Setting::Set(false),
            ..Default::default()
        },
        ..Default::default()
    };
    let resolved = resolve(&base, &ProfileOverrides::default(), &model).unwrap();
    let legacy = apply_stage_overrides(
        &base.stage,
        serde_json::json!({"n_batch":128,"mlock":false}),
    )
    .unwrap();
    assert_eq!(resolved.profile.stage, legacy);
    let invalid = ProfileOverrides {
        stage: StageOverrides {
            n_batch: Setting::Set(0),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(resolve(&base, &ProfileOverrides::default(), &invalid).is_err());
    assert!(apply_stage_overrides(&base.stage, serde_json::json!({"n_batch":0})).is_err());
}
