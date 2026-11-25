use std::time::Duration;

use anyhow::{Result, bail};
use skippy_api::native_runtime::{NativeRuntimeOptions, local_native_runtime_install};
use skippy_gpu_bench::{
    BenchmarkBackend, Cancellation, MeasurementRequest, NativeMeasurementRunner,
};
use skippy_runtime_install::NativeRuntimeFlavor;

pub async fn run(options: &NativeRuntimeOptions, timeout_secs: u64) -> Result<()> {
    if !(1..=86_400).contains(&timeout_secs) {
        bail!("GPU benchmark timeout must be between 1 and 86400 seconds");
    }
    let runtime = local_native_runtime_install(options)?;
    let backend = match &runtime.manifest.runtime.backend.kind {
        NativeRuntimeFlavor::Metal => BenchmarkBackend::Metal,
        NativeRuntimeFlavor::Cuda => BenchmarkBackend::Cuda,
        NativeRuntimeFlavor::Rocm => BenchmarkBackend::Hip,
        backend => bail!("runtime backend {backend:?} does not provide a GPU benchmark"),
    };
    let environment = [
        "CUDA_VISIBLE_DEVICES",
        "CUDA_DEVICE_ORDER",
        "HIP_VISIBLE_DEVICES",
        "ROCR_VISIBLE_DEVICES",
        "GPU_DEVICE_ORDINAL",
        "LD_LIBRARY_PATH",
        "DYLD_LIBRARY_PATH",
        "PATH",
        "SystemRoot",
        "WINDIR",
    ]
    .into_iter()
    .filter_map(|name| Some((name.into(), std::env::var_os(name)?)))
    .collect();
    let request = MeasurementRequest {
        executable: runtime.gpu_benchmark_tool()?,
        backend,
        environment,
        targets: Vec::new(),
        timeout: Duration::from_secs(timeout_secs),
    };
    let cancellation = Cancellation::default();
    let worker_cancellation = cancellation.clone();
    let mut worker = tokio::task::spawn_blocking(move || {
        skippy_gpu_bench::measure(&NativeMeasurementRunner, &request, &worker_cancellation)
    });
    let readings = tokio::select! {
        result = &mut worker => result??,
        signal = tokio::signal::ctrl_c() => {
            cancellation.cancel();
            let result = worker.await?;
            signal?;
            result?
        }
    };
    let report = serde_json::json!({
        "runtime": runtime.native_runtime_id,
        "backend": backend,
        "devices": readings,
    });
    skippy_commands::console::present(&report, |output| {
        for reading in &readings {
            let result = reading
                .result
                .as_ref()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            writeln!(
                output,
                "{} [{}]: {:.2} GB/s p50, {:.2} GB/s p90",
                result.device, reading.ordinal, result.p50_gbps, result.p90_gbps
            )?;
        }
        Ok(())
    })
}
