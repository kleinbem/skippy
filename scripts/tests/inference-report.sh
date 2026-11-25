#!/usr/bin/env bash
# Offline contract tests: no GitHub calls or native hardware required.
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT
export FIXTURE=$fixture
export SOURCE_SHA=1111111111111111111111111111111111111111
export GITHUB_RUN_ID=42 GITHUB_RUN_ATTEMPT=2 GITHUB_EVENT_NAME=pull_request_target
export PLAN=success REQUIRED=true RESULT=success
export GITHUB_EVENT_PATH="$fixture/event.json" GITHUB_STEP_SUMMARY="$fixture/summary.md"
mkdir -p "$fixture/bin" "$fixture/evidence"
export PATH="$fixture/bin:$PATH"
cat > "$fixture/bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ $1 == run && $2 == download ]]; then
  [[ ! -f $FIXTURE/download-fails ]] || exit 1
  [[ $3 == 40 && $4 == --repo && $5 == Mesh-LLM/skippy ]] || exit 1
  [[ $6 == --pattern && $7 == inference-*-40-3 && $8 == --dir ]] || exit 1
  mkdir -p "$9"
  cp -R "$FIXTURE/baseline/." "$9/"
  exit
fi
[[ $1 == api ]]
if [[ ${3:-} == --method ]]; then
  printf '%s %s\n' "$2" "$4" > "$FIXTURE/mutation"
  cp "$6" "$FIXTURE/posted.json"
else
  case $2 in
    */workflows/inference.yml/runs\?*) cat "$FIXTURE/main-runs.json" ;;
    */git/trees/1111111111111111111111111111111111111111\?*) cat "$FIXTURE/current-tree.json" ;;
    */git/trees/2222222222222222222222222222222222222222\?*) cat "$FIXTURE/main-tree.json" ;;
    */runs/40/attempts/3/jobs\?*) cat "$FIXTURE/main-jobs.json" ;;
    */runs/42/attempts/2/jobs\?*) cat "$FIXTURE/jobs.json" ;;
    */comments\?*) cat "$FIXTURE/comments.json" ;;
    */pulls/52) cat "$FIXTURE/pr.json" ;;
    *) exit 1 ;;
  esac
fi
EOF
chmod +x "$fixture/bin/gh"
printf '{"number":52}\n' > "$GITHUB_EVENT_PATH"
jq -n --arg sha "$SOURCE_SHA" '{state:"open",head:{sha:$sha}}' > "$fixture/pr.json"
printf '[[]]\n' > "$fixture/comments.json"
jq '[.models[] | select(.pr) | (.id + "-standalone"), (select(.split) | .id + "-split")]' \
  "$root/ci/inference-ladder.json" > "$fixture/rows.json"
jq 'to_entries | map({id:(.key+1), name:("qualification ("+.value+")"), conclusion:"success"}) |
  [{jobs: .[:3]}, {jobs: .[3:]}]' \
  "$fixture/rows.json" > "$fixture/jobs.json"
row=$(jq -r '.[0]' "$fixture/rows.json")
receipt="$fixture/evidence/$row-job.json"
rates="$fixture/evidence/$row-throughput.json"
jq -n --arg sha "$SOURCE_SHA" '{repo_rev:$sha,id:"job_test",status:"succeeded",runs:[{id:"run_test",status:"succeeded"}]}' > "$receipt"
jq -n --arg sha "$SOURCE_SHA" --arg row "$row" '{source_sha:$sha,configuration:$row,job_id:"job_test",run_id:"run_test",requests:3,prefill_tokens:300,prefill_ms:600,decode_tokens:30,decode_ms:300}' > "$rates"
cp "$rates" "$fixture/good-rates.json"
cp "$receipt" "$fixture/good-receipt.json"

report() {
  rm -f "$fixture/posted.json" "$fixture/mutation" "$GITHUB_STEP_SUMMARY"
  bash "$root/scripts/inference-report.sh" "$fixture/evidence" > "$fixture/stdout" 2> "$fixture/stderr"
}
reject() {
  if report; then printf 'Expected rejection: %s\n' "$*" >&2; exit 1; fi
  [[ ! -f $fixture/posted.json ]]
}

report
jq -e '.body | contains("Lab Qualification: Passed") and contains("| 500.0 | Unavailable | 100.0 | Unavailable |") and contains("/jobs/job_test") and contains("/attempts/2")' "$fixture/posted.json" > /dev/null
grep -q 'POST$' "$fixture/mutation"
cmp "$GITHUB_STEP_SUMMARY" <(jq -rj .body "$fixture/posted.json")

