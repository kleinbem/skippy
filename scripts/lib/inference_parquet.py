"""Store public qualification history as a fixed Parquet table."""


def schema():
    import pyarrow as pa

    return pa.schema([
        ("run_id", pa.string()), ("started_at", pa.string()), ("source_sha", pa.string()),
        ("profile", pa.string()), ("run_outcome", pa.string()), ("run_note", pa.string()),
        ("baseline_revision", pa.string()), ("configuration", pa.string()), ("workload", pa.string()),
        ("lab_outcome", pa.string()), ("metric", pa.string()), ("measured", pa.float64()),
        ("reference", pa.float64()), ("reference_sha", pa.string()), ("contract_sha256", pa.string()),
        ("verdict", pa.string()), ("baseline_update", pa.string()),
    ])


def write_run(path, run):
    import pyarrow as pa
    from pyarrow import parquet

    columns = {
        "run_id": run["id"], "started_at": run["started_at"], "source_sha": run["source_sha"],
        "profile": run["profile"], "run_outcome": run["outcome"], "run_note": run.get("note"),
        "baseline_revision": run.get("baseline_revision"),
    }
    rows = []
    for config in run["configurations"]:
        for metric in config["metrics"]:
            reference = metric.get("reference") or {}
            rows.append({
                **columns, "configuration": config["id"], "lab_outcome": config["outcome"],
                "workload": metric["workload"], "metric": metric["metric"], "measured": metric["measured"],
                "contract_sha256": metric.get("contract_sha256"), "reference": reference.get("value"),
                "reference_sha": reference.get("source_sha"), "verdict": metric.get("verdict"),
                "baseline_update": metric.get("baseline_update"),
            })
    parquet.write_table(pa.Table.from_pylist(rows, schema=schema()), path)


def read_run(path):
    from pyarrow import parquet

    content = parquet.read_table(path)
    if content.schema != schema() or not content.num_rows:
        raise ValueError("invalid inference history schema")
    rows = content.to_pylist()
    first = rows[0]
    run_fields = ("run_id", "started_at", "source_sha", "profile", "run_outcome", "run_note", "baseline_revision")
    configurations = {}
    for row in rows:
        if any(row[key] != first[key] for key in run_fields):
            raise ValueError("inconsistent run metadata in inference history")
        key = (row["configuration"], row["workload"])
        configuration = configurations.setdefault(key, {
            "id": row["configuration"], "workload": row["workload"], "outcome": row["lab_outcome"],
            "metrics": [],
        })
        if row["lab_outcome"] != configuration["outcome"]:
            raise ValueError("inconsistent lab outcome in inference history")
        metric = {key: row[key] for key in ("workload", "metric", "measured")}
        metric.update({key: row[key] for key in ("contract_sha256", "verdict", "baseline_update")
                       if row[key] is not None})
        if row["reference"] is not None:
            metric["reference"] = {"value": row["reference"], "source_sha": row["reference_sha"]}
        configuration["metrics"].append(metric)
    run = {
        "id": first["run_id"], "started_at": first["started_at"], "source_sha": first["source_sha"],
        "profile": first["profile"], "outcome": first["run_outcome"],
        "published": True, "configurations": list(configurations.values()),
    }
    if first["run_note"]:
        run["note"] = first["run_note"]
    if first["baseline_revision"]:
        run["baseline_revision"] = first["baseline_revision"]
    return run
