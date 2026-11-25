"""Run numerical and endpoint checks on a native inference worker."""

import json
import re
import shutil
import tarfile
from pathlib import Path

from .ci_artifacts import read_json, write_json
from .ci_process import command
from .ci_smoke import curl
from .inference_inputs import downloaded_path, safe_relative

ENDPOINT = "http://127.0.0.1:9337"


def numerical(model, coordinate, fixture, output):
    layers = fixture["layers"]
    for kind in ("chain", "state-handoff"):
        args = [
            "target/debug/skippy-correctness", kind, "--model", str(model), "--model-id",
            coordinate, "--layer-end", str(layers), "--n-gpu-layers", "0", "--ctx-size", "512",
            "--report-out", str(output / f"{kind}.json"),
        ]
        if kind == "chain":
            args.extend(["--splits", f"{layers // 3},{2 * layers // 3}"])
        else:
            args.extend([
                "--activation-width", str(fixture["activation_width"]), "--state-layer-start", "0",
                "--state-layer-end", str(layers // 2), "--prefix-token-count", "32",
            ])
        command(args, timeout=1800, overrides={"CUDA_VISIBLE_DEVICES": ""})


def offload(log):
    with log.open(encoding="utf-8") as stream:
        for line in stream:
            match = re.search(r"offloaded ([0-9]+)/[^\n]*GPU", line)
            if match and int(match[1]) > 0:
                return
    raise ValueError("server log has no positive GPU-layer offload evidence")


def request(model, prompt, maximum, timeout, stream=False):
    payload = {"model": model, "messages": [{"role": "user", "content": prompt}],
               "temperature": 0, "seed": 42, "max_tokens": maximum,
               "chat_template_kwargs": {"enable_thinking": False}}
    if stream:
        payload["stream"] = True
    return curl(["-H", "Content-Type: application/json", "--data-binary", json.dumps(payload),
                 ENDPOINT + "/v1/chat/completions"], timeout)


def score(coordinate, output, profile):
    evidence = []
    timeout, maximum = (60, 64) if profile == "pr" else (300, 256)
    for case in read_json(Path("ci/inference-quality.json")):
        first = json.loads(request(coordinate, case["prompt"], maximum, timeout))
        second = json.loads(request(coordinate, case["prompt"], maximum, timeout))
        text = first["choices"][0]["message"]["content"]
        tokens = first["usage"]["completion_tokens"]
        passed = (text.strip() == case["expected"] and text == second["choices"][0]["message"]["content"]
                  and type(tokens) is int and tokens > 0)
        evidence.append({"case": case, "response": first, "repeat": second, "passed": passed})
        write_json(output / "quality.json", evidence)
        if not passed:
            raise ValueError("scored quality or deterministic repeat check failed")


def streaming(coordinate, output):
    response = request(coordinate, "What is 17 + 25? Reply with only the integer.", 16, 60, True)
    (output / "streaming.sse").write_text(response, encoding="utf-8")
    text, stop, done = [], False, False
    for line in response.splitlines():
        if not line.startswith("data: "):
            continue
        if line == "data: [DONE]":
            done = True
            continue
        chunk = json.loads(line[6:])
        if chunk.get("error") is not None:
            raise ValueError("stream returned an error")
        choices = chunk.get("choices") or []
        if choices:
            content = (choices[0].get("delta") or {}).get("content")
            if isinstance(content, str):
                text.append(content)
            stop |= choices[0].get("finish_reason") == "stop"
    if not (done and stop and "".join(text).strip() == "42"):
        raise ValueError("incomplete or incorrect streaming response")
    write_json(output / "smoke.json", {"status": "passed", "streaming": True, "expected": "42",
                                      "context": 4096, "max_output_tokens": 16, "request_timeout_s": 60})


def verify_corpus(corpus, output, phase):
    result = json.loads(curl(["--retry", "3",
                             f'https://huggingface.co/api/datasets/{corpus["repo"]}/revision/main'], 60, local=False))
    write_json(output / f"corpus-{phase}.json", result)
    if not re.fullmatch(r"[0-9a-f]{40}", corpus["revision"]) or result.get("sha") != corpus["revision"]:
        raise ValueError("AgentX corpus revision differs from the admitted pin")


def retain(directory, output):
    for index, path in enumerate(sorted(directory.rglob("profile_export_aiperf.json"))):
        shutil.copyfile(path, output / f"agentx-profile-{index}.json")
    if directory.is_dir():
        with tarfile.open(output / "agentx.tar.gz", "w:gz") as archive:
            archive.add(directory, arcname="agentx")


def performance(profile, coordinate, context, output, fixture):
    revision = fixture["tokenizer_revision"]
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("unpinned tokenizer revision")
    files = fixture["tokenizer_files"]
    tokenizer = downloaded_path(command([
        "hf", "download", fixture["tokenizer_repo"], "--revision", revision, "--quiet", *files,
    ], timeout=600, capture_stdout=True))
    if any((tokenizer / safe_relative(name)).stat().st_size == 0 for name in files):
        raise ValueError("empty pinned tokenizer asset")
    corpus = read_json(Path("ci/inference-corpus.json"))
    verify_corpus(corpus, output, "before")
    directory = output / "agentx"
    duration = {"pr": "120", "nightly": "900", "release": "1800"}[profile]
    args = [
        "uv", "tool", "run", "--python", "3.12", "--constraints", "ci/aiperf-requirements.lock",
        "--from", "aiperf==0.13.0", "aiperf", "profile", "--ui", "simple", "--scenario",
        "inferencex-agentx-mvp", "--url", ENDPOINT, "--model", coordinate, "--tokenizer",
        str(tokenizer), "--max-context-length", str(context), "--endpoint-type", "chat", "--streaming",
        "--cache-bust", "first-turn-prefix", "--public-dataset", corpus["alias"], "--concurrency", "1",
        "--use-server-token-count", "--random-seed", "42", "--benchmark-duration", duration,
        "--artifact-dir", str(directory),
    ]
    if profile == "pr":
        args += ["--unsafe-override", "--num-dataset-entries", "4"]
    else:
        args += ["--num-profile-runs", "2", "--no-profile-run-disable-warmup-after-first"]
    try:
        command(args, timeout=15000)
    finally:
        try:
            verify_corpus(corpus, output, "after")
        finally:
            retain(directory, output)
    exports = list(directory.rglob("profile_export_aiperf.json"))
    expected = 1 if profile == "pr" else 2
    if len(exports) != expected:
        raise ValueError(f"AgentX requires {expected} complete trial exports")
    for path in exports:
        value = read_json(path)
        count, errors = value["request_count"]["avg"], value["request_error_rate"]["avg"]
        if (type(count) not in {int, float} or not count > 0
                or type(errors) not in {int, float} or errors != 0
                or value.get("error_summary") != [] or value.get("is_complete") is not True):
            raise ValueError("AgentX produced empty, incomplete or failed request evidence")
        if profile != "pr" and value["metadata"].get("submission_valid") is not True:
            raise ValueError("AgentX rejected extended-run validity")
