"""Offline checks for inference planning and protected placement."""

import io
import json
import os
import unittest
from pathlib import Path
from runpy import run_path
from tempfile import TemporaryDirectory
from urllib.error import HTTPError
from unittest.mock import Mock, patch

from lib import inference_ladder, inference_throughput
from lib.jy_qualification import Pilot, PilotHTTPError, placement


class LadderTests(unittest.TestCase):
    sha = "0123456789abcdef0123456789abcdef01234567"

    def test_short_profile(self):
        plan = inference_ladder.plan("pr", self.sha)
        rows = plan["rows"]
        self.assertEqual(len(rows), 7)
        self.assertEqual(len({row["architecture"] for row in rows}), 4)
        self.assertEqual(sum(row["mode"] == "split" for row in rows), 3)
        self.assertNotIn("qwen35-4b-split", {row["id"] for row in rows})
        self.assertEqual(plan["performance_claim"], "smoke-only")
        for row in rows:
            self.assertEqual(row["machines"], 2 if row["mode"] == "split" else 1)
            self.assertEqual(row["checks"], ["gpu-startup", "scored-quality", "streaming-smoke"])

    def test_extended_profiles(self):
        nightly = inference_ladder.plan("nightly", self.sha)["rows"]
        release = inference_ladder.plan("release", self.sha)["rows"]
        self.assertEqual(nightly, release)
        self.assertEqual(len({row["id"] for row in release}), 12)
        self.assertEqual({row["id"] for row in release if row["mode"] == "split"}, {
            "qwen38-27b-split", "qwen3-coder-30b-a3b-split",
            "qwen36-35b-a3b-split", "qwen38-flash-next-split",
        })

    def test_invalid_source_and_duplicate_model(self):
        for sha in ("main", "pr:1", "", "a" * 39 + "z"):
            with self.assertRaises(ValueError):
                inference_ladder.plan("pr", sha)
        ladder = inference_ladder.read_json(inference_ladder.LADDER)
        ladder["models"].append(ladder["models"][0])
        with patch.object(inference_ladder, "read_json", return_value=ladder):
            with self.assertRaises(ValueError):
                inference_ladder.plan("pr", self.sha)


class PilotRequestTests(unittest.TestCase):
    def test_jy_requests_identify_the_client(self):
        pilot = Pilot()
        with patch.object(pilot.opener, "open") as send:
            send.return_value.__enter__.return_value.read.return_value = b'{"ok":true}'
            self.assertEqual(pilot.request("POST", "/v1/auth/challenge", {}), {"ok": True})
        request = send.call_args.args[0]
        self.assertEqual(request.get_header("User-agent"), "skippy-inference-ci/1.0")

    def test_authentication_failures_name_the_endpoint_and_redact_the_response(self):
        pilot = Pilot()
        response = HTTPError("https://pilot.jianyang.cloud/v1/auth/login", 403, "Forbidden",
                             {}, io.BytesIO(b"sensitive authentication response"))
        with patch.object(pilot.opener, "open", side_effect=response):
            with self.assertRaisesRegex(RuntimeError,
                                        r"^JY HTTP 403: /v1/auth/login authentication request failed$"):
                pilot.request("POST", "/v1/auth/login", {})


