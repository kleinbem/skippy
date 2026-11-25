"""Build per-run comparisons and time series for inference qualification."""

from dataclasses import dataclass
from datetime import timedelta
from decimal import Decimal, ROUND_CEILING
from math import isfinite
import re

from .inference_labels import configuration_label, workload_label


CHART_METRICS = ("output_tok_s", "ttft_p95_ms", "prefill_tok_s", "decode_tok_s")
HISTORY_DAYS = 30
BAR_STYLES = {
    "baseline": ("#68778A", "Gray = saved baseline"),
    "passed": ("#216FAD", "Blue = equivalent (within tolerance)"),
    "improved": ("#207956", "Green = improvement"),
    "regressed": ("#B13A4D", "Red = regression"),
    "inconclusive": ("#906318", "Amber = inconclusive"),
    None: ("#537489", "Slate = measurement without a verdict"),
}


@dataclass(frozen=True)
class Chart:
    title: str
    unit: str
    labels: tuple
    values: tuple
    caption: str
    verdicts: tuple
    baseline: tuple = ()
    horizontal: bool = False

    def mermaid(self):
        height = max(260, 125 + 36 * len(self.labels)) if self.horizontal else 290
        layout = " horizontal" if self.horizontal else ""
        labels = ", ".join(
            quoted(f"{label} — {value_label(value)}")
            for label, value in zip(self.labels, self.values)
        )
        outcomes = tuple(dict.fromkeys(self.verdicts))
        palette_keys = (*outcomes, "baseline") if self.baseline else outcomes
        palette = ", ".join(BAR_STYLES[outcome][0] for outcome in palette_keys)
        peak = Decimal(str(max((*self.values, *self.baseline))))
        upper = (peak * Decimal("1.12")).quantize(
            Decimal(1).scaleb(peak.adjusted() - 1), rounding=ROUND_CEILING,
        ) if peak else Decimal(1)
        # Numeric tick labels add axis padding in Mermaid, which gives the zero
        # entries in the overlaid series a visible strip. Each category already
        # carries the measured value.
        axis = ["    yAxis:", "      showLabel: false", "      showTick: false"] if len(outcomes) > 1 else []
        lines = [
            "---", "config:", "  xyChart:", "    width: 760", f"    height: {height}",
            *axis,
            "  themeVariables:", "    xyChart:", f'      plotColorPalette: "{palette}"',
            "---", f"xychart-beta{layout}", f"    title {quoted(self.title)}",
            f"    x-axis [{labels}]", f"    y-axis {quoted(self.unit)} 0 --> {upper:f}",
        ]
        for outcome in outcomes:
            series = (
                value_label(value if verdict == outcome else 0)
                for value, verdict in zip(self.values, self.verdicts)
            )
            lines.append(f"    bar [{', '.join(series)}]")
        if self.baseline:
            references = ", ".join(value_label(value) for value in self.baseline)
            lines.append(f"    line [{references}]")
        return "\n".join(lines) + "\n"


@dataclass(frozen=True)
class HistoryChart:
    title: str
    unit: str
    dates: tuple
    values: tuple
    baseline: tuple
    verdicts: tuple
    caption: str


def quoted(value):
    # Mermaid front matter and chart syntax can only come from this template.
    label = re.sub(r"[^a-zA-Z0-9 ./_()—-]", " ", value)
    return '"' + " ".join(label.split()) + '"'


def value_label(value):
    if type(value) not in (int, float) or not isfinite(value) or value < 0:
        raise ValueError(f"invalid chart measurement: {value!r}")
    return f"{value:.2f}".rstrip("0").rstrip(".")


def bar_legend(verdicts):
    return " · ".join(label for verdict, (_, label) in BAR_STYLES.items() if verdict in verdicts)


def observations(run):
    for config in run["configurations"]:
        for row in config["metrics"]:
            if row["metric"] in CHART_METRICS:
                yield (row["workload"], config["id"], row["metric"]), row


