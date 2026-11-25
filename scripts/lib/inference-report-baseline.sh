#!/usr/bin/env bash
# Protected reporter helpers. Downloaded evidence and candidate trees are data only.
: "${repository:?Reporter repository required}" "${temporary:?Reporter workspace required}"

validate_rates() {
  jq -e --arg sha "$3" --arg row "$4" --slurpfile job "$2" '
    def uint: type == "number" and . >= 0 and . == floor and . < 18446744073709551616;
    (keys == (["source_sha", "configuration", "job_id", "run_id", "requests",
      "prefill_tokens", "prefill_ms", "decode_tokens", "decode_ms"] | sort)) and
    .source_sha == $sha and .configuration == $row and .requests == 3 and
    (.prefill_tokens | uint) and (.decode_tokens | uint) and
    (.prefill_ms | type == "number") and (.decode_ms | type == "number") and
    $job[0].repo_rev == $sha and $job[0].status == "succeeded" and
    (.job_id | type == "string" and test("^job_[A-Za-z0-9_]+$")) and
    .job_id == $job[0].id and .run_id == $job[0].runs[-1].id and
    $job[0].runs[-1].status == "succeeded" and
    (.run_id | type == "string" and test("^run_[A-Za-z0-9_]+$"))
  ' "$1" > /dev/null
}

prepare_baseline() {
  baseline_ready=false
  baseline_note='Main comparison unavailable: no readable successful main run.'
  # Filter again locally: a PR can name its branch main, but is not a main run.
  if ! gh api "$repository/actions/workflows/inference.yml/runs?branch=main&status=success&per_page=100" \
      --paginate --slurp > "$temporary/main-runs.json"; then return; fi
  if ! jq -e '
    [.[] | .workflow_runs[] | select(.head_branch == "main" and .conclusion == "success") |
      select(.event == "schedule" or .event == "workflow_dispatch" or .event == "push") ] |
    sort_by(.created_at, .id) | last |
    select(.id > 0 and .id == (.id | floor) and
      .run_attempt > 0 and .run_attempt == (.run_attempt | floor) and
      (.head_sha | type == "string" and test("^[a-fA-F0-9]{40}$")))
  ' "$temporary/main-runs.json" > "$temporary/main-run.json"; then return; fi
  baseline_run=$(jq -r .id "$temporary/main-run.json")
  baseline_attempt=$(jq -r .run_attempt "$temporary/main-run.json")
  baseline_sha=$(jq -r .head_sha "$temporary/main-run.json")
  baseline_url="https://github.com/Mesh-LLM/skippy/actions/runs/$baseline_run/attempts/$baseline_attempt"
  baseline_note="Latest successful main: [run $baseline_run, attempt $baseline_attempt]($baseline_url), commit \`$baseline_sha\`."
  # Compare workload inputs by blob identity without fetching/executing PR code.
  # Engine sources are intentionally excluded: their performance is being measured.
  local inputs
  inputs='["scripts/inference-execute.py", "scripts/lib/inference_checks.py",
    "scripts/lib/inference_throughput.py", "scripts/lib/inference_inputs.py",
    "scripts/lib/inference_ladder.py", "scripts/lib/ci_artifacts.py",
    "scripts/lib/ci_process.py", "scripts/lib/ci_smoke.py",
    "ci/inference-quality.json", "ci/inference-artifacts.json", ".gi/templates.yaml"]'
  if ! gh api "$repository/git/trees/$SOURCE_SHA?recursive=1" > "$temporary/current-tree.json" ||
     ! gh api "$repository/git/trees/$baseline_sha?recursive=1" > "$temporary/main-tree.json" ||
     ! jq -e --argjson inputs "$inputs" --slurpfile main "$temporary/main-tree.json" '
       def contract:
         if .truncated == false then
           [.tree[] | select(.type == "blob") | select(.path as $p | $inputs | index($p)) |
             {path, sha}] | sort_by(.path)
         else [] end;
       contract as $current | ($main[0] | contract) as $previous |
       ($current | length) == ($inputs | length) and $current == $previous
     ' "$temporary/current-tree.json" > /dev/null; then
    baseline_note+=' Comparison unavailable: workload inputs changed or could not be verified.'
    return
  fi
  if ! gh api "$repository/actions/runs/$baseline_run/attempts/$baseline_attempt/jobs?per_page=100" \
      --paginate --slurp | jq -e '[.[] | .jobs[]]' > "$temporary/main-jobs.json" ||
     ! gh run download "$baseline_run" --repo Mesh-LLM/skippy \
       --pattern "inference-*-$baseline_run-$baseline_attempt" --dir "$temporary/baseline"; then
    baseline_note+=' Comparison unavailable: saved evidence is missing, expired or unreadable.'
    return
  fi
  baseline_ready=true
}

