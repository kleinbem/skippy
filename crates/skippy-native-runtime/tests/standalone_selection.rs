use skippy_native_runtime::RuntimeSelection;

#[test]
fn accepts_standalone_runtime_ids() {
    let id = "skippy-native-runtime-linux-x86_64-cuda12";
    assert_eq!(
        RuntimeSelection::parse(Some(id)).unwrap(),
        RuntimeSelection::Id(id.to_owned())
    );
}

#[test]
fn retains_explicit_and_legacy_runtime_ids() {
    for id in ["meshllm-runtime-linux-x86_64-cpu", "custom-cpu-runtime"] {
        assert_eq!(
            RuntimeSelection::parse(Some(&format!("exact:{id}"))).unwrap(),
            RuntimeSelection::Id(id.to_owned())
        );
    }
}
