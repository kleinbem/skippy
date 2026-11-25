"""Offline contracts for the shared qualification report renderer."""

import json
from pathlib import Path
import unittest

from lib.inference_charts import BAR_STYLES, Chart, matching_history, image_path
from lib.inference_markdown import comparison, dataset_card, dataset_charts, run_summary


EXAMPLES = Path(__file__).resolve().parents[2] / "docs/design/inference-qualification-examples"


class InferenceMarkdownTests(unittest.TestCase):
    def setUp(self):
        self.report = json.loads((EXAMPLES / "examples.json").read_text(encoding="utf-8"))

    def test_dataset_has_published_history_and_preserves_promoted_baseline(self):
        card = dataset_card(self.report)
        self.assertTrue(card.startswith("---\npretty_name:"))
        self.assertIn("## Recent measurements", card)
        self.assertIn("| Nightly AgentX (v1) | Qwen3.5-4B (1 Spark) | Output throughput | "
                      "26.71 tok/s | 30.14 tok/s | 30.42 tok/s | — | 26.5 tok/s |", card)
        self.assertIn("| Nov 24 14:00 | Nov 21 14:00 |", card)
        self.assertIn("| Output throughput | 30.42 tok/s | \x60bbbbbbbbbbbb\x60 |", card)
        self.assertNotIn("pr-pass", card)
        self.assertNotIn("pr-regression", card)
        self.assertNotIn("main-smoke-recorded", card)

    def test_dataset_leads_with_latest_results_in_preview_and_live_modes(self):
        preview = dataset_card(self.report)
        live = dataset_card({**self.report, "preview": False})
        self.assertIn("commit IDs on this card are illustrative", preview)
        self.assertNotIn("configs:", preview)
        self.assertNotIn("/baselines.json", preview)
        self.assertIn('path: "runs/*/*.parquet"', live)
        for card, heading in ((preview, "## Latest results (preview)"), (live, "## Latest results")):
            with self.subTest(heading=heading):
                start = card.index(heading)
                charts = card.index("## Performance over time")
                self.assertLess(start, charts)
                self.assertIn("| Output throughput | 26.71 tok/s | 30.42 tok/s |",
                              card[start:charts])

    def test_pr_verdicts_use_the_saved_baseline(self):
        summary = run_summary(self.report, "pr-pass")
        self.assertIn("| 62.1 tok/s | 63.32 tok/s | \x60aaaaaaaaaaaa\x60 | -1.93% | "
                      "Within tolerance |", summary)
        self.assertIn("| -0.17% | Within tolerance |", summary)
        self.assertIn("/blob/" + "f" * 40 + "/baselines.json", summary)
        failed = run_summary(self.report, "pr-regression")
        self.assertIn("**Failed**", failed)
        self.assertIn("| 60.1 tok/s | 63.32 tok/s | \x60aaaaaaaaaaaa\x60 | -5.09% | Regression |",
                      failed)

    def test_promotion_and_failed_run_show_what_happened_to_the_baseline(self):
        promoted = run_summary(self.report, "nightly-promotion")
        self.assertIn("## Baseline updates", promoted)
        self.assertIn("| Output throughput | Promoted | 30.42 tok/s |", promoted)
        failed = run_summary(self.report, "nightly-regression")
        self.assertIn("**Failed**", failed)
        self.assertIn("| -12.20% | Regression |", failed)
        self.assertNotIn("## Baseline updates", failed)

    def test_skipped_workflow_and_unavailable_receipt_are_distinct(self):
        skipped = run_summary(self.report, "pr-skipped")
        self.assertIn("**Skipped**", skipped)
        self.assertNotIn("Measurement unavailable", skipped)
        self.assertNotIn("## Performance charts", skipped)
        recorded = run_summary(self.report, "main-smoke-recorded")
        self.assertIn("| Qwen3.6-35B-A3B (2 Sparks) | Passed |", recorded)
        self.assertIn("Measurement unavailable · Timing receipt unavailable", recorded)
        self.assertIn("bar [260.23, 216.07, 612.17, 251.8, 40.59, 89.28]", recorded)
        chart_section = recorded.split("## Lab outcomes")[0]
        self.assertNotIn('"Qwen3.6-35B-A3B (2 Sparks)', chart_section)
        self.assertIn('"Qwen3.6-35B-A3B (1 Spark) — 251.8"', chart_section)

    def test_summary_chart_compares_saved_baseline_and_current_run(self):
        summary = run_summary(self.report, "pr-pass")
        self.assertIn("xychart-beta", summary)
        self.assertIn('x-axis ["Saved baseline — 63.32", "This run — 62.1"]', summary)
        self.assertIn('y-axis "tok/s" 0 --> 71', summary)
        self.assertIn("bar [63.32, 0]\n    bar [0, 62.1]", summary)
        self.assertIn("yAxis:\n      showLabel: false", summary)
        self.assertIn("Blue = equivalent (within tolerance)", summary)
        for run, verdict in (("pr-pass", "passed"), ("pr-regression", "regressed"),
                             ("nightly-promotion", "improved")):
            with self.subTest(run=run):
                color, label = BAR_STYLES[verdict]
                markdown = run_summary(self.report, run)
                self.assertIn(f'plotColorPalette: "#68778A, {color}"', markdown)
                self.assertIn(label, markdown)

    def test_zero_bar_has_an_explicit_value(self):
        chart = Chart("Decode throughput", "tok/s", ("This run",), (0,), "", ("regressed",))
        self.assertIn('x-axis ["This run — 0"]', chart.mermaid())

    def test_dataset_chart_uses_historical_baselines_and_embeds_pngs(self):
        charts = dataset_charts(self.report)
        history = charts[0]
        self.assertEqual(tuple(date.day for date in history.dates), (9, 12, 16, 19, 21, 24))
        self.assertEqual(history.values, (26.29, 26.5, None, 30.42, 30.14, 26.71))
        self.assertEqual(history.baseline, (26.29, 26.29, 26.29, 30.42, 30.42, 30.42))
        self.assertEqual(history.verdicts, (None, "passed", None, "improved", "passed", "regressed"))
        card = dataset_card(self.report)
        self.assertIn("## Performance over time", card)
        for index, chart in enumerate(charts, start=1):
            asset = image_path(index)
            self.assertIn(f"![{chart.title}]({asset})", card)
            self.assertEqual((EXAMPLES / asset).read_bytes()[:8], b"\x89PNG\r\n\x1a\n")

    def test_history_uses_the_latest_contiguous_benchmark_recipe(self):
        points = {
            0: {"contract_sha256": "a" * 64, "measured": 100},
            1: {"contract_sha256": "b" * 64, "measured": 80},
            2: {"measured": None},
            3: {"contract_sha256": "a" * 64, "measured": 90},
            4: {"measured": None},
            5: {"contract_sha256": "a" * 64, "measured": 92},
        }
        self.assertEqual(list(matching_history(points)), [2, 3, 4, 5])

    def test_verdict_requires_a_saved_baseline_and_promotion_an_improvement(self):
        row = {
            "metric": "decode_tok_s", "measured": 3, "reference": {"source_sha": "a" * 40, "value": 4},
        }
        with self.assertRaisesRegex(ValueError, "requires a measurement and a saved baseline"):
            comparison({**row, "reference": None, "verdict": "regressed"})
        with self.assertRaisesRegex(ValueError, "requires an improvement"):
            comparison({**row, "baseline_update": "promoted"})

    def test_zero_error_baseline_uses_percentage_points_and_escapes_notes(self):
        row = {
            "metric": "request_error_pct", "measured": 1.25,
            "reference": {"source_sha": "a" * 40, "value": 0},
            "note": "value | <tag>",
        }
        self.assertEqual(comparison(row)[4:], [
            "+1.25 pp", r"Change only · value \| &lt;tag&gt;",
        ])

    def test_review_examples_match_the_renderer(self):
        for example in sorted(EXAMPLES.glob("*.md")):
            if example.name != "README.md":
                with self.subTest(run=example.stem):
                    self.assertEqual(run_summary(self.report, example.stem),
                                     example.read_text(encoding="utf-8"))
        self.assertEqual(dataset_card(self.report), (EXAMPLES / "README.md").read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
