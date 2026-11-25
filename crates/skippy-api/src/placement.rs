//! Stable device placement and in-process capacity accounting for model loads.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard},
};

use skippy_native_runtime::HostRuntimeProfile;
use skippy_protocol::StageDevice;
use skippy_runtime::BackendDevice;

use crate::hardware::{self, RuntimeDevice};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlacementRequest {
    Automatic,
    Pinned(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlacementDevice {
    pub stable_id: String,
    pub selector: String,
    pub selectable: bool,
    /// Maximum bytes this process may reserve on the physical device.
    pub capacity_bytes: Option<u64>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum PlacementError {
    Missing(String),
    Ambiguous(String),
    Incompatible(String),
    Exhausted {
        device: Option<String>,
        requested: u64,
        available: u64,
    },
    NoSelectableDevices,
    EmptyReservation,
}

impl std::fmt::Display for PlacementError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(id) => write!(formatter, "device {id} is missing from the inventory"),
            Self::Ambiguous(id) => {
                write!(formatter, "device {id} has an ambiguous runtime binding")
            }
            Self::Incompatible(id) => write!(
                formatter,
                "device {id} has no selectable accelerator in the runtime"
            ),
            Self::Exhausted {
                device,
                requested,
                available,
            } => write!(
                formatter,
                "device {} has {available} bytes available for a {requested}-byte reservation",
                device.as_deref().unwrap_or("inventory")
            ),
            Self::NoSelectableDevices => {
                write!(formatter, "no selectable accelerator with a stable ID")
            }
            Self::EmptyReservation => write!(formatter, "device reservation must be positive"),
        }
    }
}

impl std::error::Error for PlacementError {}

#[derive(Clone)]
pub struct DeviceAllocator {
    devices: Arc<[PlacementDevice]>,
    reserved: Arc<Mutex<BTreeMap<String, u64>>>,
}

impl DeviceAllocator {
    /// The caller owns the budget snapshot. Use the same allocator for loads
    /// sharing a process. Unknown capacity cannot admit a reservation.
    pub fn new(devices: Vec<PlacementDevice>) -> Self {
        Self {
            devices: devices.into(),
            reserved: Arc::default(),
        }
    }

    /// Use the physical stable IDs reported by the hardware inventory. A
    /// runtime-only accelerator keeps its reported runtime identifier.
    pub fn from_loaded_runtime() -> anyhow::Result<Self> {
        crate::native_runtime::ensure_native_runtime_available()?;
        Ok(Self::from_hardware(
            &hardware::physical_hardware(),
            &skippy_runtime::backend_devices()?,
        ))
    }

    fn from_hardware(host: &HostRuntimeProfile, devices: &[BackendDevice]) -> Self {
        let runtime: Vec<_> = devices.iter().map(RuntimeDevice::from).collect();
        let inventory = hardware::inventory(host, Some(&runtime), &BTreeMap::new());
        let mut placement = Vec::new();

        for gpu in &inventory.physical_gpus {
            if let Some(stable_id) = &gpu.stable_id
                && gpu.selectors.is_empty()
            {
                placement.push(PlacementDevice {
                    stable_id: stable_id.clone(),
                    selector: String::new(),
                    selectable: false,
                    capacity_bytes: None,
                });
            }
        }

        for (device, facts) in devices.iter().zip(runtime).filter(|(_, facts)| facts.gpu) {
            let mut physical = inventory
                .physical_gpus
                .iter()
                .filter(|gpu| gpu.selectors.contains(&facts.selector))
                .peekable();
            if physical.peek().is_none() {
                if let Some(id) = facts
                    .identifier
                    .as_deref()
                    .filter(|id| !id.trim().is_empty())
                {
                    placement.push(placement_device(device, id, None));
                }
                continue;
            }
            for gpu in physical {
                if let Some(stable_id) = &gpu.stable_id {
                    placement.push(placement_device(device, stable_id, gpu.total_bytes));
                }
            }
        }
        Self::new(placement)
    }