# Paginated comments: ignore forged markers and preserve newer generations.
jq -n '[[],[
  {id:7,user:{login:"github-actions[bot]",type:"Bot"},body:"<!-- skippy-inference-results:v1 -->\n<!-- run:42:1 -->\nold"},
  {id:8,user:{login:"person",type:"User"},body:"<!-- skippy-inference-results:v1 -->\n<!-- run:99:9 -->\nforged"}
]]' > "$fixture/comments.json"
report
grep -q '/issues/comments/7 PATCH$' "$fixture/mutation"
for generation in '43:1' '42:3'; do
  jq --arg generation "$generation" '.[1][0].body = ("<!-- skippy-inference-results:v1 -->\n<!-- run:" + $generation + " -->")' "$fixture/comments.json" > "$fixture/newer.json"
  cp "$fixture/comments.json" "$fixture/old.json"
  cp "$fixture/newer.json" "$fixture/comments.json"
  report
  [[ ! -f $fixture/posted.json ]]
  mv "$fixture/old.json" "$fixture/comments.json"
done
for changed in '.state="closed"' '.head.sha="2222222222222222222222222222222222222222"'; do
  cp "$fixture/pr.json" "$fixture/open.json"
  jq "$changed" "$fixture/open.json" > "$fixture/pr.json"
  report
  [[ ! -f $fixture/posted.json ]]
  mv "$fixture/open.json" "$fixture/pr.json"
done

for field in source_sha configuration job_id run_id requests; do
  jq --arg field "$field" '.[$field]="wrong"' "$fixture/good-rates.json" > "$rates"
  reject "$field mismatch"
done
cp "$fixture/good-rates.json" "$rates"
for changed in '.repo_rev="wrong"' '.id="job_bad)[url]"' '.status="failed"' '.runs[-1].id="run_wrong"' '.runs[-1].status="failed"'; do
  jq "$changed" "$fixture/good-receipt.json" > "$receipt"
  reject "$changed"
done
cp "$fixture/good-receipt.json" "$receipt"
for value in 0 -1; do
  jq --argjson value "$value" '.prefill_ms=$value' "$fixture/good-rates.json" > "$rates"
  report
  jq -e '.body | contains("| Unavailable | Unavailable | 100.0 | Unavailable |")' "$fixture/posted.json" > /dev/null
done
cp "$fixture/good-rates.json" "$rates"
for state in failure cancelled skipped timed_out startup_failure ''; do
  jq --arg state "$state" '.[0].jobs[0].conclusion=$state' "$fixture/jobs.json" > "$fixture/failed.json"
  cp "$fixture/jobs.json" "$fixture/success.json"
  mv "$fixture/failed.json" "$fixture/jobs.json"
  report
  jq -e '.body | contains("Lab Qualification: Failed or incomplete") and (contains("| 500.0 | Unavailable | 100.0 | Unavailable |") | not)' "$fixture/posted.json" > /dev/null
  mv "$fixture/success.json" "$fixture/jobs.json"
done
printf '[{"jobs":[]}]\n' > "$fixture/jobs.json"
report
jq -e '.body | contains("Lab Qualification: Failed or incomplete") and contains("Missing or incomplete")' "$fixture/posted.json" > /dev/null
REQUIRED=false reject 'irrelevant change'
GITHUB_EVENT_NAME=pull_request reject 'unprotected event'
SOURCE_SHA=invalid reject 'mutable source'
# Latest successful main: ignore newer failed runs and PR branches called main.
cp "$fixture/good-receipt.json" "$receipt"
cp "$fixture/good-rates.json" "$rates"
jq 'to_entries | map({id:(.key+1), name:("qualification ("+.value+")"), conclusion:"success"}) |
  [{jobs: .}]' "$fixture/rows.json" > "$fixture/jobs.json"
