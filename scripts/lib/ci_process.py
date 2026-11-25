"""Bounded child processes for build and qualification commands."""

import os
import queue
import selectors
import signal
import subprocess
import threading
import time
from contextlib import contextmanager


def environment(overrides=None):
    result = {
        key: value for key, value in os.environ.items()
        if not any(word in key for word in ("TOKEN", "SECRET", "PASSWORD"))
        and key not in {"GITHUB_ENV", "GITHUB_OUTPUT", "GITHUB_PATH", "GITHUB_STEP_SUMMARY",
                        "JIANYANG_API_KEY", "JIANYANG_INFERENCE_SSH_KEY"}
    }
    result.update(overrides or {})
    return result


def start(args, overrides=None, **streams):
    clean_env = streams.pop("clean_env", True)
    env = environment(overrides) if clean_env else overrides
    if os.name == "nt":
        from .windows_job import JobProcess
        return JobProcess(args, env=env, **streams)
    return subprocess.Popen(args, env=env, start_new_session=True, **streams)


def graceful_stop(child):
    if os.name == "nt":
        child.graceful_stop()
    else:
        try:
            os.killpg(child.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass


def force_stop(child):
    if os.name == "nt":
        child.force_stop()
    else:
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    child.wait(timeout=10)


def group_alive(child):
    exited = child.poll() is not None
    if os.name == "nt":
        return child.group_alive()
    try:
        os.killpg(child.pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        if not exited:
            raise
    if not exited:
        return True
    # A group containing only unreaped descendants can still be visible to
    # killpg. Zombies cannot execute and may be owned by another reaper.
    rows = subprocess.check_output(
        ["ps", "-axo", "pgid=,stat="], timeout=5, text=True, env=environment(),
    )
    for row in rows.splitlines():
        group, state = row.split(None, 1)
        if int(group) == child.pid and not state.startswith(("Z", "X")):
            return True
    return False


def stop(child):
    """Wait for graceful termination and fail if forced cleanup was needed."""
    if not group_alive(child):
        child.wait(timeout=10)
        return
    try:
        graceful_stop(child)
    except OSError:
        if group_alive(child):
            force_stop(child)
            raise
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        child.poll()
        if not group_alive(child):
            return
        time.sleep(0.05)
    force_stop(child)
    raise RuntimeError("child process group required forced shutdown")


@contextmanager
def cancel_on_signal():
    def cancel(signum, _frame):
        raise KeyboardInterrupt(f"received signal {signum}")

    handlers = {sig: signal.signal(sig, cancel) for sig in (signal.SIGINT, signal.SIGTERM)}
    try:
        yield
    finally:
        for sig, handler in handlers.items():
            signal.signal(sig, handler)


def captured_unix(child, deadline, limit):
    descriptor = child.stdout.fileno()
    output = bytearray()
    with selectors.DefaultSelector() as selector:
        selector.register(descriptor, selectors.EVENT_READ)
        while True:
            if not selector.select(timeout=max(0, deadline - time.monotonic())):
                raise subprocess.TimeoutExpired(child.args, 0)
            data = os.read(descriptor, min(65536, limit + 1 - len(output)))
            if not data:
                return bytes(output)
            output.extend(data)
            if len(output) > limit:
                raise ValueError(f"captured stdout exceeded limit: {child.args[0]}")


def captured(child, deadline, limit):
    if os.name != "nt":
        return captured_unix(child, deadline, limit)
    result = queue.Queue(maxsize=1)

    def read():
        try:
            result.put(child.stdout.read(limit + 1))
        except OSError as error:
            result.put(error)

    reader = threading.Thread(target=read, daemon=True)
    reader.start()
    try:
        output = result.get(timeout=max(0, deadline - time.monotonic()))
    except queue.Empty:
        raise subprocess.TimeoutExpired(child.args, 0) from None
    if isinstance(output, Exception):
        raise output
    if len(output) > limit:
        raise ValueError(f"captured stdout exceeded limit: {child.args[0]}")
    return output


def command(args, timeout=180, overrides=None, capture_stdout=False, cwd=None, *,
            limit=16 * 1024 * 1024, raw=False, clean_env=True, stdout=None, stderr=None):
    deadline = time.monotonic() + timeout
    with start(args, overrides, cwd=cwd, clean_env=clean_env,
               stdout=subprocess.PIPE if capture_stdout else stdout, stderr=stderr) as child:
        try:
            output = captured(child, deadline, limit) if capture_stdout else None
            child.wait(timeout=max(0, deadline - time.monotonic()))
        finally:
            if group_alive(child):
                stop(child)
        if child.returncode:
            raise subprocess.CalledProcessError(child.returncode, args)
        if output is not None:
            return output if raw else output.decode("utf-8")
    return None


def capture(args):
    return command(args, capture_stdout=True)
