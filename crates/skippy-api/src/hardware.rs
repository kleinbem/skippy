//! Physical hardware and selected-runtime visibility for placement decisions.

use std::collections::BTreeMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use skippy_native_runtime::{HostGpuProfile, HostRuntimeProfile};
use skippy_runtime::{BackendDevice, BackendDeviceType};

/// Values supplied by the caller, keyed by a physical stable device ID.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DevicePolicy {
    pub reserved_bytes: Option<u64>,
    pub capacity_limit_bytes: Option<u64>,
    pub measurements: Option<DeviceMeasurements>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceMeasurements {
    pub bandwidth_gbps: Option<f64>,
    pub compute_tflops_fp32: Option<f64>,
    pub compute_tflops_fp16: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Selectable,
    Invisible,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PhysicalGpu {
    pub name: String,
    pub stable_id: Option<String>,
    pub unified_memory: Option<bool>,
    pub total_bytes: Option<u64>,
    pub reserved_bytes: Option<u64>,
    pub allocatable_bytes: Option<u64>,
    pub visibility: Visibility,
    pub selectors: Vec<String>,
    pub measurements: Option<DeviceMeasurements>,
}

/// A device offered by the loaded runtime. Runtime device identifiers are raw
/// driver facts. Use `PhysicalGpu::stable_id` for persisted physical placement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeDevice {
    pub selector: String,
    pub identifier: Option<String>,
    pub gpu: bool,
    pub integrated: bool,
    pub total_bytes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HardwareInventory {
    pub os: String,
    pub arch: String,
    pub physical_gpus: Vec<PhysicalGpu>,
    /// None means the selected runtime was unavailable or was not probed.
    pub runtime_devices: Option<Vec<RuntimeDevice>>,
}

/// Probe the already loaded native runtime. The caller owns runtime selection
/// and loading. A missing runtime or failed enumeration is returned as an error.
pub fn runtime_devices() -> Result<Vec<RuntimeDevice>> {
    crate::native_runtime::ensure_native_runtime_available()?;
    Ok(skippy_runtime::backend_devices()?
        .iter()
        .map(RuntimeDevice::from)
        .collect())
}

impl From<&BackendDevice> for RuntimeDevice {
    fn from(device: &BackendDevice) -> Self {
        Self {
            selector: device.name.clone(),
            identifier: device.device_id.clone(),
            gpu: matches!(
                device.device_type,
                BackendDeviceType::Gpu | BackendDeviceType::IntegratedGpu
            ),
            integrated: device.device_type == BackendDeviceType::IntegratedGpu,
            total_bytes: (device.memory_total > 0).then_some(device.memory_total),
        }
    }
}

/// Discover OS hardware using the same host probe as native runtime selection.
pub fn physical_hardware() -> HostRuntimeProfile {
    skippy_runtime_install::host_runtime_profile()
}

/// Build a snapshot from explicit OS and runtime facts. A runtime selector is
/// associated with a physical device only through a unique stable identity.
/// An omitted reservation intentionally leaves allocatable capacity unknown.
pub fn inventory(
    host: &HostRuntimeProfile,
    runtime: Option<&[RuntimeDevice]>,
    policies: &BTreeMap<String, DevicePolicy>,
) -> HardwareInventory {
    let hardware_ids: Vec<_> = host.gpus.iter().map(physical_ids).collect();
    let runtime_ids: Vec<_> = runtime
        .unwrap_or_default()
        .iter()
        .filter(|device| device.gpu)
        .map(|device| (device, device.identifier.as_deref().and_then(normalize_id)))
        .collect();
    let physical_gpus = host
        .gpus
        .iter()
        .zip(&hardware_ids)
        .map(|(gpu, ids)| {
            let matches: Vec<_> = runtime_ids
                .iter()
                .filter_map(|(device, id)| {
                    let id = id.as_ref()?;
                    (ids.contains(id)
                        && hardware_ids.iter().filter(|ids| ids.contains(id)).count() == 1)
                        .then_some(*device)
                })
                .collect();
            let visibility = if !matches.is_empty() {
                Visibility::Selectable
            } else if runtime.is_some()
                && !ids.is_empty()
                && runtime_ids
                    .iter()
                    .all(|(_, id)| id.as_ref().is_some_and(|id| !ids.contains(id)))
            {
                Visibility::Invisible
            } else {
                Visibility::Unknown
            };
            let identity = ids
                .first()
                .filter(|id| hardware_ids.iter().filter(|ids| ids.contains(id)).count() == 1);
            let policy = identity.and_then(|id| policies.get(id));
            physical_gpu(gpu, identity, policy, visibility, &matches)
        })
        .collect();
    HardwareInventory {
        os: host.os.clone(),
        arch: host.arch.clone(),
        physical_gpus,
        runtime_devices: runtime.map(<[RuntimeDevice]>::to_vec),
    }
}

fn physical_gpu(
    gpu: &HostGpuProfile,
    identity: Option<&String>,
    policy: Option<&DevicePolicy>,
    visibility: Visibility,
    matches: &[&RuntimeDevice],
) -> PhysicalGpu {
    let total_bytes = gpu.vram_bytes.or_else(|| {
        let total = matches.first()?.total_bytes?;
        matches
            .iter()
            .all(|device| device.total_bytes == Some(total))
            .then_some(total)
    });
    let reserved_bytes = policy.and_then(|policy| policy.reserved_bytes);
    let allocatable_bytes = total_bytes.zip(reserved_bytes).map(|(total, reserved)| {
        let budget = policy
            .and_then(|policy| policy.capacity_limit_bytes)
            .map_or(total, |limit| limit.min(total));
        budget.saturating_sub(reserved)
    });
    PhysicalGpu {
        name: gpu.display_name.clone(),
        stable_id: identity.cloned(),
        unified_memory: if gpu.unified_memory || matches.iter().any(|device| device.integrated) {
            Some(true)
        } else if gpu.cuda_sm.is_some() || gpu.rocm_gfx.is_some() {
            Some(false)
        } else {
            None
        },
        total_bytes,
        reserved_bytes,
        allocatable_bytes,
        visibility,
        selectors: matches
            .iter()
            .map(|device| device.selector.clone())
            .collect(),
        measurements: policy.and_then(|policy| policy.measurements.clone()),
    }
}

fn physical_ids(gpu: &HostGpuProfile) -> Vec<String> {
    let mut ids: Vec<_> = gpu
        .stable_id
        .as_deref()
        .and_then(normalize_id)
        .into_iter()
        .collect();
    if let Some(pci) = gpu
        .probe
        .as_ref()
        .and_then(|probe| probe.fields.get("Bus Location"))
        .and_then(|pci| normalize_id(pci))
    {
        ids.push(pci);
    }
    ids
}

fn normalize_id(id: &str) -> Option<String> {
    let lower = id.trim().to_ascii_lowercase();
    let raw = lower.strip_prefix("pci:").unwrap_or(&lower);
    if let Some((bus, function)) = raw.rsplit_once(':')
        && function.contains('.')
        && !bus.is_empty()
        && raw
            .chars()
            .all(|character| character.is_ascii_hexdigit() || ":.".contains(character))
    {
        if bus.contains(':') {
            return Some(format!("pci:{raw}"));
        }
        return Some(format!("pci:0000:{raw}"));
    }
    let raw = lower.strip_prefix("uuid:").unwrap_or(&lower);
    if raw.starts_with("gpu-") {
        return Some(format!("uuid:{raw}"));
    }
    (!lower.is_empty()).then_some(lower)
}

#[cfg(test)]
mod tests;
