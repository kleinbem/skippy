//! Explicit Hugging Face repository destinations for catalogs and job storage.

use anyhow::{Context, Result, ensure};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HubRepository {
    pub namespace: String,
    pub name: String,
}

impl HubRepository {
    pub fn parse(value: &str) -> Result<Self> {
        let (namespace, name) = value
            .split_once('/')
            .context("repository must be namespace/name")?;
        for component in [namespace, name] {
            ensure!(
                !component.is_empty()
                    && component != "."
                    && component != ".."
                    && component
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric()
                            || matches!(byte, b'-' | b'_' | b'.')),
                "repository must contain two safe namespace/name components"
            );
        }
        Ok(Self {
            namespace: namespace.to_owned(),
            name: name.to_owned(),
        })
    }

    pub fn from_env(variable: &str) -> Result<Option<Self>> {
        match std::env::var(variable) {
            Ok(value) if value.trim().is_empty() => Ok(None),
            Ok(value) => Self::parse(&value)
                .with_context(|| variable.to_owned())
                .map(Some),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(_) => anyhow::bail!("{variable} must contain UTF-8"),
        }
    }

    pub fn require(variable: &str) -> Result<Self> {
        Self::from_env(variable)?
            .with_context(|| format!("set {variable}=namespace/name explicitly"))
    }

    pub fn full_name(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_locations_cannot_escape_their_namespace() {
        for value in [
            "",
            "/catalog",
            "org/",
            "../catalog",
            "org/..",
            "org/repo/path",
            "https://host/repo",
            "org/repo@main",
        ] {
            assert!(HubRepository::parse(value).is_err(), "{value}");
        }
        let repository = HubRepository::parse("owner/private-catalog").unwrap();
        assert_eq!(repository.namespace, "owner");
        assert_eq!(repository.full_name(), "owner/private-catalog");
    }
}
