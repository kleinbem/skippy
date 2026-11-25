#!/usr/bin/env bash
# Runs from protected main; evidence is data, never executable source.
set -euo pipefail
export LC_ALL=C

fail() { printf '%s\n' "$*" >&2; exit 1; }
[[ ${GITHUB_EVENT_NAME:-} == pull_request_target ]] || fail 'Results comments require pull_request_target'
[[ ${SOURCE_SHA:-} =~ ^[a-fA-F0-9]{40}$ ]] || fail 'Invalid source SHA'
[[ ${GITHUB_RUN_ID:-} =~ ^[1-9][0-9]*$ ]] || fail 'Invalid workflow run ID'
[[ ${GITHUB_RUN_ATTEMPT:-} =~ ^[1-9][0-9]*$ ]] || fail 'Invalid workflow attempt'
[[ ${REQUIRED:-} == true ]] || fail 'Results comments require inference qualification'

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
repository=repos/Mesh-LLM/skippy
marker='<!-- skippy-inference-results:v1 -->'
number=$(jq -er '.number | select(type == "number" and . > 0 and . == floor)' "$GITHUB_EVENT_PATH")
temporary=$(mktemp -d)
trap 'rm -rf -- "$temporary"' EXIT
directory=${1:?Usage: inference-report.sh EVIDENCE_DIRECTORY}
run=$GITHUB_RUN_ID
attempt=$GITHUB_RUN_ATTEMPT
url="https://github.com/Mesh-LLM/skippy/actions/runs/$run"

# Pagination is needed even though today's PR ladder fits on one page.
gh api "$repository/actions/runs/$run/attempts/$attempt/jobs?per_page=100" --paginate --slurp |
  jq -e '[.[] | .jobs[]]' > "$temporary/jobs.json"
jq -e '
  if .schema_version != 1 then error("Unsupported ladder") else
    [.models[] | select(.pr) |
      if (.id | test("^[a-z0-9-]+$")) then
        (.id + "-standalone"), (select(.split) | .id + "-split")
      else error("Invalid configuration ID") end]
  end | if length > 0 and length == (unique | length) then . else error("Invalid PR ladder") end
' "$root/ci/inference-ladder.json" > "$temporary/rows.json"

outcome='Failed or incomplete'
description='Inference admission or qualification incomplete or failed'
if [[ ${PLAN:-} == success && ${RESULT:-} == success ]]; then
  description='Native Spark lab qualification'
  if jq -e --slurpfile jobs "$temporary/jobs.json" '
    all(.[]; . as $id | any($jobs[0][];
      .name == ("qualification (" + $id + ")") and .conclusion == "success"))
  ' "$temporary/rows.json" > /dev/null; then
    outcome=Passed
  fi
fi

# shellcheck source=scripts/lib/inference-report-baseline.sh
source "$root/scripts/lib/inference-report-baseline.sh"
prepare_baseline

cat > "$temporary/body.md" <<EOF
$marker
<!-- run:$run:$attempt -->
### Skippy Lab Qualification: $outcome

