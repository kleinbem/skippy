#!/usr/bin/env python3
"""Stage and promote the exact version commit produced by a release run."""

import argparse
import os
import re
import subprocess
import sys

from lib.ci_artifacts import source
from lib.ci_process import cancel_on_signal, command
from lib.release_version import bump, parse_tag, workspace_version


MAIN = "refs/heads/main"


def git(*args, remote=False, output=False, overrides=None):
    options = []
    env = dict(overrides or {})
    if remote and os.environ.get("GH_TOKEN"):
        options = ["-c", "credential.helper=", "-c", "credential.helper=!gh auth git-credential"]
        env.update(GH_TOKEN=os.environ["GH_TOKEN"], GIT_TERMINAL_PROMPT="0")
    return command(["git", *options, *args], timeout=120, overrides=env, capture_stdout=output)


def revision(value):
    if not re.fullmatch(r"[0-9a-f]{40}", value):
        raise ValueError("expected a full commit SHA")
    return value


def candidate_ref():
    run = os.environ["GITHUB_RUN_ID"]
    if not re.fullmatch(r"[1-9][0-9]*", run):
        raise ValueError("expected a numeric release run ID")
    return f"refs/heads/release-candidates/{run}"


def remote_refs(*refs):
    rows = git("ls-remote", "--refs", "origin", *refs, remote=True, output=True)
    return {ref: revision(sha) for sha, ref in (row.split("\t") for row in rows.splitlines())}


def verify_commit(tag, base, candidate):
    parse_tag(tag)
    revision(base)
    revision(candidate)
    run = candidate_ref().rsplit("/", 1)[1]
    if (source() != candidate or workspace_version() != tag[1:]
            or git("show", "-s", "--format=%P", candidate, output=True).strip() != base
            or git("show", "-s", "--format=%(trailers:key=Skippy-Release-Run,valueonly)",
                   candidate, output=True).strip() != run):
        raise ValueError("checkout is not the version commit prepared by this release run")


def next_patch(version, tags):
    """The workspace version if it is unreleased, otherwise the patch after the newest tag."""
    match = re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", version)
    if not match:
        raise ValueError("automatic releases require a stable workspace version")
    series, patch = f"v{match[1]}.{match[2]}.", int(match[3])
    released = [int(tag[len(series):]) for tag in tags
                if tag.startswith(series) and tag[len(series):].isdecimal()]
    return f"{series}{max([patch, *(number + 1 for number in released)])}"


def parents(commit):
    git("fetch", "--no-tags", "--depth=1", "origin", commit, remote=True)
    # The raw commit keeps its parents even where a shallow fetch cut history.
    header = git("cat-file", "commit", commit, output=True).split("\n\n", 1)[0]
    return [line.split()[1] for line in header.splitlines() if line.startswith("parent ")]


def next_release():
    prefix = "refs/tags/"
    # A peeled entry sorts after its tag and gives an annotated tag's commit.
    rows = git("ls-remote", "--tags", "origin", f"{prefix}v*", remote=True, output=True)
    refs = sorted(row.split("\t")[::-1] for row in rows.splitlines())
    tags = {ref.removeprefix(prefix).removesuffix("^{}"): sha for ref, sha in refs}
    head = revision(os.environ["GITHUB_SHA"])
    tag = next_patch(workspace_version(), tags)
    series, patch = tag.rsplit(".", 1)
    # A release tagged beside main is a version commit whose parent is the released head.
    newest = tags.get(f"{series}.{int(patch) - 1}")
    if head not in tags.values() and not (newest and head in parents(newest)):
        print(tag)


