use super::*;

fn gpu(id: Option<&str>) -> HostGpuProfile {
    HostGpuProfile {
        display_name: "Identical model".to_owned(),
        stable_id: id.map(str::to_owned),
        ..Default::default()
    }
}

fn runtime(id: Option<&str>) -> RuntimeDevice {
    RuntimeDevice {
        selector: "CUDA0".to_owned(),
        identifier: id.map(str::to_owned),
        gpu: true,
        integrated: false,
        total_bytes: Some(10_000),
    }
}

#[test]
fn identical_models_are_attributed_by_id_and_os_only_gpu_stays_invisible() {
    let mut host = HostRuntimeProfile::current_without_gpu_probe();
    host.gpus = vec![gpu(Some("pci:0000:02:00.0")), gpu(Some("pci:0000:03:00.0"))];
    let policy = DevicePolicy {
        capacity_limit_bytes: Some(9_000),
        reserved_bytes: Some(1_000),
        measurements: Some(DeviceMeasurements {
            bandwidth_gbps: Some(0.0),
            ..Default::default()
        }),
    };
    let measured = policy.measurements.clone();
    let report = inventory(
        &host,
        Some(&[runtime(Some("03:00.0"))]),
        &BTreeMap::from([("pci:0000:03:00.0".to_owned(), policy)]),
    );
    assert_eq!(report.physical_gpus[0].visibility, Visibility::Invisible);
    assert_eq!(report.physical_gpus[0].allocatable_bytes, None);
    let selected = &report.physical_gpus[1];
    assert_eq!(selected.visibility, Visibility::Selectable);
    assert_eq!(selected.selectors, ["CUDA0"]);
    assert_eq!(selected.total_bytes, Some(10_000));
    assert_eq!(selected.allocatable_bytes, Some(8_000));
    assert_eq!(selected.measurements, measured);
}

#[test]
fn unknown_runtime_and_unidentifiable_devices_are_explicit() {
    let mut host = HostRuntimeProfile::current_without_gpu_probe();
    let cpu_only = inventory(&host, Some(&[]), &BTreeMap::new());
    assert!(cpu_only.physical_gpus.is_empty());
    assert_eq!(cpu_only.runtime_devices, Some(vec![]));
    host.gpus = vec![gpu(None), gpu(Some("pci:0000:02:00.0"))];
    let unknown = inventory(&host, None, &BTreeMap::new());
    assert_eq!(unknown.runtime_devices, None);
    assert_eq!(unknown.physical_gpus[0].stable_id, None);
    let runtime_without_id = inventory(&host, Some(&[runtime(None)]), &BTreeMap::new());
    assert!(
        runtime_without_id
            .physical_gpus
            .iter()
            .all(|gpu| gpu.visibility == Visibility::Unknown)
    );
}

#[test]
fn duplicate_physical_identifiers_do_not_publish_selectors_or_capacity_policy() {
    let mut host = HostRuntimeProfile::current_without_gpu_probe();
    host.gpus = vec![gpu(Some("02:00.0")), gpu(Some("02:00.0"))];
    let report = inventory(&host, Some(&[runtime(Some("02:00.0"))]), &BTreeMap::new());
    assert!(
        report
            .physical_gpus
            .iter()
            .all(|gpu| gpu.visibility == Visibility::Unknown
                && gpu.stable_id.is_none()
                && gpu.selectors.is_empty())
    );
}
