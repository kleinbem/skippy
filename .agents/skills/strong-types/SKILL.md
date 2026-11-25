---
name: strong-types
description: Use when a value in Skippy's Rust crates carries a meaning its type does not state — a 0 or None that means auto or unbounded, two counts that can be swapped, a bound checked by hand — to choose the type, keep the wire stable and parse it once at the edge it enters by.
---

# Strong types in Skippy's Rust crates

For `skippy_api` and `skippy-api-rs`, the rules and boundaries are already
written down: `skippy_api/AGENTS.md`, `skippy_api/TYPE-RULES.md` (start at §0,
which currently defers new C++ strong types) and the `.clang-tidy` files in
each boundary directory. This skill covers `crates/`, which those do not.

## Signs

- A sentinel with a documented meaning: "zero disables", "None derives eight
  times", "0 as many as the hardware allows".
- One value that means different things in different places, such as a `0`
  that is "bounded default" for one runtime and "disabled" for another.
- Two spellings of one meaning, such as `None` and `Some(0)`.
- Neighbouring parameters of one integer type, or a product of two counts
  formed outside one constructor.

## Choosing the type

| the value means | type | in this tree |
|---|---|---|
| automatic, or a positive amount | an enum with a named `Auto` variant; `Option<NonZero*>` where `None` is already that struct's "unset" | `SampleThreads`, `RuntimeConfig::kv_pool_tokens` |
| a limit, or no limit | an enum such as `{ Tokens(u64), Unbounded }` | `StageKvRetention` |
| one of a closed set | an enum with serde renames | `BatchPolicyKind`, `SplitMode` |
| a count that must not be swapped with another | a newtype with a private field and a fallible constructor | |

If `0` and `None` mean the same thing, the type makes one of them
unrepresentable.

## The edges a value crosses

| edge | parse or convert here |
|---|---|
| config and wire | serde in `crates/skippy-protocol`; CLI settings reach it through `crates/skippy-cli/src/serve_settings` |
| stage config into the runtime | `runtime_config_from_stage_config` (`skippy-serving/src/runtime_state.rs`) and `draft_runtime_config` (`frontend/generation/draft_runner.rs`) |
| the native FFI | `RuntimeConfig::as_raw` in `crates/skippy-runtime/src/config.rs`, and nowhere else |

- **Parse once, where it enters.** `#[serde(from = "…", into = "…")]` when every
  legacy value has a meaning (a legacy `0` becomes `Auto`), `try_from` when a
  value must be rejected. Code below takes the type and never re-checks it.
- **Keep the wire deliberately.** The serialized form stays byte-identical unless
  the change is meant to be a protocol change. Add one test that feeds the
  legacy value and reads the serialized value back, and check whether the field
  feeds an identity hash (`crates/skippy-cache/src/identity.rs`).
- **Raw only for the FFI.** `as_raw` turns types into native integers
  (`map_or(0, NonZeroU32::get)`); call sites never do.
- **Name the meaning at each site.** `StageConfig` and `RuntimeConfig` are built
  field by field in many places; write `SampleThreads::Auto`, not
  `Default::default()`.

## Editing and gates

Change the declaration, build, and fix each site the compiler reports; no sed,
perl or regex over `*.rs`. Then, serially from the repository root:
`cargo fmt --all --check`, `cargo check -p <crate>` per touched crate,
`just lint`, `cargo test -p <crate>` for every touched crate, and `just test`.
