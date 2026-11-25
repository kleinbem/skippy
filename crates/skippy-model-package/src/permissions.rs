use anyhow::{Context, Result};
use hf_hub::HFClient;

/// Catalog publication policy for the authenticated Hugging Face user.
#[derive(Debug, Clone)]
pub struct PermissionCheck {
    /// HuggingFace username.
    pub username: String,
    /// Whether the user owns or belongs to the configured catalog namespace.
    pub can_update_catalog: bool,
    /// Namespace for job submission and target repos.
    /// Defaults to the authenticated user's own namespace.
    pub namespace: String,
    /// Whether catalog updates should be submitted as PRs.
    /// `true` for non-members, `false` for org members.
    pub catalog_create_pr: bool,
}

/// Call `whoami`, inspect org memberships, and decide direct vs PR mode.
pub async fn check_permissions(client: &HFClient) -> Result<PermissionCheck> {
    let catalog =
        skippy_model_hf::repository_settings::HubRepository::require("SKIPPY_MODEL_CATALOG")?;
    let user = client
        .whoami()
        .send()
        .await
        .context("HF whoami failed — is your token valid?")?;

    let can_update_catalog = user.username == catalog.namespace
        || user
            .orgs
            .as_ref()
            .map(|orgs| {
                orgs.iter()
                    .any(|org| org.name.as_deref() == Some(catalog.namespace.as_str()))
            })
            .unwrap_or(false);

    let namespace = user.username.clone();

    Ok(PermissionCheck {
        username: user.username,
        can_update_catalog,
        catalog_create_pr: !can_update_catalog,
        namespace,
    })
}
