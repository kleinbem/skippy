"""Bounded local HTTP exchanges for workload certification."""

import json
import tempfile
from pathlib import Path
from urllib.parse import urlsplit

from .ci_process import command


def post(url, content_type, body, expected):
    address = urlsplit(url)
    if address.scheme != "http" or not address.hostname or address.username or address.fragment:
        raise ValueError("workload smoke requires an HTTP endpoint")
    with tempfile.TemporaryDirectory() as tmp:
        data = Path(tmp) / "payload"
        data.write_bytes(body)
        response = command(
            ["curl", "--fail", "--silent", "--show-error", "--max-time", "240",
             "--noproxy", "*", "--header", f"Content-Type: {content_type}",
             "--data-binary", f"@{data}", "--write-out", "\n%{content_type}", url],
            timeout=245, capture_stdout=True, raw=True, limit=64 * 1024 * 1024 + 1024,
        )
    payload, sep, media = response.rpartition(b"\n")
    if not sep or media.split(b";")[0].strip().decode("ascii") != expected:
        raise ValueError("unexpected workload response content type")
    if len(payload) > 64 * 1024 * 1024:
        raise ValueError("workload response exceeds 64 MiB")
    return payload


def request(base, path, payload):
    media = "audio/wav" if path == "/audio/speech" else "application/json"
    result = post(base.rstrip("/") + path, "application/json",
                  json.dumps(payload, ensure_ascii=False).encode("utf-8"), media)
    return result if media == "audio/wav" else json.loads(result)


def transcription(base, model, media, filename):
    if any(ch in model for ch in "\r\n") or any(ch in filename for ch in '\r\n"'):
        raise ValueError("invalid multipart field")
    boundary = "skippy-workload-smoke"
    prefix = (f"--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n{model}\r\n"
              f"--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n"
              "Content-Type: audio/wav\r\n\r\n")
    body = prefix.encode("utf-8") + media + f"\r\n--{boundary}--\r\n".encode("ascii")
    return json.loads(post(base.rstrip("/") + "/audio/transcriptions",
                           f"multipart/form-data; boundary={boundary}", body, "application/json"))