baseline_for_row() {
  [[ $baseline_ready == true ]] || return 1
  local id=$1 candidate found='' name
  # Nightly adds smoke-$id artifacts; AgentX artifacts must never supply PR rates.
  for name in "$id" "smoke-$id"; do
    candidate="$temporary/baseline/inference-$name-$baseline_run-$baseline_attempt"
    [[ -f $candidate/$id-throughput.json ]] || continue
    [[ -z $found ]] || return 1
    jq -e --arg name "qualification ($name)" '
      any(.[]; .name == $name and .conclusion == "success")
    ' "$temporary/main-jobs.json" > /dev/null || return 1
    jq -e --arg sha "$baseline_sha" --arg row "$id" '
      .schema_version == 1 and .source_sha == $sha and .profile == "pr" and
      .hardware == "nvidia-gb10-dgx-spark" and
      (.rows | length) == 1 and .rows[0].id == $row and
      (if ($row | endswith("-split")) then .rows[0].mode == "split" and .rows[0].machines == 2
       else .rows[0].mode == "standalone" and .rows[0].machines == 1 end)
    ' "$candidate/plan.json" > /dev/null || return 1
    validate_rates "$candidate/$id-throughput.json" "$candidate/$id-job.json" "$baseline_sha" "$id" || return 1
    found=$candidate
  done
  [[ -n $found ]] || return 1
  printf '%s' "$found"
}

delta() {
  local current=$1 saved=$2 phase=$3 current_job=$4 saved_job=$5 value main_rate
  # Match the reported serving host class. Physical nodes/conditions can differ.
  if ! jq -e --slurpfile main "$saved_job" '
    def hardware: .machine.host | {arch, chip, cpu_cores, gpu, gpu_count};
    hardware as $current | ($main[0] | hardware) as $previous |
    $current == $previous and $current.arch == "aarch64" and
    $current.gpu == "NVIDIA GB10" and $current.gpu_count == 1 and
    ($current.chip | type == "string" and length > 0) and $current.cpu_cores > 0
  ' "$current_job" > /dev/null; then printf Unavailable; return; fi
  value=$(jq -r --arg phase "$phase" --slurpfile main "$saved" '
    .[$phase + "_tokens"] as $tokens | .[$phase + "_ms"] as $ms |
    $main[0][$phase + "_tokens"] as $previous_tokens |
    $main[0][$phase + "_ms"] as $previous_ms |
    if $tokens == $previous_tokens and $tokens > 0 and $ms > 0 and $previous_ms > 0 and
       ($ms | isfinite) and ($previous_ms | isfinite) then
      (($previous_ms / $ms - 1) * 100) | if isfinite then . else "Unavailable" end
    else "Unavailable" end
  ' "$current")
  if [[ $value == Unavailable ]]; then printf Unavailable; return; fi
  main_rate=$(rate "$saved" "$phase")
  if [[ $main_rate == Unavailable ]]; then printf Unavailable; return; fi
  printf '%+.1f%% (main %s)' "$value" "$main_rate"
}
