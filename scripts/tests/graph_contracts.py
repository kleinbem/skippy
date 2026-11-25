"""Exercise graph coverage without model weights, native compilation or downloads."""

import copy
import json
import re
import subprocess
import tempfile
import unittest
from pathlib import Path

from lib.ci_artifacts import read_json
from lib.graph_contracts import MANIFEST, POLICY, TABLE, check, contract_cases, coverage, factory_architectures, registry_errors

ROOT = Path(__file__).resolve().parents[2]


def patched_table():
    patches = list(sorted((ROOT / "llama_cpp/patches").glob("*.patch")))
    for name in ("model_support", "generated"):
        directory = ROOT / "llama_cpp/patches" / name
        patches += [directory / line for line in (directory / "series").read_text().splitlines() if line]
    with tempfile.TemporaryDirectory() as tmp:
        for patch in patches:
            if f"diff --git a/{TABLE} ".encode() in patch.read_bytes():
                subprocess.run(["git", "apply", "--include=" + TABLE, str(patch)], cwd=tmp, check=True)
        return (Path(tmp) / TABLE).read_text()


class GraphCoverage(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.table = patched_table()
        cls.cases = contract_cases(cls.table)
        cls.models = read_json(ROOT / MANIFEST)["models"]
        cls.policy = read_json(ROOT / POLICY)

    def test_ordered_native_patches_match_every_registry_dimension_and_mode(self):
        self.assertEqual(registry_errors(self.models, self.cases, self.policy), [])
        self.assertEqual(len(self.models), 95)
        self.assertGreater(len(self.cases), len(self.models))

    def test_missing_duplicate_or_stale_graph_cases_fail(self):
        for index in range(5):
            cases = copy.deepcopy(self.cases)
            value = list(cases["qwen3-dense"])
            value[index] = "wrong" if isinstance(value[index], str) else value[index] + 1
            cases["qwen3-dense"] = tuple(value)
            self.assertTrue(registry_errors(self.models, cases, self.policy))
        cases = dict(self.cases)
        del cases["qwen3-dense"]
        self.assertTrue(registry_errors(self.models, cases, self.policy))
        with self.assertRaisesRegex(ValueError, "duplicate"):
            contract_cases(self.table + "\nskippy_contract_case(llama llama 16 2048 0 causal)\n")

    def test_a_generated_fixture_without_its_execution_test_is_not_coverage(self):
        damaged = re.sub(r"add_test\(NAME skippy_graph_contract_glm53_synthetic.*?\)", "", self.table, flags=re.S)
        with self.assertRaisesRegex(ValueError, "no executable graph contract"):
            contract_cases(damaged)

    def fixture(self, root):
        for name in (MANIFEST, POLICY, "ci/llama-canary/generated-family-map.json", "llama_cpp/upstream.txt"):
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes((ROOT / name).read_bytes())
        source = root / "source"
        (source / TABLE).parent.mkdir(parents=True)
        (source / TABLE).write_text(self.table)
        architectures = sorted({"uncovered-existing"} | {case[0] for case in self.cases.values()})
        self.factory(source, architectures)
        series = read_json(ROOT / "llama_cpp/patches/generated/series.json")
        report = {"builders": [{"file": path, "verdict": "transformable", "proof": {"execution_scope": "partitioned_decoder"}}
                               for path in sorted({path for shard in series["shards"] for path in shard["sources"]})]}
        return source, report, architectures

    def factory(self, source, names):
        (source / "src/llama-arch.cpp").write_text('LLM_ARCH_NAMES = {' + ','.join(
            f'{{LLM_ARCH_A{i}, "{name}"}}' for i, name in enumerate(names)) + '};')
        (source / "src/llama-model.cpp").write_text(''.join(
            f'case LLM_ARCH_A{i}: return make_model();' for i in range(len(names))) +
            'throw("unsupported model architecture:");')

    def test_existing_gaps_are_reported_without_claiming_universal_coverage(self):
        with tempfile.TemporaryDirectory() as tmp:
            source, report, _ = self.fixture(Path(tmp))
            result = check(Path(tmp), source, report, Path(tmp) / "result.json")
            self.assertEqual(result["errors"], [])
            self.assertTrue(result["architectures_without_graph_case"])
            self.assertTrue(result["decoder_builders_without_graph_case"])

    def test_three_new_architectures_are_reported_without_blocking(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source, report, architectures = self.fixture(root)
            additions = ["new-arch-a", "new-arch-b", "new-arch-c"]
            self.factory(source, architectures + additions)
            for name in additions:
                report["builders"].append({"file": f"src/models/{name}.cpp", "verdict": "transformable",
                                           "proof": {"execution_scope": "partitioned_decoder"}})
            result = check(root, source, report, root / "result.json")
            self.assertEqual(result["errors"], [])
            self.assertTrue(set(additions) <= set(result["architectures_without_graph_case"]))
            self.assertTrue({f"src/models/{name}.cpp" for name in additions} <=
                            set(result["decoder_builders_without_graph_case"]))
            with (source / TABLE).open("a") as stream:
                stream.write("\nskippy_contract_case(new-probe new-arch-a 2 8 0 causal)\n")
            result = coverage(root, source, report)
            self.assertEqual(result["errors"], [])
            self.assertNotIn("new-arch-a", result["architectures_without_graph_case"])
            self.assertIn("new-arch-b", result["architectures_without_graph_case"])

    def test_changed_uncovered_builder_is_reported_without_blocking(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source, report, _ = self.fixture(root)
            result = coverage(root, source, report)
            changed = result["decoder_builders_without_graph_case"][0]
            output = root / "result.json"
            check(root, source, report, output, [changed])
            result = read_json(output)
            self.assertIn(changed, result["changed_model_sources"])
            self.assertEqual(result["changed_decoder_builders_without_graph_case"], [changed])
            self.assertEqual(result["errors"], [])

    def test_new_builder_gap_is_reported_with_unchanged_architecture_inventory(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source, report, _ = self.fixture(root)
            report["builders"].append({"file": "src/models/untested-variant.cpp", "verdict": "transformable",
                                       "proof": {"execution_scope": "partitioned_decoder"}})
            result = check(root, source, report, root / "result.json")
            self.assertEqual(result["errors"], [])
            self.assertIn("src/models/untested-variant.cpp", result["decoder_builders_without_graph_case"])

    def test_uncovered_additions_do_not_hide_existing_contract_regressions(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source, report, architectures = self.fixture(root)
            self.factory(source, architectures + ["new-arch"])
            (source / TABLE).write_text(self.table.replace(
                "skippy_contract_case(qwen3-dense qwen3 28 1024 0 causal)", ""))
            output = root / "result.json"
            with self.assertRaisesRegex(ValueError, "qwen3-dense"):
                check(root, source, report, output)
            result = read_json(output)
            self.assertTrue(result["errors"])
            self.assertIn("new-arch", result["architectures_without_graph_case"])

    def test_empty_rewriter_report_and_unknown_factory_enum_fail(self):
        with tempfile.TemporaryDirectory() as tmp:
            source, _, _ = self.fixture(Path(tmp))
            self.assertTrue(coverage(Path(tmp), source, {"builders": []})["errors"])
        with self.assertRaisesRegex(ValueError, "reconcile"):
            factory_architectures('LLM_ARCH_NAMES = {{LLM_ARCH_A, "a"}};',
                                  'case LLM_ARCH_MISSING: break; throw("unsupported model architecture:");')
        with self.assertRaisesRegex(ValueError, "locate"):
            factory_architectures("changed upstream format", "changed upstream format")


if __name__ == "__main__":
    unittest.main()
