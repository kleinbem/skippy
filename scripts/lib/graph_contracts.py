"""Validate existing graph contracts and report upstream coverage gaps."""

import json
import re

from .ci_artifacts import write_json
from .ci_process import command
from .upgrade_source import PIN, git

MANIFEST = "ci/llama-canary/model-fixtures.json"
POLICY = "ci/llama-canary/graph-coverage.json"
TABLE = "src/skippy/tests/graph_contract_cases.cmake"
MODES = {"causal", "causal-tokens", "stateless", "shared-kv-18", "shared-kv-22",
         "encoder-decoder-rejection"}


def factory_architectures(architecture_source, model_source):
    if "LLM_ARCH_NAMES =" not in architecture_source or "unsupported model architecture:" not in model_source:
        raise ValueError("cannot locate upstream architecture table or model factory")
    table = architecture_source.split("LLM_ARCH_NAMES =", 1)[1].split("};", 1)[0]
    names = dict(re.findall(r'\{\s*LLM_ARCH_(\w+)\s*,\s*"([^"]+)"', table))
    text = model_source.split("unsupported model architecture:", 1)[0]
    cases = set(re.findall(r"case LLM_ARCH_(\w+)\s*:", text))
    if not cases or cases - names.keys():
        raise ValueError("cannot reconcile upstream model factory")
    return set(names[name] for name in cases)


def contract_cases(text):
    text = re.sub(r"#[^\n]*", "", text)
    cases = {}
    for match in re.finditer(r"skippy_contract_case\(([^()]*)\)", text):
        fields = match[1].split()
        if len(fields) != 6 or fields[0] in cases or fields[5] not in MODES:
            raise ValueError("invalid or duplicate synthetic graph case")
        family, arch, layers, width, mtp, mode = fields
        cases[family] = (arch, int(layers), int(width), int(mtp), mode)
    tests = {}
    for match in re.finditer(r"add_test\(([^()]*)\)", text):
        fields = match[1].replace('"', '').split()
        if len(fields) >= 4 and fields[0] == "NAME":
            name = fields[1]
            if name in tests:
                raise ValueError("duplicate graph test name")
            tests[name] = fields[2:]
    for name, fields in tests.items():
        if "skippy-model-fixture-generator" not in fields or "--graph-contract" not in fields or "${" in name:
            continue
        test = tests.get(name.replace("skippy_generate_", "skippy_graph_", 1), [])
        if len(test) < 4 or test[1] != "skippy-graph-contract-models" or test[-1] not in MODES:
            raise ValueError("synthetic generator has no executable graph contract: " + name)
        def value(flag):
            return fields[fields.index(flag) + 1]
        offset = fields.index("--graph-contract")
        cases[name] = (value("--arch"), int(fields[offset + 1]), int(fields[offset + 2]),
                       int(value("--contract-mtp")), test[-1])
    if not cases:
        raise ValueError("no executable synthetic graph cases")
    return cases


def registry_errors(models, cases, policy):
    errors, seen = [], set()
    overrides = policy["case_modes"]
    for model in models:
        family, kind = model["family"], model["class"]
        if family in seen:
            errors.append(f"duplicate fixture family: {family}")
        seen.add(family)
        mode = overrides.get(family, "stateless" if kind in {"embedding", "rerank"} else "causal")
        execution = model["execution"]
        expected = (model["architecture"], execution["trunk_layers"], execution["activation_width"],
                    execution["mtp_layers"], mode)
        if cases.get(family) != expected:
            errors.append(f"{family}: synthetic graph case differs from registry: expected {expected}, got {cases.get(family)}")
    if set(overrides) - seen:
        errors.append("graph mode policy refers to an unknown fixture")
    return errors


def coverage(root, source, report, changed_sources=()):
    models = json.loads((root / MANIFEST).read_text())["models"]
    policy = json.loads((root / POLICY).read_text())
    if policy.get("schema_version") != 1:
        raise ValueError("invalid graph coverage policy")
    cases = contract_cases((source / TABLE).read_text())
    errors = registry_errors(models, cases, policy)
    architectures = factory_architectures((source / "src/llama-arch.cpp").read_text(),
                                          (source / "src/llama-model.cpp").read_text())
    uncovered = architectures - {case[0] for case in cases.values()}
    mapping = json.loads((root / "ci/llama-canary/generated-family-map.json").read_text())["families"]
    covered_sources = {path for family, paths in mapping.items() if family in cases for path in paths}
    # New builders conventionally use the architecture name. Shared/aliased
    # builders retain the explicit source map above.
    covered_sources.update("src/models/" + case[0].replace("_", "-") + ".cpp" for case in cases.values())
    for name, paths in policy["additional_case_sources"].items():
        if name not in cases:
            errors.append("missing supplemental synthetic case: " + name)
        else:
            covered_sources.update(paths)
    decoder_sources = set()
    for builder in report["builders"]:
        if (builder["verdict"] == "transformable"
                and builder.get("proof", {}).get("execution_scope") == "partitioned_decoder"):
            filename = builder["file"].replace("\\", "/")
            marker = filename.rfind("src/models/")
            if marker < 0:
                errors.append("decoder report has an invalid source path: " + filename)
            else:
                decoder_sources.add(filename[marker:])
    if not decoder_sources:
        errors.append("rewriter report contains no transformed decoder builders")
    missing = decoder_sources - covered_sources
    return {
        "schema_version": 1, "upstream_sha": (root / PIN).read_text().strip(),
        "registry_cases": len(models), "executable_cases": len(cases),
        "upstream_architectures": sorted(architectures),
        "architectures_without_graph_case": sorted(uncovered),
        "decoder_builders_without_graph_case": sorted(missing),
        "changed_decoder_builders_without_graph_case": sorted(missing & set(changed_sources)),
        "changed_model_sources": sorted(changed_sources), "errors": errors,
    }


def check(root, source, report, output, changed_sources=()):
    result = coverage(root, source, report, changed_sources)
    write_json(output, result)
    if result["architectures_without_graph_case"] or result["decoder_builders_without_graph_case"]:
        print("Graph coverage gaps (informational): "
              f'{len(result["architectures_without_graph_case"])} architectures, '
              f'{len(result["decoder_builders_without_graph_case"])} decoder builders, '
              f'{len(result["changed_decoder_builders_without_graph_case"])} changed uncovered builders. '
              f"Report: {output}")
    if result["errors"]:
        raise ValueError("graph coverage failed:\n  " + "\n  ".join(result["errors"]))
    return result


def check_upgrade_coverage(root, base_pin):
    source = root / ".deps/llama.cpp"
    pin = (root / PIN).read_text().strip()
    if base_pin != pin:
        command(["git", "-C", str(source), "fetch", "--no-tags", "--depth=1", "origin", base_pin], timeout=600)
    changed = git(source, "diff", "--name-only", base_pin, pin, "--", "src/models").splitlines()
    directory = root / "target/llama-regeneration"
    report = json.loads((directory / "report.json").read_text())
    check(root, source, report, directory / "graph-coverage.json", changed)