Commit: \`$SOURCE_SHA\` · PR smoke · [Workflow and timings]($url/attempts/$attempt) · [Saved evidence]($url#artifacts)

$description.

$baseline_note

| Configuration | Outcome | Prefill tok/s | Prefill vs main | Decode tok/s | Decode vs main | Actions | Lab record |
| --- | --- | ---: | ---: | ---: | ---: | --- | --- |
EOF

rate() {
  local value
  value=$(jq -r --arg phase "$2" '
    .[$phase + "_tokens"] as $tokens | .[$phase + "_ms"] as $ms |
    if $tokens > 0 and $ms > 0 and ($ms | isfinite) then
      ($tokens * 1000 / $ms) | if isfinite then . else "Unavailable" end
    else "Unavailable" end
  ' "$1")
  if [[ $value == Unavailable ]]; then printf '%s' "$value"; else printf '%.1f' "$value"; fi
}

jq -r '.[]' "$temporary/rows.json" > "$temporary/rows.txt"
while IFS= read -r row; do
  job=$(jq -c --arg name "qualification ($row)" '[.[] | select(.name == $name)][0] // {}' "$temporary/jobs.json")
  case $(jq -r '.conclusion // ""' <<< "$job") in
    success) state=Passed ;;
    failure|timed_out|action_required|startup_failure) state=Failed ;;
    cancelled) state=Cancelled ;;
    skipped) state='Skipped (not qualified)' ;;
    *) state='Missing or incomplete' ;;
  esac
  action=—
  action_id=$(jq -r '.id // empty' <<< "$job")
  if [[ -n $action_id ]]; then
    [[ $action_id =~ ^[1-9][0-9]*$ ]] || fail 'Invalid Actions job ID'
    action="[Logs]($url/job/$action_id)"
  fi
  receipt="$directory/$row-job.json"
  lab=Unavailable
  if [[ -f $receipt ]]; then
    lab_id=$(jq -er --arg sha "$SOURCE_SHA" '
      if .repo_rev == $sha and (.id | type == "string" and test("^job_[A-Za-z0-9_]+$"))
      then .id else error("Comment evidence source or job ID mismatch") end
    ' "$receipt")
    lab="[$lab_id](https://pilot.jianyang.cloud/jobs/$lab_id)"
  fi
  prefill=Unavailable
  decode=Unavailable
  prefill_delta=Unavailable
  decode_delta=Unavailable
  rates="$directory/$row-throughput.json"
  if [[ $state == Passed && -f $rates ]]; then
    # Invalid current evidence remains a reporting failure, not an unavailable baseline.
    validate_rates "$rates" "$receipt" "$SOURCE_SHA" "$row" || fail 'Comment throughput evidence identity mismatch'
    prefill=$(rate "$rates" prefill)
    decode=$(rate "$rates" decode)
    if baseline=$(baseline_for_row "$row" 2> /dev/null); then
      prefill_delta=$(delta "$rates" "$baseline/$row-throughput.json" prefill "$receipt" "$baseline/$row-job.json")
      decode_delta=$(delta "$rates" "$baseline/$row-throughput.json" decode "$receipt" "$baseline/$row-job.json")
    fi
  fi
  printf "| \`%s\` | %s | %s | %s | %s | %s | %s | %s |\n" "$row" "$state" "$prefill" "$prefill_delta" "$decode" "$decode_delta" "$action" "$lab" >> "$temporary/body.md"
done < "$temporary/rows.txt"

cat >> "$temporary/body.md" <<'EOF'

TPS describes the first response of each of three short scored smoke cases at concurrency 1; deterministic repeats are excluded. Each rate is total tokens / total server-reported phase time, not an average of request rates. Prefill excludes cached prompt tokens. These short samples include startup/warmup effects and are not sustained benchmarks or controlled speedup comparisons. Missing or invalid timings show as Unavailable.

Deltas use unrounded rates from matching PR-profile smoke receipts on the latest successful main run, including the nightly smoke evidence. Positive means faster. Changed workload inputs, different serving host classes, unequal phase token counts or missing evidence show Unavailable. Physical Spark placement and runtime conditions can differ; deltas are descriptive, not controlled speedup claims.

Smoke checks cover GPU startup, deterministic endpoint repeats and bounded streaming. They do not establish an AgentX performance score. [Nightly history and saved performance baselines](https://huggingface.co/datasets/meshllm/skippy-inference-qualification) are published separately; a saved baseline can be older than the latest successful main run used here. The lab retains native results and logs. GitHub evidence copies expire after 30 days.
EOF
if [[ -n ${GITHUB_STEP_SUMMARY:-} ]]; then cat "$temporary/body.md" >> "$GITHUB_STEP_SUMMARY"; fi

gh api "$repository/issues/$number/comments?per_page=100" --paginate --slurp |
  jq -e --arg marker "$marker" '
    [.[][] | select(.user.login == "github-actions[bot]" and .user.type == "Bot") |
      select(.body | startswith($marker))] | last // {}
  ' > "$temporary/comment.json"

# Re-read the PR immediately before posting; stale runs must not overwrite newer results.
gh api "$repository/pulls/$number" > "$temporary/pr.json"
if ! jq -e --arg sha "$SOURCE_SHA" '.state == "open" and .head.sha == $sha' "$temporary/pr.json" > /dev/null ||
   jq -e --argjson run "$run" --argjson attempt "$attempt" '
     (.body // "" | split("\n")[1] // "" |
       capture("^<!-- run:(?<run>[0-9]+):(?<attempt>[0-9]+) -->$")?) as $old |
     ($old.run | tonumber) > $run or
     (($old.run | tonumber) == $run and ($old.attempt | tonumber) > $attempt)
   ' "$temporary/comment.json" > /dev/null; then
  printf '%s\n' 'Skipping results comment for superseded qualification'
  exit 0
fi

method=POST
endpoint="$repository/issues/$number/comments"
comment_id=$(jq -r '.id // empty' "$temporary/comment.json")
if [[ -n $comment_id ]]; then
  [[ $comment_id =~ ^[1-9][0-9]*$ ]] || fail 'Invalid comment ID'
  method=PATCH
  endpoint="$repository/issues/comments/$comment_id"
fi
jq -Rs '{body: .}' "$temporary/body.md" > "$temporary/payload.json"
gh api "$endpoint" --method "$method" --input "$temporary/payload.json" > /dev/null
