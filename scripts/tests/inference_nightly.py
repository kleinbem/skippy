"""Offline contracts for collecting and comparing nightly inference metrics."""

import json
import os
from pathlib import Path
from runpy import run_path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import Mock, patch

from lib.ci_artifacts import write_json
from lib.inference_baselines import apply_baselines, classify
from lib.inference_inputs import input_hashes
from lib.inference_ladder import matrix
from lib.inference_admission import split_admissions_digest
from lib.inference_nightly_metrics import agentx_metrics, await_artifacts, collect_nightly, contract
from lib.inference_nightly_results import nightly_run, read_configuration


SOURCE = "a" * 40
PR_PATH = Path(__file__).resolve().parents[1] / "inference-publish-nightly.py"


def admissions_fixture():
    return {"schema_version": 2, "stages": [
        {"package_id": "fixture", "plan_id": f"plan-{start}", "layer_start": start,
         "layer_end": start + 2, "resident_tensor_ids": [start], "profiles": [{"profile_id": "decode"}]}
        for start in (0, 2)]}


def trial(throughput, latency, requests):
    return {
        "aiperf_version": "0.13.0", "is_complete": True, "error_summary": [],
        "metadata": {"submission_valid": True}, "request_error_rate": {"avg": 0},
        "request_count": {"avg": requests}, "output_token_throughput": {"avg": throughput},
        "time_to_first_token": {"p95": latency}, "inter_token_latency": {"p95": 10},
    }


def smoke_receipt(root, sha, run_id, configuration="qwen35-4b-standalone", value=100):
    entry = next(row for row in matrix("nightly", sha, include_smoke=True)
                 if (row["profile"], row["configuration"]) == ("pr", configuration))
    directory = root / f'inference-{entry["id"]}-{run_id}-1'
    directory.mkdir(parents=True)
    job = {"id": "job_fixture", "repo_rev": sha, "status": "succeeded",
           "runs": [{"id": "run_fixture", "status": "succeeded"}]}
    admission_hash = None
    if entry["mode"] == "split":
        admission_hash = split_admissions_digest(admissions_fixture())
        write_json(directory / "split-admissions.json", admissions_fixture())
        job["formation_id"] = "frm_fixture"
        write_json(directory / f"{configuration}-formation.json", {
            "id": "frm_fixture", "status": "succeeded",
            "members": [{"job_id": "job_fixture", "role": "leader", "status": "succeeded"},
                        {"job_id": "job_worker", "role": "worker", "status": "succeeded"}],
        })
    write_json(directory / f"{configuration}-job.json", job)
    model = configuration.removesuffix(f'-{entry["mode"]}')
    write_json(directory / "measurements.json", {
        "schema_version": 1, "source_sha": sha, "configuration": configuration, "profile": "pr",
        "job_id": "job_fixture", "run_id": "run_fixture", "split_admissions_sha256": admission_hash,
        "contract_sha256": contract("pr", entry, model),
        "metrics": {"prefill_tok_s": value, "decode_tok_s": value},
    })


