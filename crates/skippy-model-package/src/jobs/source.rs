//! Credentials for jobs that compile the Skippy source repository.

use anyhow::{Context, Result, ensure};

/// Read the dedicated source credential only when submitting a source-build job.
/// Callers must put it in the job's redacted secrets, never its environment map.
pub fn github_token_from_env() -> Result<String> {
    let token = std::env::var("SKIPPY_GITHUB_TOKEN").context(
        "source-build jobs require SKIPPY_GITHUB_TOKEN with read access to Mesh-LLM/skippy",
    )?;
    ensure!(
        !token.is_empty() && !token.chars().any(char::is_whitespace),
        "SKIPPY_GITHUB_TOKEN must be nonempty and contain no whitespace"
    );
    Ok(token)
}
