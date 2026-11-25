# Reviewed catalog generation defaults

These JSON files are reviewed publisher inputs for standalone layer packages.
Each file is a standalone `GenerationRequestDefaults` object accepted by
`skippy models package --generation-defaults`.

The values come from an official model card or `generation_config.json` at the
immutable revision embedded in every profile. Fields without an official
recommendation remain absent. The package schema test parses and validates all
19 files, including their revision-pinned provenance.

These files cover 20 of the 22 active package mappings because the two Nemotron
Super packages share one base-model profile. Kimi K3 and Inkling have no
general deployment recommendation in their official repositories at the
audited revisions; they intentionally use Skippy's bounded fallback.

Publish these defaults only after a compatible Skippy runtime is released.
