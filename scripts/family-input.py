#!/usr/bin/env python3
"""Project source identity from a model path or select local loopback ports."""

import argparse
import json
import re
import socket
from contextlib import ExitStack
from pathlib import Path


def identity(model_id, path):
    result = {"model_id": model_id}
    parts = Path(path).parts
    for index, part in enumerate(parts):
        if not part.startswith("models--") or parts[index + 1:index + 2] != ("snapshots",):
            continue
        if len(parts) < index + 4:
            continue
        repo = part[len("models--"):].replace("--", "/")
        revision = parts[index + 2]
        file = "/".join(parts[index + 3:])
        name = parts[-1]
        stem = re.sub(r"-00001-of-[0-9]{5}$", "", name.removesuffix(".gguf"))
        result.update(source_repo=repo, source_revision=revision, source_file=file,
                      canonical_ref=f"{repo}@{revision}/{file}", distribution_id=stem)
        quant = (re.search(r"(?a)(?:UD-)?(?:(?:IQ|Q)[0-9][A-Z0-9_]*|BF16|F16|F32)$", stem, re.I)
                 if name.endswith(".gguf") else None)
        if quant:
            result["selector"] = quant[0]
        break
    return result


def snapshot_revision(path):
    parts = Path(path).parts
    values = {revision for marker, revision in zip(parts, parts[1:])
              if marker == "snapshots" and re.fullmatch(r"[0-9a-f]{40,64}", revision)}
    if len(values) != 1:
        raise ValueError("path needs exactly one immutable snapshot revision")
    return values.pop()


def ports(count):
    if not 1 <= count <= 16:
        raise ValueError("port count must be between 1 and 16")
    with ExitStack() as stack:
        sockets = [stack.enter_context(socket.socket()) for _ in range(count)]
        for listener in sockets:
            listener.bind(("127.0.0.1", 0))
        result = [str(listener.getsockname()[1]) for listener in sockets]
    return ",".join(result)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    sub.add_parser("local-ports").add_argument("count", type=int)
    model = sub.add_parser("family-model-identity")
    model.add_argument("model_id", nargs="?")
    model.add_argument("path", nargs="?")
    model.add_argument("--snapshot-revision")
    args = parser.parse_args()
    try:
        if args.action == "local-ports":
            print(ports(args.count))
        elif args.snapshot_revision and not args.model_id:
            print(snapshot_revision(args.snapshot_revision))
        elif args.model_id is not None and args.path is not None and not args.snapshot_revision:
            print(json.dumps(identity(args.model_id, args.path), sort_keys=True, separators=(",", ":")))
        else:
            parser.error("provide MODEL_ID MODEL_PATH or --snapshot-revision PATH")
    except (OSError, ValueError) as error:
        parser.exit(1, f"{error}\n")
