use std::{fs, io, path::Path};

use serde::{Deserialize, Serialize};

use crate::{BenchmarkBackend, DeviceMeasurement, DeviceTarget};

/// Explicit identity of the benchmark recipe, ordered visible devices and
/// loaded driver/runtime. Include driver and helper content identity when the
/// runtime release alone does not bind them.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FingerprintKey {
    pub backend: BenchmarkBackend,
    pub runtime_identity: String,
    pub hardware_identity: String,
    pub devices: Vec<DeviceTarget>,
}

#[derive(Serialize, Deserialize)]
struct Fingerprint {
    version: u32,
    key: FingerprintKey,
    measurements: Vec<DeviceMeasurement>,
}

/// Missing or stale fingerprints return None. Malformed files report an error.
pub fn load_fingerprint(
    path: &Path,
    key: &FingerprintKey,
) -> io::Result<Option<Vec<DeviceMeasurement>>> {
    let data = match fs::File::open(path) {
        Ok(data) => data,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let fingerprint: Fingerprint = serde_json::from_reader(data)?;
    Ok((fingerprint.version == 1 && fingerprint.key == *key).then_some(fingerprint.measurements))
}

/// Replace one cache entry atomically so concurrent readers see complete data.
pub fn store_fingerprint(
    path: &Path,
    key: FingerprintKey,
    measurements: Vec<DeviceMeasurement>,
) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(
        &mut temporary,
        &Fingerprint {
            version: 1,
            key,
            measurements,
        },
    )?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|failure| failure.error)?;
    Ok(())
}
