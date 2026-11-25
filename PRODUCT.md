# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Stack

Nextra 4 with the documentation theme and Pagefind, built as a static Next.js
site and deployed as Cloudflare Workers Static Assets through the `cf` CLI.

## Users

Engineers evaluating or operating the Skippy inference engine. They need to
move quickly from installation to serving a model, then understand and tune
pipeline parallelism, KV caching, and serving configuration.

## Product Purpose

Skippy is a standalone model server for local and distributed staged
inference. The documentation site teaches engineers how to install it, run
models, operate explicit pipeline stages, configure caching and serving, and
contribute safely.

## Positioning

Skippy owns the complete model-serving path: model and runtime management,
OpenAI- and Anthropic-compatible serving, explicit staged execution, package
materialization, scheduling, caching, and native runtime qualification.

## Operating Context

Readers work from terminals, TOML settings, model repositories, local or
multi-machine GPU hosts, HTTP clients, and the Skippy source repository. All
repository builds and checks run through Just.

## Capabilities and Constraints

- Initial public sections: Getting Started, Running Models, Pipeline
  Parallelism, KV Caching, Configuration Reference, and Contributing.
- Public documentation must describe standalone Skippy and must not reintroduce
  MeshLLM commands, configuration, product boundaries, or release policy.
- The site is a static export with client-side search and no runtime database.
- Cloudflare operations use the `cf` CLI because the repository has no Wrangler
  configuration.
- Internal design notes, experiments, qualification ledgers, and historical
  migration plans remain outside the public site.

## Brand Commitments

Use the supplied black Skippy kangaroo-and-wordmark logo. The site should feel
like a precise engineering reference and preserve the familiar Nextra docs
interaction model rather than becoming a marketing surface.

## Evidence on Hand

- Current standalone source and operator documentation in `README.md`,
  `crates/skippy-cli/README.md`, `docs/`, `CONTRIBUTING.md`, and `ci/README.md`.
- Earlier Skippy-focused editorial material in the local
  `Mesh-LLM/mesh-llm` documentation site, used only as adaptation input.
- Supplied Skippy logo asset.
- No testimonials, customer claims, or external benchmarks are approved for
  publication.

## Product Principles

- Lead with a successful first model run.
- Explain distributed execution from the operator's point of view.
- Keep commands, defaults, and configuration tied to current standalone code.
- Separate public guidance from implementation plans and certification records.
- Make every page searchable, linkable, and useful from a terminal workflow.

## Accessibility & Inclusion

Meet WCAG 2.2 AA for reading, navigation, focus states, contrast, reduced
motion, and responsive layouts.
