#!/usr/bin/env python3
"""Compare deterministic Skippy TTS PCM with pinned llama-tts full-model PCM.

The public speech endpoint uses a random seed, so this numerical oracle runs a
test-only in-process candidate with shared fixed sampling and the same GGUF,
projector, and prompt as llama-tts. The regular HTTP smoke remains a separate
API contract check; waveform parity does not certify intelligibility.
"""

from __future__ import annotations

import argparse
from array import array
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import wave

from lib.ci_process import cancel_on_signal, command
from lib.upgrade_source import prepared_head
from tts_oracle_metrics import MAX_RELATIVE_RMS_ERROR, MIN_WAVEFORM_COSINE, validate_tts_metrics


PROMPT = "The mesh is ready."
SEED = 7
TOP_K = 20
TOP_P = 0.8
# Compare complete utterances; a short frame cap would only compare two
# identically truncated clips and is now rejected by the candidate runtime.
MAX_FRAMES = 512
CONTEXT_SIZE = 2048
TEST_NAME = "frontend::tests::tts_oracle::deterministic_tts_candidate_when_fixture_is_set"


def require_stamp(path, patched_sha, tools):
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 65536:
        raise RuntimeError("missing or invalid pinned TTS build stamp")
    stamp = set(path.read_text(encoding="utf-8").splitlines())
    required = {
        f"patched-sha={patched_sha}", "backend=cpu", "link-mode=static",
        "cmake-arg=-DGGML_NATIVE=OFF", "cmake-arg=-DGGML_METAL=OFF",
    }
    if tools:
        required.add("cmake-arg=-DLLAMA_BUILD_TOOLS=ON")
    if not required.issubset(stamp):
        raise RuntimeError(f"TTS build stamp missing {sorted(required - stamp)}")


def run_logged(argv, log_path, *, root, env=None):
    stderr_path = log_path.with_suffix(".stderr.log")
    diagnostic = log_path.with_suffix(".diagnostic.log")
    try:
        with log_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
            command(argv, cwd=root, stdout=stdout, stderr=stderr, timeout=900, overrides=env)
        if max(log_path.stat().st_size, stderr_path.stat().st_size) > 8 * 1024 * 1024:
            raise ValueError("oracle command exceeded log limit")
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError, KeyboardInterrupt) as error:
        diagnostic.write_text(str(error) + "\n", encoding="utf-8")
        raise
    diagnostic.write_text("success\n", encoding="utf-8")


def read_pcm16_wav(path: Path) -> tuple[int, int, array]:
    """Decode nonempty PCM16 WAV data with explicit format and endian validation."""
    try:
        with wave.open(str(path), "rb") as audio:
            sample_rate = audio.getframerate()
            channels = audio.getnchannels()
            if audio.getcomptype() != "NONE" or audio.getsampwidth() != 2:
                raise RuntimeError(f"{path.name} is not uncompressed PCM16 WAV")
            frames = audio.getnframes()
            if frames * channels * 2 > 64 * 1024 * 1024:
                raise RuntimeError("TTS WAV exceeds the size limit")
            raw = audio.readframes(frames)
            if len(raw) != frames * channels * 2:
                raise RuntimeError("TTS WAV has truncated samples")
    except (OSError, EOFError, wave.Error) as error:
        raise RuntimeError(f"cannot read {path.name} as WAV: {error}") from error
    if sample_rate <= 0 or channels <= 0 or not raw or len(raw) % (2 * channels):
        raise RuntimeError(f"{path.name} has invalid or empty PCM samples")
    samples = array("h")
    samples.frombytes(raw)
    if sys.byteorder != "little":
        samples.byteswap()
    return sample_rate, channels, samples


def compare_wavs(candidate_path: Path, oracle_path: Path) -> dict[str, float | int]:
    """Check exact audio dimensions and bounded, nonsilent waveform disagreement."""
    candidate_rate, candidate_channels, candidate = read_pcm16_wav(candidate_path)
    oracle_rate, oracle_channels, oracle = read_pcm16_wav(oracle_path)
    if candidate_rate != oracle_rate or candidate_channels != oracle_channels:
        raise RuntimeError("TTS sample rate or channel count differs from monolithic oracle")
    if len(candidate) != len(oracle):
        raise RuntimeError(
            "TTS sample count differs from monolithic oracle: "
            f"candidate={len(candidate)}, reference={len(oracle)}"
        )
    candidate_energy = math.fsum(float(value) ** 2 for value in candidate)
    oracle_energy = math.fsum(float(value) ** 2 for value in oracle)
    if candidate_energy <= len(candidate) or oracle_energy <= len(oracle):
        raise RuntimeError("TTS candidate or monolithic oracle is silent")
    delta_energy = math.fsum(
        float(left - right) ** 2 for left, right in zip(candidate, oracle)
    )
    dot = math.fsum(
        float(left) * right for left, right in zip(candidate, oracle)
    )
    relative_rms_error = math.sqrt(delta_energy / oracle_energy)
    waveform_cosine = max(-1.0, min(1.0, dot / math.sqrt(candidate_energy * oracle_energy)))
    metrics: dict[str, float | int] = {
        "sample_rate_hz": candidate_rate,
        "channels": candidate_channels,
        "sample_count": len(candidate) // candidate_channels,
        "relative_rms_error": relative_rms_error,
        "waveform_cosine": waveform_cosine,
    }
    try:
        validate_tts_metrics(metrics)
    except ValueError as error:
        raise RuntimeError(
            "TTS PCM differs from monolithic oracle: "
            f"relative_rms_error={relative_rms_error:.7g}, waveform_cosine={waveform_cosine:.8g}"
        ) from error
    return metrics


