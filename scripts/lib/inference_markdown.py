"""Render the same qualification evidence for GitHub Actions and Hugging Face.

The input contains one measurement per run, configuration, workload, and metric.
A reference value comes from the pinned Hugging Face snapshot. Qualification
supplies the verdict after applying the configured tolerance.
"""

from datetime import datetime, timezone
from html import escape
from math import isfinite
import re

from .inference_charts import HISTORY_DAYS, HistoryChart, history_charts, image_path, run_charts
from .inference_labels import configuration_label, workload_label


HF_DATASET = "https://huggingface.co/datasets/meshllm/skippy-inference-qualification"
METRICS = {
    "prefill_tok_s": ("Prefill throughput", "tok/s"),
    "decode_tok_s": ("Decode throughput", "tok/s"),
    "output_tok_s": ("Output throughput", "tok/s"),
    "ttft_p95_ms": ("Time to first token (p95)", "ms"),
    "itl_p95_ms": ("Time between tokens (p95)", "ms"),
    "requests": ("Requests", ""),
    "request_error_pct": ("Request error rate", "%"),
}
OUTCOMES = {
    "passed": "Passed",
    "failed": "Failed",
    "skipped": "Skipped",
    "cancelled": "Cancelled",
    "incomplete": "Incomplete",
}
VERDICTS = {
    "passed": "Within tolerance",
    "regressed": "Regression",
    "improved": "Improvement",
    "inconclusive": "Inconclusive",
}
SHA = re.compile(r"[0-9a-f]{40}")
HTTPS = re.compile(r"https://[a-zA-Z0-9.-]+[/a-zA-Z0-9_.~%?&=#:+-]*")
UNAVAILABLE = "—"


def plain(value):
    if not isinstance(value, str):
        raise ValueError("expected report text")
    return re.sub(r"([\\\x60*_[\]~|])", r"\\\1", escape(" ".join(value.split()), quote=False))


def link(label, url):
    if not url:
        return label
    if not HTTPS.fullmatch(url):
        raise ValueError(f"expected an HTTPS report link, got {url!r}")
    return f"[{label}]({url})"


def sha(value):
    if not SHA.fullmatch(value):
        raise ValueError(f"expected a full commit SHA, got {value!r}")
    return f"\x60{value[:12]}\x60"


def utc(value):
    stamp = datetime.fromisoformat(value.replace("Z", "+00:00"))
    if stamp.tzinfo is None:
        raise ValueError(f"run timestamp needs a time zone: {value!r}")
    return stamp.astimezone(timezone.utc)


def table(headers, rows):
    return ["| " + " | ".join(row) + " |" for row in (headers, ["---"] * len(headers), *rows)]


def number(value):
    if type(value) not in (int, float) or not isfinite(value) or value < 0:
        raise ValueError(f"expected a finite, nonnegative measurement, got {value!r}")
    return value


def measurement(metric, value):
    if value is None:
        return UNAVAILABLE
    unit = METRICS[metric][1]
    formatted = f"{number(value):,.2f}".rstrip("0").rstrip(".")
    if unit == "%":
        return formatted + unit
    return " ".join(part for part in (formatted, unit) if part)


def delta(metric, current, reference):
    difference = current - reference
    if metric == "request_error_pct":
        return f"{difference:+.2f} pp"
    if reference == 0:
        unit = METRICS[metric][1]
        return f"{difference:+,.2f}" + (f" {unit}" if unit else "")
    return f"{(difference / reference) * 100:+.2f}%"


def comparison(row):
    metric, measured = row["metric"], row["measured"]
    reference = row.get("reference")
    verdict = row.get("verdict")
    if verdict is not None:
        if verdict not in VERDICTS:
            raise ValueError(f"unknown baseline verdict: {verdict!r}")
        if reference is None or measured is None:
            raise ValueError("a verdict requires a measurement and a saved baseline")
    update = row.get("baseline_update")
    if update == "seeded" and (measured is None or reference is not None):
        raise ValueError("seeding a baseline requires a first measurement")
    if update == "promoted" and verdict != "improved":
        raise ValueError("promoting a baseline requires an improvement")
    if update not in (None, "seeded", "promoted"):
        raise ValueError(f"unknown baseline update: {update!r}")

    current = measurement(metric, measured)
    expected = reference_label = change = UNAVAILABLE
    if reference is not None:
        expected = measurement(metric, number(reference["value"]))
        reference_label = sha(reference["source_sha"])
        if measured is not None:
            change = delta(metric, measured, reference["value"])
    if measured is None:
        outcome = "Measurement unavailable"
    elif reference is None:
        outcome = "No matching baseline"
    else:
        outcome = VERDICTS.get(verdict, "Change only")
    if row.get("note"):
        outcome += f' · {plain(row["note"])}'
    return [METRICS[metric][0], current, expected, reference_label, change, outcome]