class ThroughputTests(unittest.TestCase):
    run_id = "run_test"
    prefix = f"/v1/artifacts/{run_id}/raw/"

    def test_late_artifact_produces_throughput_receipt(self):
        sha = "a" * 40
        model = "qwen35-4b"
        row = {"id": model + "-standalone", "mode": "standalone"}
        job = {"id": "job_test", "repo_rev": sha, "status": "succeeded",
               "runs": [{"id": self.run_id, "status": "succeeded"}]}
        qualification = {"source_sha": sha, "model": model, "mode": "standalone",
                         "profile": "pr", "status": "passed"}
        cases = json.loads(Path("ci/inference-quality.json").read_text())
        response = {"timings": {"prompt_n": 3, "prompt_ms": 2, "predicted_n": 1, "predicted_ms": 3},
                    "usage": {"prompt_tokens_details": {"cached_tokens": 1}}}
        samples = [{"case": case, "response": response, "passed": True} for case in cases]
        quality_path = self.prefix + "quality.json"
        missing = HTTPError("https://pilot.jianyang.cloud" + quality_path, 404, "Not Found", {},
                            io.BytesIO(b'{"error":"not_found","message":"artifact not found"}'))
        pilot = Pilot()
        replies = [io.BytesIO(json.dumps(qualification).encode()), missing,
                   io.BytesIO(json.dumps(samples).encode())]
        with TemporaryDirectory() as directory, patch.object(pilot, "auth", return_value="test"), \
                patch.object(pilot.opener, "open", side_effect=replies) as send, \
                patch.object(inference_throughput.time, "sleep") as sleep:
            output = Path(directory)
            inference_throughput.collect(pilot, job, row, sha, output, model)
            receipt = json.loads((output / (row["id"] + "-throughput.json")).read_text())

        self.assertEqual(receipt["run_id"], self.run_id)
        self.assertEqual(receipt["requests"], len(cases))
        self.assertEqual(receipt["prefill_tokens"], 2 * len(cases))
        self.assertEqual(receipt["decode_ms"], 3 * len(cases))
        self.assertEqual([call.args[0].selector for call in send.call_args_list],
                         [self.prefix + "qualification.json", quality_path, quality_path])
        sleep.assert_called_once_with(inference_throughput.ARTIFACT_RETRY_DELAY)

    def test_persistent_404_stops_and_names_the_artifact(self):
        path = self.prefix + "qualification.json"
        pilot = Mock(spec=Pilot)
        pilot.api.side_effect = PilotHTTPError(path, 404, "artifact not found")
        with patch.object(inference_throughput.time, "sleep") as sleep:
            with self.assertRaisesRegex(RuntimeError, r"run_test/qualification\.json: JY HTTP 404"):
                inference_throughput.fetch_artifact(pilot, self.run_id, "qualification.json")
        self.assertEqual(pilot.api.call_count, inference_throughput.ARTIFACT_RETRIES + 1)
        self.assertEqual(sleep.call_count, inference_throughput.ARTIFACT_RETRIES)

    def test_other_http_errors_are_not_retried(self):
        for path, status in [(self.prefix + "quality.json", 403), ("/v1/auth/login", 404)]:
            with self.subTest(path=path, status=status):
                pilot = Mock(spec=Pilot)
                pilot.api.side_effect = PilotHTTPError(path, status, "request failed")
                with patch.object(inference_throughput.time, "sleep") as sleep:
                    with self.assertRaisesRegex(RuntimeError, rf"run_test/quality\.json: JY HTTP {status}"):
                        inference_throughput.fetch_artifact(pilot, self.run_id, "quality.json")
                pilot.api.assert_called_once_with("GET", self.prefix + "quality.json")
                sleep.assert_not_called()


class PlacementTests(unittest.TestCase):
    def test_two_distinct_hosts_on_one_network(self):
        def agent(name, network, runs):
            return {"id": name, "node": name, "online": True, "enabled": True, "drain": False,
                    "network": network, "host": {"gpu": "NVIDIA GB10"}, "active_runs": runs}

        source = "a" * 40
        agents = {"agents": [
            agent("controller", "dan-lab", []),
            agent("first", "dan-lab", [{"job_name": "skippy-inference-standalone-" + source[:12]}]),
            agent("second", "dan-lab", []),
            agent("elsewhere", "james-lab", []),
        ]}
        self.assertEqual(placement(agents, "controller", 0, source), ("first", "second"))
        with self.assertRaises(ValueError):
            placement(agents, "controller", 0, "b" * 40)


class ReleaseAdmissionTests(unittest.TestCase):
    def test_only_the_staged_commit_from_protected_release_dispatch_is_admitted(self):
        admission = run_path(str(Path(__file__).resolve().parents[1] / "inference-plan.py"))
        admit = admission["admit_release_candidate"]
        candidate, base = "a" * 40, "b" * 40
        env = {
            "GITHUB_EVENT_NAME": "workflow_dispatch",
            "GITHUB_REF": "refs/heads/main",
            "GITHUB_WORKFLOW_REF": "Mesh-LLM/skippy/.github/workflows/release.yml@refs/heads/main",
            "GITHUB_RUN_ID": "123",
            "GITHUB_SHA": base,
        }
        ref = {"object": {"sha": candidate}}
        commit = {"parents": [{"sha": base}], "message": "Release\n\nSkippy-Release-Run: 123"}
        with patch.dict(os.environ, env):
            with patch.dict(admit.__globals__, github=lambda path: ref if "/ref/" in path else commit):
                admit(candidate)
                with self.assertRaisesRegex(ValueError, "does not belong"):
                    admit("c" * 40)
                with patch.dict(os.environ, GITHUB_REF="refs/heads/branch"):
                    with self.assertRaisesRegex(ValueError, "protected Release dispatch"):
                        admit(candidate)


if __name__ == "__main__":
    unittest.main()