def sha256(path: Path) -> str:
    """Stream an artifact digest for identity-bound certification evidence."""
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run_oracle(args: argparse.Namespace) -> dict[str, object]:
    """Generate both deterministic utterances and bind measured PCM parity to inputs."""
    oracle_cli = Path(args.oracle_cli).resolve()
    model_path = Path(args.model_path).resolve()
    projector_path = Path(args.projector_path).resolve()
    work_dir = Path(args.work_dir).resolve()
    root = Path(args.root).resolve(strict=True)
    patched_sha = prepared_head(root)
    require_stamp(oracle_cli.parent.parent / ".skippy-build-stamp", patched_sha, True)
    binary = Path(os.environ["SKIPPY_UPGRADE_SERVING_TEST_BIN"]).resolve(strict=True)
    if not (binary.is_file() and oracle_cli.is_file()):
        raise RuntimeError("missing source-bound serving test binary or oracle")
    candidate_build_dir = os.environ.get("LLAMA_STAGE_BUILD_DIR")
    if not candidate_build_dir:
        raise RuntimeError("LLAMA_STAGE_BUILD_DIR is required for pinned TTS candidate execution")
    require_stamp(Path(candidate_build_dir).resolve() / ".skippy-build-stamp", patched_sha, False)
    for label, path in (("model", model_path), ("projector", projector_path)):
        if not path.is_file():
            raise RuntimeError(f"{label} path is not a file: {path}")
    if args.layer_end < 1 or not args.model:
        raise RuntimeError("model alias and positive layer count are required")
    work_dir.mkdir(parents=True, exist_ok=True)
    candidate_wav = work_dir / "tts-candidate.wav"
    oracle_wav = work_dir / "tts-monolithic-oracle.wav"
    result_path = work_dir / "tts-oracle-result.json"
    # A failed or filtered test must not inherit a WAV or PASS record from an
    # earlier invocation of the same work directory.
    for stale_output in (candidate_wav, oracle_wav, result_path):
        stale_output.unlink(missing_ok=True)
    candidate_env = {
        "LLAMA_STAGE_BACKEND": "cpu",
        "SKIPPY_WORKLOAD_MODEL": str(model_path),
        "SKIPPY_WORKLOAD_PROJECTOR": str(projector_path),
        "SKIPPY_WORKLOAD_MODEL_ID": args.model,
        "SKIPPY_WORKLOAD_LAYER_END": str(args.layer_end),
        "SKIPPY_TTS_ORACLE_CANDIDATE_WAV": str(candidate_wav),
        "SKIPPY_TTS_ORACLE_PROMPT": PROMPT,
        "SKIPPY_TTS_ORACLE_SEED": str(SEED),
        "SKIPPY_TTS_ORACLE_TOP_K": str(TOP_K),
        "SKIPPY_TTS_ORACLE_TOP_P": str(TOP_P),
        "SKIPPY_TTS_ORACLE_MAX_FRAMES": str(MAX_FRAMES),
    }
    run_logged(
        [str(binary), TEST_NAME, "--exact", "--nocapture", "--test-threads=1"],
        work_dir / "tts-candidate-test.log",
        env=candidate_env, root=root,
    )
    if not candidate_wav.is_file():
        raise RuntimeError("deterministic TTS candidate test did not write WAV output")
    # StageConfig leaves weight repacking disabled. llama-tts enables it by
    # default, which changes Q8 logits enough to alter stochastic audio tokens.
    run_logged(
        [str(oracle_cli), "-m", str(model_path), "-mm", str(projector_path),
         "-p", PROMPT, "--output", str(oracle_wav),
         "-n", str(MAX_FRAMES), "--seed", str(SEED),
         "--top-k", str(TOP_K), "--top-p", str(TOP_P),
         "--temp", "1", "--min-p", "0", "--repeat-penalty", "1",
         "--no-repack",
         "-c", str(CONTEXT_SIZE), "-b", str(CONTEXT_SIZE),
         "-ub", str(CONTEXT_SIZE), "-ngl", "0"],
        work_dir / "tts-monolithic-oracle.log", root=root,
    )
    metrics = compare_wavs(candidate_wav, oracle_wav)
    result: dict[str, object] = {
        "status": "pass",
        "class": "speech_synthesis",
        "mode": "deterministic_local_monolithic_pcm_parity",
        "prompt": PROMPT,
        "seed": SEED,
        "top_k": TOP_K,
        "top_p": TOP_P,
        "max_frames": MAX_FRAMES,
        "pinned_patch_sha": patched_sha,
        "thresholds": {
            "max_relative_rms_error": MAX_RELATIVE_RMS_ERROR,
            "min_waveform_cosine": MIN_WAVEFORM_COSINE,
        },
        "metrics": metrics,
        "candidate_wav_sha256": sha256(candidate_wav),
        "oracle_wav_sha256": sha256(oracle_wav),
    }
    result_path.write_text(
        json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return result


def main() -> None:
    """Run the selected TTS comparison and persist its measured evidence."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", required=True)
    parser.add_argument("--oracle-cli", required=True)
    parser.add_argument("--model-path", required=True)
    parser.add_argument("--projector-path", required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--layer-end", type=int, required=True)
    parser.add_argument("--work-dir", required=True)
    result = run_oracle(parser.parse_args())
    print(
        "speech_synthesis local-monolithic oracle passed: "
        + json.dumps(result["metrics"], sort_keys=True)
    )


if __name__ == "__main__":
    try:
        with cancel_on_signal():
            main()
    except (OSError, ValueError, RuntimeError, KeyError, subprocess.SubprocessError, KeyboardInterrupt) as error:
        sys.exit(str(error))
