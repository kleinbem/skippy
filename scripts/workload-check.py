#!/usr/bin/env python3
"""Exercise certified workload endpoints and compare independent oracles."""

import argparse
import base64
import io
import math
import struct
import subprocess
import sys
import wave
from pathlib import Path

from lib.ci_process import cancel_on_signal, command
from lib.workload_http import request, transcription

INPUTS = [
    "search_query: distributed GPU inference",
    "search_document: GPUs share one language model over a mesh",
    "search_document: A recipe for tomato soup",
]
RERANK_QUERY = "distributed GPU inference"
RERANK_DOCUMENTS = ["GPUs share one language model over a mesh", "A recipe for tomato soup"]
ENCODER_DECODER_PROMPT = "translate English to German: The house is wonderful."
CLASSES = ("embedding", "rerank", "encoder_decoder", "ocr", "speech_synthesis", "speech_recognition")


def number(value):
    return type(value) in {int, float} and math.isfinite(value)


def positive(value):
    return type(value) is int and value > 0


def vector_rows(response, count):
    rows = response["data"]
    if len(rows) != count or any(type(row["index"]) is not int or row["index"] != index
                                 for index, row in enumerate(rows)):
        raise ValueError("embedding response has invalid cardinality or indexes")
    return rows


def vector(value):
    if not isinstance(value, list) or not value or not all(map(number, value)):
        raise ValueError("embedding response requires nonempty finite vectors")
    return value


def dot(left, right):
    return sum(a * b for a, b in zip(left, right))


def embedding_smoke(numeric, encoded, model):
    values = []
    for count, response in ((3, numeric), (1, encoded)):
        rows = vector_rows(response, count)
        if (response["object"], response["model"]) != ("list", model) or any(
            row["object"] != "embedding" for row in rows
        ):
            raise ValueError("embedding envelope differs from request")
        values.append([row["embedding"] for row in rows])
    query, match, unrelated = map(vector, values[0])
    if not positive(numeric["usage"]["prompt_tokens"]):
        raise ValueError("embedding prompt usage is empty")
    if any(len(row) != len(query) or abs(math.sqrt(dot(row, row)) - 1) > 1e-4
           for row in (query, match, unrelated)):
        raise ValueError("embedding dimensions or normalization failed")
    if dot(query, match) <= dot(query, unrelated):
        raise ValueError("embedding semantic ordering failed")
    data = base64.b64decode(values[1][0], validate=True)
    if len(data) != 4 * len(query):
        raise ValueError("base64 embedding byte length differs")
    for (actual,), expected in zip(struct.iter_unpack("<f", data), query):
        if not math.isfinite(actual) or abs(actual - expected) > max(1e-6, 1e-5 * max(abs(actual), abs(expected))):
            raise ValueError("base64 embedding differs from float response")


def rankings(response):
    rows = []
    for row in response["results"]:
        index, score = row["index"], row["relevance_score"]
        if type(index) is not int or not number(score):
            raise ValueError("invalid rerank index or score")
        rows.append((index, score))
    if sorted(index for index, _ in rows) != [0, 1]:
        raise ValueError("invalid rerank result cardinality or indexes")
    return rows


def rerank(base, model):
    return request(base, "/rerank", {"model": model, "query": RERANK_QUERY,
                                    "documents": RERANK_DOCUMENTS, "return_documents": True})


def complete(base, model, seed=None):
    payload = {"model": model, "prompt": ENCODER_DECODER_PROMPT, "max_tokens": 32, "temperature": 0}
    if seed is not None:
        payload["seed"] = seed
    return request(base, "/completions", payload)


def answer(response, key):
    rows = response["choices"]
    if len(rows) != 1:
        raise ValueError("invalid completion choices")
    return rows[0][key]


def audio(data):
    with wave.open(io.BytesIO(data), "rb") as sound:
        frames = sound.getnframes()
        if sound.getsampwidth() != 2 or frames < sound.getframerate() // 10:
            raise ValueError("unsupported WAV samples or too little audio")
        samples = sound.readframes(frames)
        if not samples or len(samples) != 2 * frames * sound.getnchannels():
            raise ValueError("invalid WAV length")
    if sum(value * value for (value,) in struct.iter_unpack("<h", samples)) < len(samples) // 2:
        raise ValueError("silent audio")


def smoke(args):
    base, model, model_class = args.base_url, args.model, args.model_class
    if model_class == "embedding":
        query = {"model": model, "input": INPUTS, "encoding_format": "float"}
        encoded = dict(query, input=INPUTS[0], encoding_format="base64")
        embedding_smoke(request(base, "/embeddings", query), request(base, "/embeddings", encoded), model)
    elif model_class == "rerank":
        result = rerank(base, model)
        ranks = dict(rankings(result))
        if (ranks[0] <= ranks[1] or not positive(result["usage"]["prompt_tokens"])
                or any(not isinstance(row["document"], str) for row in result["results"])):
            raise ValueError("rerank semantic ordering or usage failed")
    elif model_class == "encoder_decoder":
        response = complete(base, model)
        if "haus" not in answer(response, "text").lower() or not positive(response["usage"]["completion_tokens"]):
            raise ValueError("translation anchor or usage failed")
    elif model_class == "speech_synthesis":
        audio(request(base, "/audio/speech", {"model": model, "input": "The mesh is ready.",
                                              "voice": "default", "response_format": "wav"}))
    else:
        if not args.media_path:
            raise ValueError("workload requires --media-path")
        path = Path(args.media_path)
        if model_class == "ocr":
            encoded = base64.b64encode(path.read_bytes()).decode("ascii")
            content = [{"type": "text", "text": "Read all visible text. Return only the transcription."},
                       {"type": "image_url", "image_url": {"url": "data:image/png;base64," + encoded}}]
            response = request(base, "/chat/completions", {"model": model, "messages": [
                {"role": "user", "content": content}], "max_tokens": 32, "temperature": 0})
            message = answer(response, "message")
        else:
            message = transcription(base, model, path.read_bytes(), path.name)
        if not message.get("text", message.get("content", "")).strip():
            raise ValueError("transcription is empty")
    print(f"OpenAI HTTP {model_class} smoke passed: model={model}")


