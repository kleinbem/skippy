use crate::cli::NativeRuntimeArgs;
#[cfg(feature = "dynamic-native-runtime")]
use anyhow::Context;
use anyhow::Result;
use skippy_api::native_runtime::NativeRuntimeOptions;
use skippy_commands::runtime::RuntimeRunOptions;
#[cfg(feature = "dynamic-native-runtime")]
use std::sync::Arc;

pub fn resolve_options(args: NativeRuntimeArgs) -> Result<NativeRuntimeOptions> {
    let mut options: NativeRuntimeOptions = args.into();
    if options.cache_dir.is_none() {
        options.cache_dir = Some(
            skippy_runtime_install::native_runtime_cache(None)?
                .root()
                .to_path_buf(),
        );
    }
    let release = options
        .release
        .as_deref()
        .unwrap_or(skippy_runtime_install::runtime_release_version());
    options.bundle_dirs =
        skippy_runtime_install::discover_native_runtime_bundle_dirs(&options.bundle_dirs, release)?;
    Ok(options)
}

/// Explicit conversion from the resolved native runtime options; a free
/// function because both endpoint types are foreign to this crate (orphan
/// rule).
pub fn command_options(options: &NativeRuntimeOptions) -> RuntimeRunOptions {
    RuntimeRunOptions {
        cache_dir: options.cache_dir.clone(),
        release: options.release.clone(),
        bundle_dirs: options.bundle_dirs.clone(),
        selection: options.selection.clone(),
    }
}

pub fn doctor(options: &NativeRuntimeOptions, device_inventory: bool) -> Result<()> {
    let hardware = skippy_api::hardware::physical_hardware();
    let runtime = skippy_api::native_runtime::local_native_runtime_plan(options);
    let runtime_summary = runtime.as_ref().ok().map(|plan| {
        serde_json::json!({
            "id": plan.native_runtime_id,
            "path": plan.root,
        })
    });
    let issue = runtime.err().map(|error| format!("{error:#}"));
    let model_cache = skippy_commands::models::model_cache_dir();
    let mut report = serde_json::json!({
        "hardware": hardware,
        "native_runtime": runtime_summary,
        "runtime_issue": issue,
        "runtime_cache": options.cache_dir,
        "model_cache": model_cache,
    });
    if device_inventory {
        #[cfg(feature = "dynamic-native-runtime")]
        let devices = skippy_api::native_runtime::load_local_native_runtime(options)
            .and_then(|()| skippy_api::hardware::runtime_devices());
        #[cfg(not(feature = "dynamic-native-runtime"))]
        let devices = skippy_api::hardware::runtime_devices();
        report["device_inventory"] = serde_json::to_value(skippy_api::hardware::inventory(
            &hardware,
            devices.as_deref().ok(),
            &Default::default(),
        ))?;
        if let Err(error) = devices {
            report["device_inventory_issue"] = serde_json::Value::String(format!("{error:#}"));
        }
    }
    skippy_commands::console::present(&report, |output| {
        writeln!(output, "🩺 Skippy doctor")?;
        writeln!(output, "   Machine: {} {}", hardware.os, hardware.arch)?;
        for gpu in &hardware.gpus {
            writeln!(output, "   GPU: {}", gpu.display_name)?;
        }
        if let Some(runtime) = runtime_summary.as_ref() {
            writeln!(
                output,
                "   Runtime: {}",
                runtime["id"].as_str().unwrap_or("unknown")
            )?;
            writeln!(
                output,
                "   Path: {}",
                runtime["path"].as_str().unwrap_or("unknown")
            )?;
        } else {
            writeln!(output, "   Runtime: none compatible")?;
            writeln!(output, "   Try: skippy runtime install")?;
        }
        writeln!(output, "   Model cache: {}", model_cache.display())?;
        if device_inventory {
            writeln!(
                output,
                "   Device inventory: {}",
                report["device_inventory"]
            )?;
            if let Some(issue) = report["device_inventory_issue"].as_str() {
                writeln!(output, "   Device inventory issue: {issue}")?;
            }
        }
        Ok(())
    })
}

#[cfg(feature = "dynamic-native-runtime")]
pub async fn prepare_native_runtime(options: &NativeRuntimeOptions, automatic: bool) -> Result<()> {
    if skippy_api::native_runtime::local_native_runtime_plan(options).is_err() && automatic {
        use skippy_runtime_install::{
            NativeRuntimeInstallOptions, RuntimeSelection, install_native_runtime,
        };
        let release = skippy_runtime_install::runtime_release_version();
        let build = option_env!("SKIPPY_BUILD_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"));
        let catalog =
            skippy_runtime_install::publication_catalog(build, env!("CARGO_PKG_VERSION"), release);
        let mut install = NativeRuntimeInstallOptions::new(release, catalog);
        install.cache_dir = options.cache_dir.clone();
        install.bundle_dirs = options.bundle_dirs.clone();
        install.selection = RuntimeSelection::Recommended;
        install.skippy_abi_version = Some(skippy_runtime_install::current_skippy_abi_version());
        install.progress = Some(Arc::new(|progress| {
            if let Some(total) = progress.total_bytes {
                let _ = skippy_commands::console::progress(
                    "Native runtime",
                    progress.downloaded_bytes,
                    total,
                );
            }
        }));
        skippy_commands::console::status("🔎 Selecting a compatible native runtime")?;
        install_native_runtime(install).await.with_context(|| {
            format!(
                "could not install a released runtime for this Skippy build (ABI {}); private releases require `gh release download --repo Mesh-LLM/skippy` with an authorized GitHub login, then extract a runtime and pass --runtime-bundle; run `just build` for a matching local runtime",
                skippy_runtime_install::current_skippy_abi_version()
            )
        })?;
    }
    skippy_api::native_runtime::load_local_native_runtime(options)
}