class NightlyTests(unittest.TestCase):
    def test_every_profile_requires_native_stage_admissions_without_an_override(self):
        prepare_split = run_path(str(PR_PATH.with_name("inference-execute.py")))["prepare_split"]
        peers = json.dumps([{"role": "leader", "host": "192.0.2.1", "node": "leader"},
                            {"role": "worker", "host": "192.0.2.2", "node": "worker"}])
        for profile in ("pr", "nightly", "release"):
            for valid in (True, False):
                with self.subTest(profile=profile, valid=valid), TemporaryDirectory() as temporary:
                    def plan_split(args, *, timeout):
                        plan_dir = Path(args[args.index("--output-dir") + 1])
                        plan_dir.mkdir()
                        write_json(plan_dir / "admissions.json", admissions_fixture() if valid else {})
                        write_json(plan_dir / "stage-1.json", {})
                    command = Mock(side_effect=plan_split)
                    with patch.dict(prepare_split.__globals__, command=command), patch.dict(os.environ, JIANYANG_PEERS=peers):
                        args = (Path("model.gguf"), "test-model", profile, 65536, Path(temporary), True)
                        if valid:
                            self.assertEqual(len(prepare_split(*args)), 2)
                        else:
                            with self.assertRaisesRegex(ValueError, "native stage admissions"):
                                prepare_split(*args)
                    self.assertNotIn("--allow-uncertified-split", command.call_args.args[0])

    def test_nightly_split_retains_native_admission_after_valid_agentx_trials(self):
        key = "qwen38-27b"
        configuration = f"{key}-split"
        entry = next(row for row in matrix("nightly", SOURCE) if row["id"] == configuration)
        job = {"id": "job_leader", "repo_rev": SOURCE, "status": "succeeded", "formation_id": "frm_fixture",
               "runs": [{"id": "run_fixture", "status": "succeeded"}]}
        qualification = {"source_sha": SOURCE, "model": key, "mode": "split", "profile": "nightly",
                         "status": "passed", "artifact_manifest_sha256": input_hashes()["model_manifest_sha256"],
                         "gpu": "NVIDIA GB10", "split_admissions_sha256": split_admissions_digest(admissions_fixture())}
        formation = {"id": "frm_fixture", "status": "succeeded",
                     "members": [{"role": "leader", "job_id": "job_leader", "status": "succeeded"},
                                 {"role": "worker", "job_id": "job_worker", "status": "succeeded"}]}
        pilot = Mock()
        pilot.api.side_effect = [qualification, trial(20, 1200, 70), trial(22, 1000, 60), admissions_fixture()]
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            directory = root / f"inference-{configuration}-23-1"
            directory.mkdir()
            collect_nightly(pilot, job, entry, SOURCE, directory, key, "nightly")
            write_json(directory / f"{configuration}-job.json", job)
            write_json(directory / f"{configuration}-formation.json", formation)
            result = read_configuration(root, entry, SOURCE, "23", "1")
            self.assertEqual(result["outcome"], "passed")
            self.assertEqual(result["workload"], "DGX Spark AgentX nightly v1")
            self.assertEqual(result["metrics"][0]["measured"], 21)
            self.assertEqual(result["metrics"][0]["contract_sha256"], contract("nightly", entry, key))

    def test_matrix_preserves_existing_profiles_and_uses_pr_smoke(self):
        ordinary = matrix("nightly", SOURCE)
        both = matrix("nightly", SOURCE, include_smoke=True)
        self.assertEqual(both[:12], ordinary)
        self.assertEqual(len(both), 19)
        self.assertEqual(len({row["id"] for row in both}), 19)
        self.assertEqual(
            [{**row, "id": row["configuration"]} for row in both[12:]], matrix("pr", SOURCE),
        )
        with self.assertRaises(ValueError):
            matrix("release", SOURCE, include_smoke=True)

    def test_metric_direction_and_exact_tolerance(self):
        self.assertEqual(classify("decode_tok_s", 95, 100), "passed")
        self.assertEqual(classify("decode_tok_s", 94.99, 100), "regressed")
        self.assertEqual(classify("ttft_p95_ms", 105.01, 100), "regressed")
        self.assertEqual(classify("ttft_p95_ms", 94, 100), "improved")

    def test_promotes_independent_improvement_and_never_walks_baseline_down(self):
        metrics = [
            {"workload": "smoke", "metric": name, "measured": 100, "contract_sha256": "c" * 64}
            for name in ("prefill_tok_s", "decode_tok_s")
        ]
        config = {"id": "example", "outcome": "passed", "metrics": metrics}

        def run(number, sha, values, saved):
            for metric, value in zip(metrics, values):
                metric["measured"] = value
            return apply_baselines({"id": str(number), "source_sha": sha, "outcome": "passed",
                                    "configurations": [config]}, saved)

        _, seeded = run(1, SOURCE, [100, 100], [])
        _, passing = run(2, "b" * 40, [97, 97], seeded)
        self.assertEqual(passing[0]["value"], 100)
        self.assertEqual(passing[0]["latest_passing"]["value"], 97)
        failed, next_baselines = run(3, "d" * 40, [94, 110], passing)
        self.assertEqual(failed["outcome"], "failed")
        self.assertEqual({item["metric"]: item["value"] for item in next_baselines},
                         {"prefill_tok_s": 100, "decode_tok_s": 110})
        same_source, replayed = run(4, "d" * 40, [100, 120], next_baselines)
        self.assertEqual(replayed[0]["value"], 110)
        self.assertEqual(same_source["configurations"][0]["metrics"][1]["verdict"], "improved")
        metrics[1]["contract_sha256"] = "d" * 64
        updated, fresh = run(5, "e" * 40, [100, 80], replayed)
        self.assertEqual(updated["outcome"], "passed")
        self.assertEqual(fresh[0]["value"], 80)
        self.assertEqual(updated["configurations"][0]["metrics"][1]["baseline_update"], "seeded")

    def test_agentx_aggregates_two_valid_trials(self):
        first = trial(20, 1200, 70)
        second = trial(22, 1000, 60)
        self.assertEqual(agentx_metrics([first, second]), {
            "output_tok_s": 21, "ttft_p95_ms": 1100, "itl_p95_ms": 10,
            "requests": 130, "request_error_pct": 0,
        })
        second["request_error_rate"]["avg"] = 5
        with self.assertRaisesRegex(ValueError, "unsuccessful"):
            agentx_metrics([first, second])

    def test_artifact_retry_is_limited_to_not_found(self):
        with patch("lib.inference_nightly_metrics.time.sleep"):
            read = Mock(side_effect=[RuntimeError("JY HTTP 404: not found"), True])
            self.assertTrue(await_artifacts(read))
            self.assertEqual(read.call_count, 2)
            read = Mock(side_effect=RuntimeError("JY HTTP 403: forbidden"))
            with self.assertRaisesRegex(RuntimeError, "403"):
                await_artifacts(read)
            self.assertEqual(read.call_count, 1)

    def test_receipts_are_bound_to_source_and_missing_evidence_fails(self):
        with TemporaryDirectory() as temporary, patch("lib.inference_nightly_results.print", create=True):
            root = Path(temporary)
            smoke_receipt(root, SOURCE, "23", configuration="qwen3-coder-30b-a3b-split")
            report = nightly_run(root, SOURCE, "23", "1", "2026-11-21T00:00:00Z", True)
            split = next(config for config in report["configurations"]
                         if config["id"] == "qwen3-coder-30b-a3b-split" and config["workload"] == "DGX Spark smoke v1")
            self.assertEqual(split["outcome"], "passed")
            self.assertEqual(report["outcome"], "failed")
            self.assertEqual(len(report["configurations"]), 19)
            wrong_source = nightly_run(root, "b" * 40, "23", "1", report["started_at"], True)
            self.assertTrue(all(config["outcome"] != "passed" for config in wrong_source["configurations"]))

    def test_only_exact_protected_schedule_can_publish(self):
        admit = run_path(str(PR_PATH))["admit"]
        environment = {
            "GITHUB_EVENT_NAME": "schedule", "GITHUB_REPOSITORY": "Mesh-LLM/skippy",
            "GITHUB_REF": "refs/heads/main",
            "GITHUB_WORKFLOW_REF": "Mesh-LLM/skippy/.github/workflows/inference.yml@refs/heads/main",
            "GITHUB_WORKFLOW_SHA": SOURCE, "GITHUB_SHA": SOURCE, "SOURCE_SHA": SOURCE,
            "GITHUB_RUN_ID": "23", "GITHUB_RUN_ATTEMPT": "1", "GITHUB_RUN_NUMBER": "34",
            "RUN_STARTED_AT": "2026-11-21T00:00:00Z", "MATRIX_RESULT": "failure",
        }
        self.assertEqual(admit(environment), (SOURCE, "23", "1", 34, environment["RUN_STARTED_AT"], False))
        for name in ("pull_request_target", "workflow_dispatch", "push", "workflow_call"):
            with self.assertRaisesRegex(ValueError, "only scheduled"):
                admit({**environment, "GITHUB_EVENT_NAME": name})
        with self.assertRaisesRegex(ValueError, "must match"):
            admit({**environment, "SOURCE_SHA": "b" * 40})


if __name__ == "__main__":
    unittest.main()
