use super::*;

fn device(stable_id: &str, selector: &str, capacity: u64) -> PlacementDevice {
    PlacementDevice {
        stable_id: stable_id.into(),
        selector: selector.into(),
        selectable: true,
        capacity_bytes: Some(capacity),
    }
}

#[test]
fn tracks_shared_contention_and_independent_gpu_budgets() {
    let allocator = DeviceAllocator::new(vec![
        device("gpu1", "CUDA0", 100),
        device("gpu2", "CUDA1", 200),
    ]);
    let first = PlacementRequest::Pinned("gpu1".into());
    let loaded = allocator.reserve(&first, 80).unwrap();
    assert_eq!(loaded.device().backend_device, "CUDA0");
    assert!(matches!(
        allocator.reserve(&first, 21),
        Err(PlacementError::Exhausted { available: 20, .. })
    ));
    let other = allocator
        .reserve(&PlacementRequest::Automatic, 180)
        .unwrap();
    assert_eq!(other.device().backend_device, "CUDA1");
    drop(loaded);
    assert!(allocator.reserve(&first, 100).is_ok());
}

#[test]
fn invalid_explicit_pins_do_not_fall_back() {
    let mut invisible = device("invisible", "", 500);
    invisible.selectable = false;
    let allocator = DeviceAllocator::new(vec![
        device("duplicate", "CUDA0", 100),
        device("duplicate", "CUDA1", 100),
        invisible,
        device("usable", "CUDA2", 100),
    ]);
    for (id, expected) in [
        ("duplicate", PlacementError::Ambiguous("duplicate".into())),
        (
            "invisible",
            PlacementError::Incompatible("invisible".into()),
        ),
        ("missing", PlacementError::Missing("missing".into())),
    ] {
        assert!(
            matches!(allocator.reserve(&PlacementRequest::Pinned(id.into()), 1), Err(error) if error == expected)
        );
    }
}

#[test]
fn reservation_rolls_back_when_a_load_fails() {
    let allocator = DeviceAllocator::new(vec![device("gpu", "CUDA0", 100)]);
    let pin = PlacementRequest::Pinned("gpu".into());
    let failed = || -> anyhow::Result<()> {
        let _reservation = allocator.reserve(&pin, 100)?;
        anyhow::bail!("simulated load failure")
    };
    assert!(failed().is_err());
    assert!(allocator.reserve(&pin, 100).is_ok());
}

#[test]
fn pins_the_physical_id_reported_by_doctor() {
    use skippy_native_runtime::HostGpuProfile;
    use skippy_native_runtime::host::HostGpuProbe;
    use skippy_runtime::BackendDeviceType;

    let mut host = HostRuntimeProfile::current_without_gpu_probe();
    host.gpus = vec![
        HostGpuProfile {
            display_name: "NVIDIA GPU".into(),
            stable_id: Some("GPU-ABC".into()),
            vram_bytes: Some(90),
            probe: Some(HostGpuProbe {
                source: "fixture".into(),
                path: None,
                fields: BTreeMap::from([("Bus Location".into(), "0000:65:00.0".into())]),
                raw_lines: Vec::new(),
            }),
            ..Default::default()
        },
        HostGpuProfile {
            display_name: "Invisible GPU".into(),
            stable_id: Some("GPU-DEF".into()),
            ..Default::default()
        },
    ];
    let devices = [
        BackendDevice {
            name: "CUDA0".into(),
            description: None,
            device_id: Some("65:00.0".into()),
            memory_free: 95,
            memory_total: 100,
            device_type: BackendDeviceType::Gpu,
            caps: 0,
        },
        BackendDevice {
            name: "CUDA1".into(),
            description: None,
            device_id: Some("66:00.0".into()),
            memory_free: 70,
            memory_total: 100,
            device_type: BackendDeviceType::Gpu,
            caps: 0,
        },
    ];
    let runtime: Vec<_> = devices.iter().map(RuntimeDevice::from).collect();
    let report = hardware::inventory(&host, Some(&runtime), &BTreeMap::new());
    let stable_id = report.physical_gpus[0].stable_id.as_deref().unwrap();
    assert_eq!(stable_id, "uuid:gpu-abc");

    let allocator = DeviceAllocator::from_hardware(&host, &devices);
    let (selected, available) = allocator.pinned(stable_id).unwrap();
    assert_eq!(selected.backend_device, "CUDA0");
    assert_eq!(selected.stable_id.as_deref(), Some(stable_id));
    assert_eq!(available, 90);
    let reservation = allocator
        .reserve(&PlacementRequest::Pinned(stable_id.into()), 50)
        .unwrap();
    assert_eq!(allocator.pinned(stable_id).unwrap().1, 40);
    drop(reservation);
    assert_eq!(
        allocator.pinned("66:00.0").unwrap().0.backend_device,
        "CUDA1"
    );
    assert!(matches!(
        allocator.pinned("uuid:gpu-def"),
        Err(PlacementError::Incompatible(_))
    ));

    let duplicate = BackendDevice {
        name: "CUDA2".into(),
        ..devices[0].clone()
    };
    let allocator = DeviceAllocator::from_hardware(&host, &[devices[0].clone(), duplicate]);
    assert!(matches!(
        allocator.pinned(stable_id),
        Err(PlacementError::Ambiguous(_))
    ));
}
