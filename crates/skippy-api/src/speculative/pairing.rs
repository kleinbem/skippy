//! Pure selection of a DFlash-family draft for a target and backend.

use std::path::{Path, PathBuf};

use skippy_model_artifact::gguf::GgufCompactMeta;
use skippy_package_format::{DFLASH_MAX_DRAFT_TOKENS, DFlashDraft, DFlashTarget};
use skippy_runtime::{
    BackendDevice, BackendDeviceType,
    package::{PackageDFlashProposerInfo, PackageGenerationInfo},
};

use super::{DraftCatalog, normalize_architecture};

/// Effective backend capabilities for the target and draft placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraftBackend {
    pub metal: bool,
    pub dflash_context: bool,
    /// Whether this runtime enables DFlash by default on this placement.
    pub automatic_dflash: bool,
}

impl DraftBackend {
    /// Resolve backend policy from the native runtime's registered devices and
    /// the effective draft placement. Ambiguous placements have no default.
    pub fn for_native_placement(
        devices: &[BackendDevice],
        n_gpu_layers: i32,
        selected: Option<&str>,
    ) -> Option<Self> {
        let mut offload = devices.iter().filter(|device| {
            n_gpu_layers != 0
                && matches!(
                    device.device_type,
                    BackendDeviceType::Gpu | BackendDeviceType::IntegratedGpu
                )
                && selected.is_none_or(|name| name == device.name)
        });
        if let Some(name) = selected
            && !devices.iter().any(|device| name == device.name)
        {
            return None;
        }
        let is_metal = |device: &BackendDevice| {
            let name = device.name.to_ascii_lowercase();
            name == "metal"
                || name.strip_prefix("mtl").is_some_and(|index| {
                    !index.is_empty() && index.bytes().all(|digit| digit.is_ascii_digit())
                })
        };
        let metal = offload.next().is_some_and(is_metal);
        // A mixed Metal/non-Metal placement has no unambiguous catalog policy.
        if offload.any(|device| is_metal(device) != metal) {
            return None;
        }
        Some(Self {
            metal,
            dflash_context: !devices.is_empty(),
            // Current Windows memory planning omits per-lane draft recurrent state.
            automatic_dflash: !cfg!(windows),
        })
    }
}

/// Target identity supplied by the host. Shape comes from model metadata.
#[derive(Debug, Clone, Copy)]
pub struct DraftTarget<'a> {
    pub metadata: Option<&'a GgufCompactMeta>,
    /// Trusted model identity used when the GGUF has neither a basename nor name.
    pub name: Option<&'a str>,
    pub package_generation: Option<&'a PackageGenerationInfo>,
}

