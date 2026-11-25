"""Admit generator reports for initial transformation and idempotence."""

import json

VERDICTS = {"already_transformed", "transformable", "unsupported_shape", "error",
            "supported_auxiliary", "supported_whole_model"}


def load(path, idempotence=False):
    with path.open("rb") as stream:
        raw = stream.read(16 * 1024 * 1024 + 1)
    if len(raw) > 16 * 1024 * 1024:
        raise ValueError("report exceeds 16 MiB")
    report = json.loads(raw)
    builders = report["builders"]
    if not isinstance(builders, list) or not builders:
        raise ValueError("rewriter emitted no builder records")
    refused, inherited, remaining = [], [], []
    for builder in builders:
        verdict = builder["verdict"]
        filename = builder.get("file") or "<unknown>"
        if verdict not in VERDICTS:
            raise ValueError(f"unknown generator verdict: {verdict}")
        if verdict == "already_transformed":
            inherited.append(filename)
        elif verdict == "transformable":
            remaining.append(filename)
        elif verdict in {"unsupported_shape", "error"}:
            refused.append(f'{filename}: {builder.get("unsupported_reason") or verdict}')
    if idempotence:
        if remaining:
            raise ValueError("second rewriter pass still has edits: " + ", ".join(remaining))
    elif inherited or refused:
        label = ("first rewriter pass received transformed model builders"
                 if inherited else "rewriter refused decoder builders")
        raise ValueError(f"{label}: " + ", ".join(inherited or refused))
    counts = {key: value for key, value in report.get("summary", {}).items()
              if key in VERDICTS and value is not None}
    if any(type(value) is not int or not 0 <= value < 2**63 for value in counts.values()):
        raise ValueError("invalid rewriter summary count")
    return counts