def stage(tag):
    prerelease = parse_tag(tag)
    base = revision(os.environ["GITHUB_SHA"])
    ref, tag_ref = candidate_ref(), f"refs/tags/{tag}"
    if source() != base:
        raise ValueError("release must prepare from the dispatched main SHA")
    bump(tag)
    git("add", "--update")
    # A rerun of the same workflow starts at the original SHA and must recreate
    # exactly the same candidate, even after the temporary ref was removed.
    date = git("show", "-s", "--format=%cI", base, output=True).strip()
    git("-c", "user.name=github-actions[bot]",
        "-c", "user.email=41898282+github-actions[bot]@users.noreply.github.com",
        "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-m",
        f"release: prepare {tag}", "-m", f"Skippy-Release-Run: {ref.rsplit('/', 1)[1]}",
        overrides={"GIT_AUTHOR_DATE": date, "GIT_COMMITTER_DATE": date})
    candidate = source()
    verify_commit(tag, base, candidate)
    refs = remote_refs(MAIN, ref, tag_ref)
    existing_tag = refs.get(tag_ref)
    if existing_tag is not None and existing_tag != candidate:
        raise ValueError(f"release tag already exists: {tag}")
    if existing_tag is None and refs.get(MAIN) != base:
        raise ValueError("main advanced. Dispatch a fresh release from main.")
    if refs.get(ref) not in (None, candidate):
        raise ValueError("release candidate ref differs from the prepared commit")
    if ref not in refs:
        git("push", "origin", f"{candidate}:{ref}", remote=True)
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        print(f"tag={tag}\nbase={base}\nsource={candidate}\nprerelease={str(prerelease).lower()}",
              file=output)


def promote_tag(tag, base, candidate):
    """Publish the version commit as a tag one commit ahead of main.

    The built-in Actions token cannot push to protected main, so main keeps its
    version and the release is the tagged commit whose only parent is on main.
    """
    tag_ref = f"refs/tags/{tag}"
    existing = remote_refs(tag_ref).get(tag_ref)
    if existing not in (None, candidate):
        raise ValueError(f"release tag already exists: {tag}")
    git("fetch", "--no-tags", "origin", f"{MAIN}:refs/remotes/origin/main", remote=True)
    git("merge-base", "--is-ancestor", base, "refs/remotes/origin/main")
    if existing is None:
        git("push", "origin", f"{candidate}:{tag_ref}", remote=True)


def promote(tag, base, candidate):
    verify_commit(tag, base, candidate)
    if os.environ.get("SKIPPY_RELEASE_TAG_ONLY"):
        return promote_tag(tag, base, candidate)
    tag_ref = f"refs/tags/{tag}"
    refs = remote_refs(MAIN, tag_ref)
    if refs.get(tag_ref) == candidate:
        # A publish retry may occur after later commits have landed on main.
        git("fetch", "--no-tags", "origin", f"{MAIN}:refs/remotes/origin/main", remote=True)
        git("merge-base", "--is-ancestor", candidate, "refs/remotes/origin/main")
        return
    if tag_ref in refs or refs.get(MAIN) != base:
        raise ValueError("release tag exists or main advanced. Dispatch a fresh release.")
    # The candidate has exactly one parent, base. A regular non-forced push
    # rejects an intervening main commit. Atomic mode binds that to tag creation.
    git("push", "--atomic", "origin", f"{candidate}:{MAIN}", f"{candidate}:{tag_ref}", remote=True)


def cleanup(candidate):
    ref = candidate_ref()
    expected = revision(candidate)
    existing = remote_refs(ref).get(ref)
    if existing is not None and existing != expected:
        raise ValueError("refusing to clean up a changed release candidate")
    if existing == expected:
        git("push", f"--force-with-lease={ref}:{candidate}", "origin", f":{ref}", remote=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    actions.add_parser("next")
    actions.add_parser("stage").add_argument("tag")
    promotion = actions.add_parser("promote")
    for label in ("tag", "base", "candidate"):
        promotion.add_argument(label)
    actions.add_parser("cleanup").add_argument("candidate")
    args = vars(parser.parse_args())
    action = args.pop("action")
    {"next": next_release, "stage": stage, "promote": promote, "cleanup": cleanup}[action](**args)


if __name__ == "__main__":
    try:
        with cancel_on_signal():
            main()
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError,
            KeyboardInterrupt) as error:
        sys.exit(str(error))
