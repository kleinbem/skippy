#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
matrix="$root/ci/targets.json"
profile="${SKIPPY_CI_PROFILE-full}"

result="$(jq -cer --arg profile "$profile" '
  def safe_id: type == "string" and test("^[a-z0-9_-]+$");
  def host_runner($id):
    if $id == "darwin-aarch64" then ["macos-15"]
    elif $id == "linux-x86_64" then ["ubuntu-24.04"]
    elif $id == "linux-aarch64" then ["ubuntu-24.04-arm"]
    elif $id == "windows-x86_64" then ["windows-2025"]
    else null end;
  def valid_host_identity:
    ([.id, .target, .os, .arch] == ["darwin-aarch64", "aarch64-apple-darwin", "macos", "aarch64"])
    or ([.id, .target, .os, .arch] == ["linux-x86_64", "x86_64-unknown-linux-gnu", "linux", "x86_64"])
    or ([.id, .target, .os, .arch] == ["linux-aarch64", "aarch64-unknown-linux-gnu", "linux", "aarch64"])
    or ([.id, .target, .os, .arch] == ["windows-x86_64", "x86_64-pc-windows-msvc", "windows", "x86_64"]);
  def runtime_variant($r; $h):
    if $r.backend == "cpu" and $r.toolkit == "" and $r.gpu_architectures == "" then "cpu"
    elif $r.backend == "metal" and $h.os == "macos" and $r.toolkit == "" then "metal"
    elif $r.backend == "cuda" and $h.os != "macos" and ($r.toolkit | startswith("12.")) and $r.gpu_architectures != "" then "cuda12"
    elif $r.backend == "cuda" and $h.os != "macos" and ($r.toolkit | startswith("13.")) and $r.gpu_architectures != "" then "cuda13"
    else null end;
  def expected_runtime_runner($h; $variant):
    if $variant == "cpu" then $h.runner
    else
      (["self-hosted", "skippy-ephemeral",
        (if $h.os == "macos" then "macOS" elif $h.os == "linux" then "Linux" else "Windows" end),
        (if $h.arch == "aarch64" then "ARM64" else "X64" end)]
       + (if ($variant | startswith("cuda")) then ["runtime-image"] else [] end)
       + [$variant])
    end;
  def has_only_keys($allowed): (keys - $allowed) == [];
  def enabled: .enabled != false;

  . as $m
  | if ($profile != "pr" and $profile != "full") then error("CI profile must be pr or full") else . end
  | if (type != "object" or (has_only_keys(["schema_version", "enabled_hosts", "hosts", "runtimes"]) | not))
    then error("invalid target matrix fields") else . end
  | if (.hosts | type) != "array" or (.runtimes | type) != "array"
    then error("hosts and runtimes must be arrays") else . end
  | if any(.hosts[], .runtimes[]; type != "object")
    then error("hosts and runtimes must contain objects") else . end
  | if .schema_version != 1 or (.hosts | length) != 4 or (.runtimes | length) != 11
    then error("expected schema 1, four hosts, and eleven runtimes") else . end
  | if (([.hosts[].id, .runtimes[].id] | length) != ([.hosts[].id, .runtimes[].id] | unique | length))
    then error("host and runtime IDs must be unique") else . end
  | if (has("enabled_hosts") and .enabled_hosts != null and
        ((.enabled_hosts | type) != "array" or (.enabled_hosts | length) == 0 or
         (.enabled_hosts | length) != (.enabled_hosts | unique | length) or
         any(.enabled_hosts[]; . as $id | ([ $m.hosts[].id ] | index($id)) == null)))
    then error("enabled_hosts must select unique, defined hosts") else . end
  | if any(.hosts[];
      (has_only_keys(["id", "display_name", "target", "os", "arch", "runner", "enabled"]) | not)
      or (has("id") | not) or (has("display_name") | not) or (has("target") | not)
      or (has("os") | not) or (has("arch") | not) or (has("runner") | not)
      or (.id | safe_id | not) or (.display_name | type != "string" or (gsub("^[[:space:]]+|[[:space:]]+$"; "") | length) == 0)
      or (.target | type != "string") or (.os | type != "string") or (.arch | type != "string")
      or (.runner | type != "array" or any(.[]; type != "string"))
      or (has("enabled") and (.enabled | type != "boolean"))
      or (valid_host_identity | not) or .runner != host_runner(.id))
    then error("invalid host identity or runner boundary") else . end
  | if any(.runtimes[];
      (has_only_keys(["id", "display_name", "host", "backend", "toolkit", "gpu_architectures", "runner", "runner_group", "enabled"]) | not)
      or (has("id") | not) or (has("display_name") | not) or (has("host") | not)
      or (has("backend") | not) or (has("toolkit") | not) or (has("gpu_architectures") | not)
      or (has("runner") | not)
      or (.id | safe_id | not) or (.display_name | type != "string" or (gsub("^[[:space:]]+|[[:space:]]+$"; "") | length) == 0)
      or (.host as $host | ($host | type != "string") or ([ $m.hosts[].id ] | index($host)) == null)
      or (.backend | type != "string") or (.toolkit | type != "string") or (.gpu_architectures | type != "string")
      or (.runner | type != "array" or any(.[]; type != "string"))
      or (has("enabled") and (.enabled | type != "boolean"))
      or (has("runner_group") and .runner_group != null and (.runner_group | type != "string"))
      or (has("runner_group") and .runner_group != null and
          ((.runner[0] != "self-hosted") or (.runner_group | test("^jy-rnr_[a-z0-9]+$") | not)))
      or (. as $runtime | ([ $m.hosts[] | select(.id == $runtime.host) ] | length) != 1))
    then error("invalid runtime identity or runner boundary") else . end
  | if any(.runtimes[];
      . as $r | ($m.hosts[] | select(.id == $r.host)) as $h
      | ($r.enabled != false and $h.enabled == false)
        or (runtime_variant($r; $h) == null)
        or ($r.runner != expected_runtime_runner($h; runtime_variant($r; $h))))
    then error("unsupported backend/toolkit/OS combination or runtime runner labels") else . end
  | ([.runtimes[] as $r | ($m.hosts[] | select(.id == $r.host)) as $h | "\($r.host)/\(runtime_variant($r; $h))"] as $variants
     | if ($variants | length) != ($variants | unique | length)
       then error("duplicate runtime variant") else . end)
  | if any(.hosts[];
      . as $h | (if $h.os == "macos" then ["cpu", "metal"] else ["cpu", "cuda12", "cuda13"] end) as $required
      | any($required[]; . as $variant | ([$m.runtimes[] as $r | ($m.hosts[] | select(.id == $r.host)) as $rh | select($r.host == $h.id and runtime_variant($r; $rh) == $variant)] | length) == 0))
    then error("missing required host runtime variant") else . end
  | ([.hosts[] | select((if $m.enabled_hosts == null then true else .id as $id | any($m.enabled_hosts[]; . == $id) end) and enabled)
      | .enabled = true]) as $hosts
  | ([$m.runtimes[]
      | . as $runtime
      | select(($m.enabled_hosts == null or any($m.enabled_hosts[]; . == $runtime.host)) and $runtime.enabled != false)
      | ($m.hosts[] | select(.id == $runtime.host)) as $host
      | . + {host_runner: $host.runner, enabled: true}
      | if ($runtime.runner_group // null) != null
        then .runner = {group: $runtime.runner_group, labels: $runtime.runner}
        else del(.runner_group) end]) as $runtimes
  | {profile: $profile, hosts: {include: $hosts}, runtimes: {include: $runtimes}}
' "$matrix")"

hosts="$(jq -c '.hosts' <<<"$result")"
runtimes="$(jq -c '.runtimes' <<<"$result")"
profile="$(jq -r '.profile' <<<"$result")"

printf '%s\n' "$result"
if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  printf 'profile=%s\nhosts=%s\nruntimes=%s\n' "$profile" "$hosts" "$runtimes" >> "$GITHUB_OUTPUT"
fi