    /// Resolve a strict pin and publish its currently unreserved budget.
    pub fn pinned(&self, stable_id: &str) -> Result<(StageDevice, u64), PlacementError> {
        let device = self.named(stable_id)?;
        Ok((stage_device(device), available(device, &self.reserved())))
    }

    /// Atomically check and reserve capacity. Automatic requests select the
    /// largest fitting unambiguous device with stable-ID ordering for ties.
    pub fn reserve(
        &self,
        request: &PlacementRequest,
        bytes: u64,
    ) -> Result<DeviceReservation, PlacementError> {
        if bytes == 0 {
            return Err(PlacementError::EmptyReservation);
        }
        let mut reservations = self.reserved();
        let device = match request {
            PlacementRequest::Pinned(id) => self.named(id)?,
            PlacementRequest::Automatic => self
                .devices
                .iter()
                .filter(|device| self.named(&device.stable_id).is_ok())
                .max_by_key(|device| {
                    (
                        available(device, &reservations),
                        std::cmp::Reverse(&device.stable_id),
                    )
                })
                .ok_or(PlacementError::NoSelectableDevices)?,
        };
        let available = available(device, &reservations);
        if available < bytes {
            return Err(PlacementError::Exhausted {
                device: Some(device.stable_id.clone()),
                requested: bytes,
                available,
            });
        }
        *reservations.entry(device.stable_id.clone()).or_default() += bytes;
        Ok(DeviceReservation {
            allocator: self.clone(),
            device: stage_device(device),
            id: device.stable_id.clone(),
            bytes,
        })
    }

    fn named(&self, id: &str) -> Result<&PlacementDevice, PlacementError> {
        let mut matches = self.devices.iter().filter(|device| device.stable_id == id);
        let device = matches
            .next()
            .ok_or_else(|| PlacementError::Missing(id.to_owned()))?;
        if matches.next().is_some()
            || self
                .devices
                .iter()
                .filter(|candidate| candidate.selectable && candidate.selector == device.selector)
                .count()
                > 1
        {
            return Err(PlacementError::Ambiguous(id.to_owned()));
        }
        if !device.selectable || device.selector.is_empty() || id.is_empty() {
            return Err(PlacementError::Incompatible(id.to_owned()));
        }
        Ok(device)
    }

    fn reserved(&self) -> MutexGuard<'_, BTreeMap<String, u64>> {
        self.reserved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn placement_device(
    device: &BackendDevice,
    stable_id: &str,
    physical_total_bytes: Option<u64>,
) -> PlacementDevice {
    let capacity_bytes = (device.memory_total > 0).then(|| {
        let total = physical_total_bytes.map_or(device.memory_total, |physical_total| {
            physical_total.min(device.memory_total)
        });
        device.memory_free.min(total)
    });
    PlacementDevice {
        stable_id: stable_id.to_owned(),
        selector: device.name.clone(),
        selectable: true,
        capacity_bytes,
    }
}

fn stage_device(device: &PlacementDevice) -> StageDevice {
    StageDevice {
        backend_device: device.selector.clone(),
        stable_id: Some(device.stable_id.clone()),
        index: None,
        vram_bytes: device.capacity_bytes,
    }
}

fn available(device: &PlacementDevice, reservations: &BTreeMap<String, u64>) -> u64 {
    device.capacity_bytes.unwrap_or(0).saturating_sub(
        reservations
            .get(&device.stable_id)
            .copied()
            .unwrap_or_default(),
    )
}

#[must_use = "the device budget is reserved for the lifetime of this handle"]
pub struct DeviceReservation {
    allocator: DeviceAllocator,
    device: StageDevice,
    id: String,
    bytes: u64,
}

impl DeviceReservation {
    pub fn device(&self) -> &StageDevice {
        &self.device
    }
}

impl Drop for DeviceReservation {
    fn drop(&mut self) {
        let mut reservations = self.allocator.reserved();
        if let Some(bytes) = reservations.get_mut(&self.id) {
            *bytes -= self.bytes;
            if *bytes == 0 {
                reservations.remove(&self.id);
            }
        }
    }
}

#[cfg(test)]
mod tests;