export BASELINE_SHA=2222222222222222222222222222222222222222
jq -n --arg sha "$BASELINE_SHA" '[{workflow_runs:[
  {id:40,run_attempt:3,head_branch:"main",head_sha:$sha,conclusion:"success",event:"workflow_dispatch",created_at:"2026-11-21"},
  {id:30,run_attempt:1,head_branch:"main",head_sha:$sha,conclusion:"success",event:"schedule",created_at:"2026-11-20"},
  {id:50,run_attempt:1,head_branch:"main",head_sha:$sha,conclusion:"failure",event:"schedule",created_at:"2026-11-22"},
  {id:60,run_attempt:1,head_branch:"main",head_sha:$sha,conclusion:"success",event:"pull_request_target",created_at:"2026-11-22"}
]}]' > "$fixture/main-runs.json"
cp "$fixture/jobs.json" "$fixture/main-jobs.json"
jq -n '{truncated:false,tree:(["scripts/inference-execute.py", "scripts/lib/inference_checks.py",
  "scripts/lib/inference_throughput.py", "scripts/lib/inference_inputs.py",
  "scripts/lib/inference_ladder.py", "scripts/lib/ci_artifacts.py", "scripts/lib/ci_process.py",
  "scripts/lib/ci_smoke.py", "ci/inference-quality.json", "ci/inference-artifacts.json",
  ".gi/templates.yaml"] | map({path:.,type:"blob",sha:"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}))}' > "$fixture/current-tree.json"
cp "$fixture/current-tree.json" "$fixture/main-tree.json"
baseline="$fixture/baseline/inference-$row-40-3"
mkdir -p "$baseline"
jq -n --arg sha "$BASELINE_SHA" --arg row "$row" '{schema_version:1,source_sha:$sha,profile:"pr",hardware:"nvidia-gb10-dgx-spark",rows:[{id:$row,mode:"standalone",machines:1}]}' > "$baseline/plan.json"
jq '.machine.host={arch:"aarch64",chip:"Cortex-X925",cpu_cores:20,gpu:"NVIDIA GB10",gpu_count:1}' "$receipt" > "$fixture/hardware-job.json"
cp "$fixture/hardware-job.json" "$receipt"
jq --arg sha "$BASELINE_SHA" '.repo_rev=$sha' "$receipt" > "$baseline/$row-job.json"
jq --arg sha "$BASELINE_SHA" '.source_sha=$sha | .prefill_ms=1200 | .decode_ms=150' "$rates" > "$baseline/$row-throughput.json"
report
jq -e '.body | contains("| 500.0 | +100.0% (main 250.0) | 100.0 | -50.0% (main 200.0) |") and contains("/runs/40/attempts/3") and contains("commit `2222222222222222222222222222222222222222`")' "$fixture/posted.json" > /dev/null
cp "$baseline/$row-throughput.json" "$fixture/main-rates.json"
cp "$baseline/plan.json" "$fixture/main-plan.json"
cp "$baseline/$row-job.json" "$fixture/main-job.json"

# Same run data emitted by #118: select nightly smoke, never extended AgentX.
nightly="$fixture/baseline/inference-smoke-$row-40-3"
mv "$baseline" "$nightly"
jq --arg row "$row" '.[0].jobs[0].name=("qualification (smoke-"+$row+")")' "$fixture/jobs.json" > "$fixture/main-jobs.json"
report
jq -e '.body | contains("+100.0% (main 250.0)")' "$fixture/posted.json" > /dev/null
mv "$nightly" "$baseline"
cp "$fixture/jobs.json" "$fixture/main-jobs.json"

unavailable() {
  report
  jq -e '.body | contains("| 500.0 | Unavailable | 100.0 | Unavailable |")' "$fixture/posted.json" > /dev/null
}
for changed in '.profile="nightly"' '.source_sha="wrong"' '.rows[0].mode="split"'; do
  jq "$changed" "$fixture/main-plan.json" > "$baseline/plan.json"
  unavailable
done
cp "$fixture/main-plan.json" "$baseline/plan.json"
for changed in '.source_sha="wrong"' '.run_id="run_wrong"' '.prefill_ms=0 | .decode_ms=0' '.prefill_tokens=301 | .decode_tokens=31'; do
  jq "$changed" "$fixture/main-rates.json" > "$baseline/$row-throughput.json"
  unavailable
done
cp "$fixture/main-rates.json" "$baseline/$row-throughput.json"
for changed in '.machine.host.gpu="other"' '.machine.host=null' '.status="failed"'; do
  jq "$changed" "$fixture/main-job.json" > "$baseline/$row-job.json"
  unavailable
done
cp "$fixture/main-job.json" "$baseline/$row-job.json"
jq '.[0].jobs[0].conclusion="failure"' "$fixture/jobs.json" > "$fixture/main-jobs.json"
unavailable
cp "$fixture/jobs.json" "$fixture/main-jobs.json"
# One phase can remain comparable when only the other phase has different work.
jq '.prefill_tokens=301' "$fixture/main-rates.json" > "$baseline/$row-throughput.json"
report
jq -e '.body | contains("| 500.0 | Unavailable | 100.0 | -50.0% (main 200.0) |")' "$fixture/posted.json" > /dev/null
# Use unrounded values: both decode rates display 100.0 but the delta is +0.1%.
jq '.decode_ms=(30000 / 100.04)' "$fixture/good-rates.json" > "$rates"
jq '.prefill_ms=600 | .decode_ms=(30000 / 99.98)' "$fixture/main-rates.json" > "$baseline/$row-throughput.json"
report
jq -e '.body | contains("+0.0% (main 500.0)") and contains("100.0 | +0.1% (main 100.0)")' "$fixture/posted.json" > /dev/null
cp "$fixture/good-rates.json" "$rates"
cp "$fixture/main-rates.json" "$baseline/$row-throughput.json"
for changed in '.truncated=true' '.tree[0].sha="changed"' '.tree=[]'; do
  jq "$changed" "$fixture/current-tree.json" > "$fixture/main-tree.json"
  unavailable
  jq -e '.body | contains("workload inputs changed or could not be verified")' "$fixture/posted.json" > /dev/null
done
cp "$fixture/current-tree.json" "$fixture/main-tree.json"
touch "$fixture/download-fails"
unavailable
jq -e '.body | contains("missing, expired or unreadable")' "$fixture/posted.json" > /dev/null
rm "$fixture/download-fails"
rm "$baseline/$row-throughput.json"
unavailable
printf '%s\n' 'Inference report contracts passed'