def run_label(run):
    return link(plain(run["id"]), run.get("url"))


def outcome_table(run):
    with_workload = any(config.get("workload") for config in run["configurations"])
    headers = ["Configuration"]
    if with_workload:
        headers.append("Workload")
    results = []
    for config in run["configurations"]:
        labels = [plain(configuration_label(config["id"]))]
        if with_workload:
            labels.append(plain(workload_label(config["workload"])))
        results.append([
            *labels, OUTCOMES[config["outcome"]],
            link("Lab record", config.get("url")) if config.get("url") else UNAVAILABLE,
        ])
    return table([*headers, "Qualification", "Evidence"], results)


def run_sections(run, heading, charts=(), *, show_baseline_link=True):
    sections = [
        f'**{OUTCOMES[run["outcome"]]}** · {plain(run["profile"])} profile · '
        f'{sha(run["source_sha"])} · {utc(run["started_at"]):%Y-%m-%d %H:%M UTC} · Run {run_label(run)}',
    ]
    if show_baseline_link and run.get("baseline_revision"):
        revision = run["baseline_revision"]
        sections.extend([
            "", "Published baseline snapshot " + link(
                sha(revision), f"{HF_DATASET}/blob/{revision}/baselines.json",
            ),
        ])
    if run.get("note"):
        sections.extend(["", plain(run["note"])])
    if not run["configurations"]:
        if run["outcome"] != "skipped" and not run.get("note"):
            sections.extend(["", "Configuration evidence unavailable."])
        return sections

    if charts:
        sections.extend(["", f"{heading} Performance charts"])
        for chart in charts:
            sections.extend(["", plain(chart.caption), "", "\x60\x60\x60mermaid",
                             chart.mermaid().rstrip(), "\x60\x60\x60"])

    sections.extend(["", f"{heading} Lab outcomes", "", *outcome_table(run)])
    workloads = {}
    updates = []
    seen = set()
    for config in run["configurations"]:
        for row in config["metrics"]:
            workload, config_id, metric = row["workload"], config["id"], row["metric"]
            key = (workload, config_id, metric)
            if key in seen:
                raise ValueError(f"duplicate qualification measurement: {key!r}")
            seen.add(key)
            display_config = plain(configuration_label(config_id))
            workloads.setdefault(workload, []).append([display_config, *comparison(row)])
            if row.get("baseline_update"):
                if run.get("published") is not True or config["outcome"] != "passed":
                    raise ValueError("baseline updates require a published run and passing configuration")
                action = {"seeded": "Seeded", "promoted": "Promoted"}[row["baseline_update"]]
                updates.append([
                    display_config, plain(workload_label(workload)), METRICS[metric][0], action,
                    measurement(metric, row["measured"]),
                ])
    for workload, rows in workloads.items():
        sections.extend([
            "", f"{heading} {plain(workload_label(workload))}", "",
            *table(["Configuration", "Metric", "This run", "Saved baseline", "Baseline commit",
                    "Change vs saved", "Comparison"], rows),
        ])
    if updates:
        sections.extend([
            "", f"{heading} Baseline updates", "",
            *table(["Configuration", "Workload", "Metric", "Action", "New baseline"], updates),
        ])
    return sections


def ordered_runs(report):
    if type(report["schema_version"]) is not int or report["schema_version"] != 1:
        raise ValueError("unsupported qualification report schema version")
    runs = report["runs"]
    if len({run["id"] for run in runs}) != len(runs):
        raise ValueError("duplicate qualification run ID")
    return sorted(runs, key=lambda run: utc(run["started_at"]), reverse=True)


def run_summary(report, run_id):
    for run in ordered_runs(report):
        if run["id"] == run_id:
            charts = run_charts(run, METRICS)
            lines = ["# Inference qualification", "", *run_sections(run, "##", charts)]
            return "\n".join([*lines, ""])
    raise ValueError(f"unknown qualification run ID: {run_id!r}")


def baseline_table(baselines):
    rows = [
        [plain(configuration_label(item["configuration"])), plain(workload_label(item["workload"])),
         METRICS[item["metric"]][0],
         measurement(item["metric"], number(item["value"])), sha(item["source_sha"])]
        for item in baselines
    ]
    return table(["Configuration", "Workload", "Metric", "Baseline", "Reference SHA"], rows)