/// The host's intent after resolving its own configuration.
#[derive(Debug, Clone, Copy, Default)]
pub enum DraftOverride<'a> {
    #[default]
    Auto,
    Disabled,
    /// Select a named DFlash strategy from the package.
    PackageStrategy(&'a str),
    /// Use a caller-supplied DFlash file, subject to compatibility checks.
    Local {
        path: &'a Path,
        max_draft_tokens: Option<u32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DraftProvenance {
    Explicit,
    Package { strategy: String, proposer: String },
    PinnedIndex { sha256: String },
    Bootstrap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DraftUnavailable {
    ExplicitlyDisabled,
    TargetMetadataUnavailable,
    UnknownTarget,
    PackageStrategyUnavailable,
    PackageSelectsOtherStrategy,
    BackendMismatch,
    DraftContextUnavailable,
    AutomaticDFlashUnavailable,
    InvalidPackageProposer,
    InvalidOverride,
}

/// A selected draft still needs to be acquired and checked before use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftPlan {
    pub(super) provenance: DraftProvenance,
    pub(super) source: DraftSource,
    pub(super) target: DFlashTarget,
    pub(super) target_layers: Option<Vec<u32>>,
    pub(super) max_draft_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftSource {
    Local(PathBuf),
    Pinned(DFlashDraft),
}

impl DraftPlan {
    pub fn provenance(&self) -> &DraftProvenance {
        &self.provenance
    }

    pub fn source(&self) -> &DraftSource {
        &self.source
    }

    pub fn max_draft_tokens(&self) -> u32 {
        self.max_draft_tokens
    }

    pub fn declared_target_layers(&self) -> Option<&[u32]> {
        self.target_layers.as_deref()
    }
}

impl DraftCatalog {
    /// Resolve DFlash without disk access, network activity, or global state.
    /// A package that declares speculation owns its choice even when none of
    /// its DFlash proposers can run.
    pub fn resolve(
        &self,
        target: DraftTarget<'_>,
        backend: DraftBackend,
        requested: DraftOverride<'_>,
    ) -> Result<DraftPlan, DraftUnavailable> {
        if matches!(requested, DraftOverride::Disabled) {
            return Err(DraftUnavailable::ExplicitlyDisabled);
        }
        if !backend.dflash_context {
            return Err(DraftUnavailable::DraftContextUnavailable);
        }
        if matches!(requested, DraftOverride::Auto) && !backend.automatic_dflash {
            return Err(DraftUnavailable::AutomaticDFlashUnavailable);
        }
        let meta = target
            .metadata
            .ok_or(DraftUnavailable::TargetMetadataUnavailable)?;
        let shape = target_shape(meta).ok_or(DraftUnavailable::TargetMetadataUnavailable)?;
        let cap = if backend.metal { 6 } else { 15 };
        match requested {
            DraftOverride::Local {
                path,
                max_draft_tokens,
            } => {
                let max_draft_tokens = max_draft_tokens.unwrap_or(cap);
                if path.as_os_str().is_empty()
                    || !(1..=DFLASH_MAX_DRAFT_TOKENS).contains(&max_draft_tokens)
                {
                    return Err(DraftUnavailable::InvalidOverride);
                }
                Ok(DraftPlan {
                    provenance: DraftProvenance::Explicit,
                    source: DraftSource::Local(path.into()),
                    target: shape,
                    target_layers: None,
                    max_draft_tokens,
                })
            }
            DraftOverride::PackageStrategy(strategy) => {
                package_plan(target, shape, backend, cap, Some(strategy))
            }
            DraftOverride::Auto
                if target
                    .package_generation
                    .is_some_and(|generation| generation.speculative_decoding.is_some()) =>
            {
                package_plan(target, shape, backend, cap, None)
            }
            DraftOverride::Auto => {
                self.catalog_plan(shape, target_name(meta, target.name), backend, cap)
            }
            DraftOverride::Disabled => Err(DraftUnavailable::ExplicitlyDisabled),
        }
    }

    fn catalog_plan(
        &self,
        shape: DFlashTarget,
        name: &str,
        backend: DraftBackend,
        cap: u32,
    ) -> Result<DraftPlan, DraftUnavailable> {
        let same_shape = self
            .pairings()
            .iter()
            .filter(|pairing| pairing.target.same_shape(&shape));
        let named_family = same_shape.clone().any(|pairing| {
            pairing
                .target
                .family
                .as_deref()
                .is_some_and(|family| names_family(name, family))
        });
        let mut matches = same_shape
            .filter(|pairing| match pairing.target.family.as_deref() {
                Some(family) => names_family(name, family),
                None => !named_family,
            })
            .peekable();
        if matches.peek().is_none() {
            return Err(DraftUnavailable::UnknownTarget);
        }
        let pairing = matches
            .find(|pairing| pairing.backends.applies_on(backend.metal))
            .ok_or(DraftUnavailable::BackendMismatch)?;
        Ok(DraftPlan {
            provenance: self.provenance(),
            source: DraftSource::Pinned(pairing.draft.clone()),
            target: pairing.target.clone(),
            target_layers: None,
            max_draft_tokens: cap.min(pairing.max_draft_tokens),
        })
    }
}

fn package_plan(
    target: DraftTarget<'_>,
    shape: DFlashTarget,
    backend: DraftBackend,
    cap: u32,
    requested: Option<&str>,
) -> Result<DraftPlan, DraftUnavailable> {
    let speculative = target
        .package_generation
        .and_then(|generation| generation.speculative_decoding.as_ref())
        .ok_or(DraftUnavailable::PackageStrategyUnavailable)?;
    let strategy_name = requested.unwrap_or(&speculative.default);
    let strategy = speculative
        .strategies
        .get(strategy_name)
        .ok_or(DraftUnavailable::PackageStrategyUnavailable)?;
    if strategy.strategy_type != skippy_serving::DFLASH_STRATEGY {
        return Err(DraftUnavailable::PackageSelectsOtherStrategy);
    }
    for name in &strategy.proposers {
        let proposer = speculative
            .proposers
            .get(name)
            .and_then(|p| p.dflash.as_ref())
            .ok_or(DraftUnavailable::InvalidPackageProposer)?;
        if proposer.backends.applies_on(backend.metal) {
            validate_proposer(proposer, &shape)?;
            return Ok(DraftPlan {
                provenance: DraftProvenance::Package {
                    strategy: strategy_name.into(),
                    proposer: name.clone(),
                },
                source: DraftSource::Pinned(proposer.draft.clone()),
                target: shape,
                target_layers: Some(proposer.target_layers.clone()),
                max_draft_tokens: cap.min(proposer.max_draft_tokens),
            });
        }
    }
    Err(DraftUnavailable::BackendMismatch)
}

fn validate_proposer(
    proposer: &PackageDFlashProposerInfo,
    target: &DFlashTarget,
) -> Result<(), DraftUnavailable> {
    if !proposer.draft.is_pinned()
        || proposer.target_layers.is_empty()
        || proposer
            .target_layers
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        || !(2..=DFLASH_MAX_DRAFT_TOKENS + 1).contains(&proposer.block_size)
        || proposer
            .target_layers
            .iter()
            .any(|&layer| layer > target.layer_count)
        || !(1..=DFLASH_MAX_DRAFT_TOKENS).contains(&proposer.max_draft_tokens)
        || proposer.max_draft_tokens >= proposer.block_size
    {
        return Err(DraftUnavailable::InvalidPackageProposer);
    }
    Ok(())
}

pub(super) fn target_shape(meta: &GgufCompactMeta) -> Option<DFlashTarget> {
    let architecture = normalize_architecture(&meta.architecture);
    if architecture.is_empty() || meta.embedding_size == 0 {
        return None;
    }
    Some(DFlashTarget {
        architecture,
        layer_count: meta.executable_layer_count()?,
        embedding_length: meta.embedding_size,
        expert_count: meta.expert_count,
        family: None,
    })
}

fn target_name<'a>(meta: &'a GgufCompactMeta, supplied: Option<&'a str>) -> &'a str {
    [meta.basename.as_deref(), meta.name.as_deref(), supplied]
        .into_iter()
        .flatten()
        .find(|name| !name.trim().is_empty())
        .unwrap_or_default()
}

fn names_family(name: &str, family: &str) -> bool {
    let normalized = |text: &str| text.to_ascii_lowercase().replace([' ', '_'], "-");
    let name = normalized(name);
    let family = normalized(family);
    let continues = |c: char| c.is_ascii_alphanumeric() || c == '.';
    name.match_indices(&family).any(|(start, _)| {
        !name[..start].ends_with(continues) && !name[start + family.len()..].starts_with(continues)
    })
}
