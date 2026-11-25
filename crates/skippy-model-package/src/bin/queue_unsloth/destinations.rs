//! Resolve explicit publishing destinations before inspecting or submitting jobs.

use anyhow::{Context, Result};
use hf_hub::HFClient;
use skippy_model_hf::repository_settings::HubRepository;

pub(super) async fn configure(args: &mut super::Args, client: &HFClient) -> Result<()> {
    args.catalog_repository = HubRepository::require("SKIPPY_MODEL_CATALOG")?.full_name();
    args.package_bucket = HubRepository::require("SKIPPY_PACKAGE_BUCKET")?.full_name();
    if args.target_namespace.is_empty() || args.job_namespace.is_empty() {
        let user = client.whoami().send().await.context(
            "resolve your Hugging Face namespace, or supply --target-namespace and --job-namespace",
        )?;
        if args.target_namespace.is_empty() {
            args.target_namespace = user.username.clone();
        }
        if args.job_namespace.is_empty() {
            args.job_namespace = user.username;
        }
    }
    Ok(())
}
