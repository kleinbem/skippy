"""Export the dataset card's history charts as static PNGs."""

from datetime import timedelta, timezone
from pathlib import Path
from tempfile import TemporaryDirectory

from .inference_charts import BAR_STYLES, HistoryChart, image_path


INK = "#172939"
MUTED = "#526574"
GRID = "#DCE5EA"
MEASURED = BAR_STYLES[None][0]
BASELINE = BAR_STYLES["baseline"][0]
HISTORY_MARKERS = {
    "passed": ("o", "Equivalent"),
    "improved": ("^", "Improvement"),
    "regressed": ("D", "Regression"),
    "inconclusive": ("s", "Inconclusive"),
    None: ("o", "No verdict"),
}


def history_axes(ax, chart, mdates):
    scale = 0.001 if chart.unit == "ms" else 1
    values = [None if value is None else value * scale for value in chart.values]
    baselines = [None if value is None else value * scale for value in chart.baseline]
    dates = chart.dates
    ax.plot(dates, values, color=MEASURED, linewidth=2, zorder=3)
    pad = max((dates[-1] - dates[0]) / 20, timedelta(minutes=1))
    if any(value is not None for value in baselines):
        end = dates[-1] + pad
        ax.step([*dates, end], [*baselines, baselines[-1]], where="post", color=BASELINE,
                linewidth=2, linestyle="--", label="Baseline in force", zorder=2)
    for verdict, (marker, label) in HISTORY_MARKERS.items():
        points = [
            (date, value) for date, value, outcome in zip(dates, values, chart.verdicts)
            if outcome == verdict and value is not None
        ]
        if points:
            point_dates, point_values = zip(*points)
            ax.scatter(point_dates, point_values, color=BAR_STYLES[verdict][0], edgecolor="white",
                       linewidth=1, s=58, marker=marker, zorder=4, label=label)
    ax.set_xlim(dates[0] - pad, dates[-1] + pad)
    if len(dates) <= 6 and len({date.date() for date in dates}) == len(dates):
        ax.set_xticks(dates)
        ax.xaxis.set_major_formatter(mdates.DateFormatter("%b %d", tz=timezone.utc))
    else:
        locator = mdates.AutoDateLocator(minticks=3, maxticks=6, tz=timezone.utc)
        ax.xaxis.set_major_locator(locator)
        ax.xaxis.set_major_formatter(mdates.ConciseDateFormatter(locator, tz=timezone.utc))
    ax.set_xlabel("Run date (UTC)", color=MUTED, labelpad=8)
    ax.set_ylabel("Seconds" if scale != 1 else "Tokens per second", color=MUTED, labelpad=9)
    ax.margins(y=0.2)


def snapshot_axes(ax, chart):
    positions = range(len(chart.labels))
    colors = [BAR_STYLES[verdict][0] for verdict in chart.verdicts]
    labels = [f"{value:,.2f}".rstrip("0").rstrip(".") for value in chart.values]
    peak = max((*chart.values, *chart.baseline))
    if chart.horizontal:
        bars = ax.barh(positions, chart.values, color=colors)
        ax.set_xlim(0, peak * 1.20 if peak else 1)
        ax.set_yticks(positions, chart.labels)
        ax.invert_yaxis()
        ax.set_xlabel(chart.unit, color=MUTED)
        if chart.baseline:
            ax.plot(chart.baseline, positions, color=BASELINE, label="Saved baseline")
        ax.grid(axis="x", color=GRID, linewidth=0.75)
    else:
        bars = ax.bar(positions, chart.values, color=colors, width=0.58)
        ax.set_ylim(0, peak * 1.20 if peak else 1)
        ax.set_xticks(positions, chart.labels)
        ax.set_ylabel(chart.unit, color=MUTED)
    ax.bar_label(bars, labels=labels, padding=5, fontsize=10, color=INK)


def draw(chart, output, figure_class, mdates):
    history = isinstance(chart, HistoryChart)
    horizontal = not history and chart.horizontal
    height = max(3.8, 2.5 + len(chart.labels) / 4) if horizontal else 3.8
    figure = figure_class(figsize=(8.5, height), dpi=170, facecolor="white")
    left = 0.32 if horizontal else 0.12
    figure.subplots_adjust(left=left, right=0.96, bottom=0.19, top=0.68)
    ax = figure.add_subplot()
    figure.text(0.07, 0.93, chart.title, va="top", fontsize=19, fontweight="bold", color=INK)
    figure.text(0.07, 0.83, chart.caption, va="top", fontsize=11, color=MUTED)

    ax.set_axisbelow(True)
    ax.tick_params(colors=MUTED, labelsize=10, length=0, pad=5)
    for spine in ax.spines.values():
        spine.set_visible(False)
    if history:
        history_axes(ax, chart, mdates)
    else:
        snapshot_axes(ax, chart)
    if history or not chart.horizontal:
        ax.grid(axis="y", color=GRID, linewidth=0.75)
    handles, labels = ax.get_legend_handles_labels()
    if handles:
        ax.legend(handles, labels, frameon=False, ncol=len(labels), fontsize=9, labelcolor=INK,
                  loc="lower left", bbox_to_anchor=(0, 1.03), borderaxespad=0, columnspacing=1.4)
    figure.savefig(output)


def export_charts(charts, directory):
    if not charts:
        return
    try:
        from matplotlib import dates as mdates
        from matplotlib.backends.backend_agg import FigureCanvasAgg
        from matplotlib.figure import Figure
    except ModuleNotFoundError as error:
        raise ValueError("dataset PNG export requires ci/inference-report-requirements.txt") from error

    def figure_class(**options):
        figure = Figure(**options)
        FigureCanvasAgg(figure)
        return figure

    with TemporaryDirectory(dir=directory) as staging:
        outputs = []
        for index, chart in enumerate(charts, start=1):
            destination = directory / image_path(index)
            rendered = Path(staging) / destination.name
            draw(chart, rendered, figure_class, mdates)
            outputs.append((rendered, destination))
        (directory / "charts").mkdir(exist_ok=True)
        for rendered, destination in outputs:
            rendered.replace(destination)
