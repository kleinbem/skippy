const { test } = require('node:test');
const assert = require('node:assert/strict');
const { mkdtempSync, mkdirSync, writeFileSync, rmSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { join } = require('node:path');
const { execFileSync } = require('node:child_process');
const mainCIBase = require('./main-ci-base.cjs');

const base = 'a'.repeat(40), head = 'b'.repeat(40);
const context = {
  repo: { owner: 'Mesh-LLM', repo: 'skippy' }, eventName: 'push',
  ref: 'refs/heads/main', sha: head, runId: 100,
};
const run = (extra = {}) => ({
  id: 90, path: '.github/workflows/ci.yml', event: 'push', head_branch: 'main',
  status: 'completed', conclusion: 'success', head_sha: base,
  head_repository: { full_name: 'Mesh-LLM/skippy' }, ...extra,
});

async function select(runs, options = {}) {
  const outputs = {}, warnings = [];
  const github = { rest: { actions: { listWorkflowRuns: async query => {
    assert.deepEqual(query, { ...context.repo, workflow_id: 'ci.yml', branch: 'main',
      event: 'push', status: 'success', per_page: 100 });
    if (options.error) throw new Error('API unavailable');
    return { data: { workflow_runs: runs } };
  } } } };
  await mainCIBase({ github, context: options.context || context,
    core: { info() {}, warning: message => warnings.push(message),
      setOutput: (key, value) => { outputs[key] = value; } },
    ...(options.isAncestor ? { isAncestor: options.isAncestor } : {}),
  });
  return { outputs, warnings };
}

test('only an earlier successful main CI ancestor can establish coverage', async () => {
  const excluded = [
    { id: 100 }, { id: 101 }, { path: '.github/workflows/release.yml' },
    { event: 'workflow_dispatch' }, { head_branch: 'feature' },
    { status: 'in_progress' }, { conclusion: 'failure' }, { conclusion: 'cancelled' },
    { head_repository: { full_name: 'fork/skippy' } }, { head_sha: 'main' },
    { head_sha: head },
  ];
  for (const extra of excluded) {
    const { outputs } = await select([run(extra)], { isAncestor: () => true });
    assert.equal(outputs.force, 'true', JSON.stringify(extra));
  }
  const { outputs } = await select([run({ id: 80 }), run()], { isAncestor: () => true });
  assert.deepEqual(outputs, { base, force: 'false' });
  assert.equal((await select([run()], { isAncestor: () => false })).outputs.force, 'true');
});

test('missing history or an API failure requires both smokes and Graph Contracts', async () => {
  assert.deepEqual((await select([])).outputs, { base: head, force: 'true' });
  const failure = await select([], { error: true });
  assert.equal(failure.outputs.force, 'true');
  assert.match(failure.warnings[0], /API unavailable/);
  for (const overrides of [{ eventName: 'pull_request_target' },
    { ref: 'refs/heads/feature' }, { sha: 'main' }, { repo: { owner: 'fork', repo: 'skippy' } }]) {
    await assert.rejects(() => select([], { context: { ...context, ...overrides } }), /protected main/);
  }
});

test('a later code-only push retains inference changes from a coalesced push', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'skippy-main-ci-'));
  const previous = process.cwd();
  const git = (...args) => execFileSync('git', ['-C', directory, ...args], { encoding: 'utf8' }).trim();
  const commit = message => {
    git('add', '.');
    git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@skippy.local',
      '-c', 'commit.gpgsign=false', 'commit', '-qm', message);
    return git('rev-parse', 'HEAD');
  };
  try {
    git('init', '-q');
    mkdirSync(join(directory, 'crates/skippy-cli/src'), { recursive: true });
    mkdirSync(join(directory, 'scripts/tests'), { recursive: true });
    writeFileSync(join(directory, 'crates/skippy-cli/src/main.rs'), 'base');
    const covered = commit('covered');
    writeFileSync(join(directory, 'crates/skippy-cli/src/main.rs'), 'inference change');
    const coalesced = commit('coalesced inference change');
    writeFileSync(join(directory, 'scripts/tests/inference_contracts.py'), 'test-only change');
    const latest = commit('latest code-only push');
    process.chdir(directory);
    const { outputs } = await select([run({ head_sha: covered })], {
      context: { ...context, sha: latest, payload: { before: coalesced } },
    });
    assert.equal(outputs.base, covered);
    assert.equal(outputs.force, 'false');
    assert.match(git('diff', '--name-only', outputs.base, latest), /crates\/skippy-cli\/src\/main.rs/);
    assert.doesNotMatch(git('diff', '--name-only', coalesced, latest), /crates\/skippy-cli\/src\/main.rs/);
  } finally {
    process.chdir(previous);
    rmSync(directory, { recursive: true, force: true });
  }
});