def unit(row):
    scale = max(map(abs, row))
    if scale == 0:
        raise ValueError("embedding oracle response has a zero vector")
    scaled = [element / scale for element in row]
    norm = math.sqrt(dot(scaled, scaled))
    return [element / norm for element in scaled]


def compare_vectors(left, right, count):
    delta, cosine = 0.0, 1.0
    for a, b in zip(vector_rows(left, count), vector_rows(right, count)):
        x, y = vector(a["embedding"]), vector(b["embedding"])
        if len(x) != len(y):
            raise ValueError("embedding dimensions differ from monolithic reference")
        delta = max(delta, *(abs(i - j) for i, j in zip(x, y)))
        cosine = min(cosine, dot(unit(x), unit(y)))
    if delta > 1e-4 or cosine < 0.99999:
        raise ValueError(f"embedding differs from monolithic reference: max_abs_delta={delta}, min_cosine={cosine}")
    return f"max_abs_delta={delta}, min_cosine={cosine}"


def oracle(args):
    candidate, reference, model = args.candidate_url, args.oracle_url, args.model
    kind = args.model_class
    if kind == "encoder_decoder":
        if reference or not args.oracle_completion or not args.model_path:
            raise ValueError("encoder_decoder requires --oracle-completion and --model-path")
        executable = args.oracle_completion.resolve(strict=True)
        if not executable.is_file() or not args.model_path.is_file():
            raise ValueError("native completion requires executable and model files")
        output = command([
            str(executable), "-m", str(args.model_path), "-p", ENCODER_DECODER_PROMPT, "-n", "32",
            "-c", "0", "-b", "2048", "-ub", "2048", "-ngl", "0", "-s", "1", "--temp", "0",
            "--no-repack", "--no-display-prompt", "--simple-io",
        ], timeout=240, capture_stdout=True, limit=4 * 1024 * 1024)
        expected = " ".join(output.strip().removesuffix("[end of text]").split())
        observed = " ".join(answer(complete(candidate, model, seed=1), "text").split())
        if not observed or expected != observed:
            raise ValueError("encoder-decoder text differs from monolithic reference")
        detail = f"identical normalized text={observed!r}"
    elif not reference or args.oracle_completion or args.model_path:
        raise ValueError("embedding/rerank require --oracle-url only")
    elif kind == "embedding":
        details, failures = [], []
        for inputs in [INPUTS, *INPUTS]:
            payload = {"model": model, "input": inputs, "encoding_format": "float"}
            left, right = [request(base, "/embeddings", payload) for base in (candidate, reference)]
            try:
                details.append(compare_vectors(left, right, len(INPUTS) if isinstance(inputs, list) else 1))
            except ValueError as error:
                failures.append(str(error))
        if failures:
            raise ValueError("; ".join(failures))
        detail = "; ".join(details)
    else:
        left, right = [rankings(rerank(base, model)) for base in (candidate, reference)]
        if [index for index, _ in left] != [index for index, _ in right]:
            raise ValueError("rerank differs from monolithic reference: wire order")
        delta = max(abs(a - b) for (_, a), (_, b) in zip(left, right))
        if delta > 1e-4:
            raise ValueError("rerank differs from monolithic reference: scores")
        detail = f"max_abs_delta={delta}, order={[index for index, _ in left]}"
    print(f"{kind} local-monolithic oracle passed: {detail}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    smoke_parser = actions.add_parser("workload-smoke")
    smoke_parser.add_argument("--base-url", required=True)
    smoke_parser.add_argument("--media-path")
    smoke_parser.set_defaults(run=smoke)
    oracle_parser = actions.add_parser("workload-monolithic-oracle")
    oracle_parser.add_argument("--candidate-url", required=True)
    oracle_parser.add_argument("--oracle-url")
    oracle_parser.add_argument("--oracle-completion", type=Path)
    oracle_parser.add_argument("--model-path", type=Path)
    oracle_parser.set_defaults(run=oracle)
    for action, classes in ((smoke_parser, CLASSES), (oracle_parser, CLASSES[:3])):
        action.add_argument("--class", dest="model_class", required=True, choices=classes)
        action.add_argument("--model", required=True)
    args = parser.parse_args()
    args.run(args)


if __name__ == "__main__":
    try:
        with cancel_on_signal():
            main()
    except (OSError, ValueError, TypeError, KeyError, wave.Error, EOFError,
            RuntimeError, subprocess.SubprocessError, KeyboardInterrupt) as error:
        sys.exit(str(error))
