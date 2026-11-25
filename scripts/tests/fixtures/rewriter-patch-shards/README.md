# Patch shard candidate fixtures

These are data-only inputs and historical output copies. They are not fresh
legacy executions and do not certify native rewriting or the generated queue.

## Independent output sources

The following files are exact copies of checked-in legacy mail patches at
`a4e04070db2c6b8e2644a40f5c4f207789df6c2d`. Their expected digests were copied
from the checked-in `third_party/llama.cpp/patches/generated/series.json`, not
computed by the candidate sharder.

| Fixture | Source under `third_party/llama.cpp/patches/generated/` | SHA-256 |
| --- | --- | --- |
| `afmoe.patch` | `0001-family-afmoe.patch` | `f24da42643df886f3d854c46722ddafe0713d046a712547b976c77ece30f9b95` |
| `deepseek2--mistral4.patch` | `0009-family-deepseek2--mistral4.patch` | `cfe4e22719a7b3597ac5067d86eb7d21528af6d7be1b3ec9ca0f8c52c69ccd26` |
| `rwkv6.patch` | `0070-family-rwkv6.patch` | `513f9d910dc2e57461f42a2eebe8a903fca411a3ad2b30ca5a11cb897e3640e0` |

The regression in `scripts/tests/automation_contracts.py` removes
only the mail envelope from these copies. The reduced case reuses their entire
diff sections. It does not ask the implementation under test to generate the
expected mail bytes. Reduced filenames differ from the full queue numbering;
mail subjects and full-mail digests do not include the filename.

`family-map.json` projects the four relevant mappings from
`ci/llama-canary/generated-family-map.json`, SHA-256
`63db182a4a539ef45e9910565d79d0f3b92ab57b590ceef6175b2769c1e7ad83`.
Its insertion order and RWKV6 source order are deliberately reversed.
`certified.json` projects the four causal family/class/profile triples from
`ci/llama-canary/model-fixtures.json`, SHA-256
`90834e6f52cf494cff216b3bce40953919109596512eb68efd07b4e33f938cdf`.
It adds a synthetic embedding row to exercise the workload-only exemption.
Neither reduced JSON file is represented as historical generator output.

Run `just automation-contracts` to check the reduced shard order and exact mail bytes.
