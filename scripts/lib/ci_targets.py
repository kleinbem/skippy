"""Load the validated target matrix and bind jobs to their declared host."""

import json
import platform

from .ci_artifacts import read_json
from .ci_process import capture


def targets():
    plan = json.loads(capture(["bash", "scripts/ci-plan.sh"]))
    return (
        {row["id"]: row for row in plan["hosts"]["include"]},
        {row["id"]: row for row in plan["runtimes"]["include"]},
    )


def require_host(host):
    operating_system = {"Darwin": "macos", "Linux": "linux", "Windows": "windows"}.get(
        platform.system(), platform.system()
    )
    machine = platform.machine().lower()
    architecture = {"amd64": "x86_64", "arm64": "aarch64"}.get(machine, machine)
    if (operating_system, architecture) != (host["os"], host["arch"]):
        raise ValueError(
            f'{host["id"]} requires {host["os"]}/{host["arch"]}, '
            f'running {operating_system}/{architecture}'
        )


def validate_native_identity(directory, host, runtime, expected_arches=None):
    manifest = read_json(directory / "manifest.json")["runtime"]
    platform_value, backend = manifest["platform"], manifest["backend"]
    if (
        (platform_value["os"], platform_value["arch"], backend["kind"])
        != (host["os"], host["arch"], runtime["backend"])
    ):
        raise ValueError("native manifest platform/backend does not match matrix row")
    if runtime["backend"] == "cuda":
        major = backend["cuda"]["toolkit_major"]
        if type(major) is not int or major != int(runtime["toolkit"].split(".")[0]):
            raise ValueError("CUDA manifest toolkit mismatch")
        if expected_arches is not None and backend["cuda"]["gpu_arches"] != expected_arches.split(";"):
            raise ValueError(f"CUDA manifest architectures differ from {expected_arches}")