def run_charts(run, metrics):
    groups = {}
    for (workload, config, metric), row in observations(run):
        if row["measured"] is not None:
            groups.setdefault((metric, workload), []).append((config, row))
    charts = []
    for metric in CHART_METRICS:
        matches = ((workload, points) for (key, workload), points in groups.items() if key == metric)
        selected = max(matches, key=lambda group: len(group[1]), default=None)
        if selected is None:
            continue
        workload, points = selected
        workload = workload_label(workload)
        title, unit = metrics[metric]
        if len(points) == 1:
            config, row = points[0]
            verdicts = (row.get("verdict"),)
            labels, values = ("This run",), (row["measured"],)
            if row.get("reference"):
                labels = ("Saved baseline", *labels)
                values = (row["reference"]["value"], *values)
                verdicts = ("baseline", *verdicts)
            caption = f"{configuration_label(config)} · {workload}. {bar_legend(verdicts)}."
            charts.append(Chart(title, unit, labels, values, caption, verdicts))
            continue
        labels = tuple(configuration_label(config) for config, _ in points)
        values = tuple(row["measured"] for _, row in points)
        verdicts = tuple(row.get("verdict") for _, row in points)
        baseline = ()
        if all(row.get("reference") for _, row in points):
            baseline = tuple(row["reference"]["value"] for _, row in points)
        line = " The gray line shows the saved baseline." if baseline else ""
        caption = f"{workload}. {bar_legend(verdicts)}.{line}"
        charts.append(Chart(title, unit, labels, values, caption, verdicts, baseline, horizontal=True))
    return charts


def baseline_after(row):
    if row.get("baseline_update") in ("seeded", "promoted"):
        return row["measured"]
    if row.get("reference"):
        return row["reference"]["value"]
    return None


def history_charts(timeline, metrics):
    """Build one timeline per headline metric from recent published runs."""
    cutoff = timeline[-1][0] - timedelta(days=HISTORY_DAYS)
    window = [(date, run) for date, run in timeline if date >= cutoff]
    series = {}
    for index, (_, run) in enumerate(window):
        for key, row in observations(run):
            series.setdefault(key, {})[index] = row
    series = {key: matching_history(points) for key, points in series.items()}

    charts = []
    for metric in CHART_METRICS:
        matches = (
            (sum(row["measured"] is not None for row in points.values()), key, points)
            for key, points in series.items() if key[2] == metric
        )
        selected = max(matches, key=lambda item: (item[0], item[1]), default=None)
        if selected is None or selected[0] < 2:
            continue
        _, (workload, config, _), points = selected
        title, unit = metrics[metric]
        dates, values, baseline, verdicts = history_values(window, points)
        charts.append(HistoryChart(
            title, unit, dates, values, baseline, verdicts,
            f"{configuration_label(config)} · {workload_label(workload)}",
        ))
    return charts


def matching_history(points):
    fingerprints = {index: row["contract_sha256"] for index, row in points.items()
                    if row.get("contract_sha256")}
    if not fingerprints:
        return points
    current = fingerprints[max(fingerprints)]
    boundary = max((index for index, fingerprint in fingerprints.items() if fingerprint != current),
                   default=-1)
    return {index: row for index, row in points.items() if index > boundary}


def history_values(window, points):
    dates, values, baseline, verdicts = [], [], [], []
    active = None
    for index, (date, _) in enumerate(window):
        row = points.get(index)
        measured = None if row is None else row["measured"]
        updated = None if row is None else baseline_after(row)
        if updated is not None:
            active = updated
        dates.append(date)
        values.append(measured)
        baseline.append(active)
        verdicts.append(None if row is None else row.get("verdict"))
    return tuple(dates), tuple(values), tuple(baseline), tuple(verdicts)


def image_path(index):
    return f"charts/qualification-{index}.png"
