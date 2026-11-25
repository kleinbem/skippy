# Git history extraction

Skippy preserves the development history of the original `skippy/` subtree.
The standalone repository was prepared from original source commit
`937f236c3de65f496324af79739d4aed7b438733` with:

```console
git switch -C main 937f236c3de65f496324af79739d4aed7b438733
FILTER_BRANCH_SQUELCH_WARNING=1 \
  git filter-branch --prune-empty --subdirectory-filter skippy -- main
git update-ref -d refs/original/refs/heads/main
```

The rewrite retained 183 commits that affect the subtree and moved their files
to repository root. Commit hashes necessarily changed because every rewritten
tree has a different path. Authors, author/committer dates, messages, merge
ancestry, and file history were retained.

Only rewritten `main` is published. Original monorepo refs, tags, reflogs, and
unreachable objects must not be transferred to `Mesh-LLM/skippy`.
Standalone build, CI, release, and documentation work is committed after the
rewritten tip so it remains distinct from imported history.

Before the first push, verify:

```console
test "$(git rev-list --count main)" -ge 183
test "$(git log --format=%H --reverse | wc -l | tr -d ' ')" -ge 183
git log --reverse --format='%aN <%aE> %aI %s' | head
git for-each-ref refs/original
git fsck --no-reflogs --unreachable
```

The imported history was also compared to the source path history with
`git log --full-history --reverse`, using author name/email, author timestamp,
and subject as the comparison tuple. All 183 entries matched in order.

An empty `refs/original` is required. Unreachable objects found locally are
removed by expiring reflogs and garbage collecting before the repository is
copied or pushed; they are never part of an advertised ref.