def coverage(run):
    configs = run["configurations"]
    if not configs:
        return UNAVAILABLE
    passed = sum(config["outcome"] == "passed" for config in configs)
    return f"{passed}/{len(configs)} configurations passed"


def metric_history(runs):
    series = {}
    for column, run in enumerate(runs):
        for config in run["configurations"]:
            for row in config["metrics"]:
                key = (row["workload"], config["id"], row["metric"])
                values = series.setdefault(key, [UNAVAILABLE] * len(runs))
                values[column] = measurement(row["metric"], row["measured"])
    return [
        [plain(workload_label(workload)), plain(configuration_label(config)), METRICS[metric][0], *values]
        for (workload, config, metric), values in sorted(series.items())
    ]


def published_runs(report):
    return [run for run in ordered_runs(report) if run.get("published") is True]


def dataset_charts(report):
    runs = published_runs(report)
    if not runs:
        return []
    timeline = [(utc(run["started_at"]), run) for run in reversed(runs)]
    return history_charts(timeline, METRICS) or run_charts(runs[0], METRICS)


def dataset_card(report, charts=None):
    runs = published_runs(report)
    preview = report.get("preview") is True
    lines = [
        "---", 'pretty_name: "Skippy inference qualification"',
        "tags:", "- benchmark", "- tabular",
    ]
    if runs and not preview:
        lines.extend([
            "configs:", "  - config_name: default", "    data_files:",
            '      - split: runs', '        path: "runs/*/*.parquet"',
        ])
    lines.extend([
        "---", "",
        "# Skippy inference qualification", "",
        "Nightly performance and correctness results from Skippy’s main branch.",
    ])
    if preview:
        lines.extend([
            "", "**Preview.** Awaiting the first nightly publication. The values, dates, "
            "and commit IDs on this card are illustrative.",
        ])
    if report.get("note"):
        lines.extend(["", plain(report["note"])])
    lines.extend(["", "## Latest results (preview)" if preview else "## Latest results", ""])
    if runs:
        lines.extend(run_sections(runs[0], "###", show_baseline_link=not preview))
    else:
        lines.append("Awaiting the first nightly publication.")

    charts = dataset_charts(report) if charts is None else charts
    if charts:
        if isinstance(charts[0], HistoryChart):
            lines.extend([
                "", "## Performance over time", "",
                f"Up to {HISTORY_DAYS} days ending at the latest run shown. "
                "The dashed line marks the active baseline. Missing measurements leave gaps.",
            ])
        else:
            lines.extend(["", "## Latest performance"])
        for index, chart in enumerate(charts, start=1):
            lines.extend(["", f"![{plain(chart.title)}]({image_path(index)})", "", plain(chart.caption)])
    lines.extend(["", "## Saved baselines (preview)" if preview else "## Active baselines", ""])
    baselines = report["active_baselines"]
    lines.extend(baseline_table(baselines) if baselines else ["Awaiting the first valid measurements."])
    if runs:
        rows = [
            [run_label(run), f'{utc(run["started_at"]):%Y-%m-%d %H:%M}', plain(run["profile"]),
             sha(run["source_sha"]), OUTCOMES[run["outcome"]], coverage(run)]
            for run in runs[:10]
        ]
        lines.extend([
            "", "## Recent runs", "",
            *table(["Run", "Started (UTC)", "Profile", "Tested SHA", "Outcome", "Coverage"], rows),
        ])
        history = runs[:5]
        measurements = metric_history(history)
        if measurements:
            lines.extend([
                "", "## Recent measurements", "", "Run times are in UTC.", "",
                *table(["Workload", "Configuration", "Metric", *(
                    link(f'{utc(run["started_at"]):%b %d %H:%M}', run.get("url")) for run in history
                )], measurements),
            ])
    lines.extend([
        "", "## Reading the results", "",
        "Smoke prefill and decode rates and AgentX output throughput measure different workloads. "
        "Each comparison uses its own configuration and workload.",
        "",
        "Changes are signed differences from the reference. Higher throughput and lower latency "
        "are favorable. Request error rate changes are shown in percentage points.",
        "",
        "Qualification compares each measurement with the saved baseline using the configured "
        "tolerance. The baseline advances when the result meets the improvement threshold.",
        "",
    ])
    links = []
    if runs and not preview:
        links.append(f"[Run history and measurements]({HF_DATASET}/tree/main/runs)")
    if baselines and not preview:
        links.append(f"[Baseline manifest]({HF_DATASET}/blob/main/baselines.json)")
    if links:
        lines.extend([" · ".join(links), ""])
    return "\n".join(lines)
