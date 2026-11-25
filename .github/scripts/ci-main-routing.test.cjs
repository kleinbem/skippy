const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');

const workflow = readFileSync(join(__dirname, '../workflows/ci.yml'), 'utf8');
const filters = readFileSync(join(__dirname, '../path-filters.yml'), 'utf8');
const changes = workflow.split('\n  changes:\n')[1].split('\n    steps:\n')[0];
const main = { event_name: 'push', ref: 'refs/heads/main', event: {} };

function matches(name, files) {
  const rules = filters.match(new RegExp(`^${name}:\\n((?:  .*\\n|\\n)*)`, 'm'))[1]
    .trim().split('\n').map(line => {
      const glob = line.match(/^  - '([^']+)'$/)?.[1] || line.match(/^- '([^']+)'$/)[1];
      const include = !glob.startsWith('!');
      const expression = glob.replace(/^!/, '').split(/(\*\*\/|\*\*|\*)/).map(part => {
        if (part === '**/') return '(?:[^/]+/)*';
        if (part === '**') return '.*';
        if (part === '*') return '[^/]*';
        return part.replaceAll('.', '\\.');
      }).join('');
      return { include, regex: new RegExp(`^${expression}$`) };
    });
  return files.some(file => rules.some(rule => rule.include && rule.regex.test(file)) &&
    !rules.some(rule => !rule.include && rule.regex.test(file)));
}

function selection(github, files, { force = false, verified = false } = {}) {
  const paths = Object.fromEntries(['automation', 'code', 'packages', 'inference', 'packaging', 'native']
    .map(name => [name, String(matches(name, files))]));
  const steps = { paths: { outputs: paths }, release: { outputs: { verified: String(verified) } },
    main_base: { outputs: { force: String(force) } } };
  return Object.fromEntries(['automation', 'code', 'packages', 'native', 'hosted_requires_native'].map(name => {
    const expression = changes.match(new RegExp(`^      ${name}: \\$\\{\\{ (.+) \\}\\}$`, 'm'))[1];
    return [name, new Function('github', 'steps', `return (${expression});`)(github, steps)];
  }));
}

test('main runs device smokes for inference and packaging changes, Graph Contracts for native changes', () => {
  const cases = [
    ['crates/skippy-cli/src/main.rs', true, false],
    ['crates/skippy-cli/tests/smoke.rs', false, false],
    ['crates/skippy-cli/src/helper_tests.rs', false, false],
    ['scripts/tests/inference_contracts.py', false, false],
    ['.github/scripts/pr-checks.cjs', false, false],
    ['crates/skippy-package-builder/src/lib.rs', true, false],
    ['skippy-api-rs/skippy-api/src/lib.rs', true, false],
    ['scripts/lib/ci_smoke.py', true, false],
    ['scripts/inference-execute.py', true, false],
    ['.gi/templates.yaml', true, false],
    ['ci/inference-ladder.json', true, false],
    ['llama_cpp/upstream.txt', true, true],
    ['llama_cpp/patches/core/0001.patch', true, true],
    ['skippy_api/src/api.cpp', true, true],
    ['crates/skippy-ffi/src/lib.rs', true, true],
    ['scripts/tools/skippy-stage-rewriter/main.cpp', true, true],
    ['scripts/tools/skippy-stage-rewriter/tests/rewriter_fixture.cpp', true, true],
    ['scripts/check-skippy-generated-family-patch.sh', true, true],
    ['.github/workflows/ci.yml', true, true],
    ['.github/path-filters.yml', true, true],
    ['.github/actions/ci-package/action.yml', true, true],
    ['Cargo.lock', true, true],
    ['README.md', false, false],
    ['crates/skippy-cli/docs/example.rs', false, false],
  ];
  for (const [file, inference, native] of cases) {
    const actual = selection(main, [file]);
    assert.equal(actual.packages, inference, file);
    assert.equal(actual.native, native, file);
    assert.equal(actual.hosted_requires_native, false, file);
  }
  assert.deepEqual(selection(main, ['scripts/tests/inference_contracts.py'], { force: true }),
    { automation: true, code: true, packages: true, native: true, hosted_requires_native: false });
  assert.deepEqual(selection(main, ['llama_cpp/upstream.txt'], { verified: true }),
    { automation: false, code: false, packages: false, native: false, hosted_requires_native: false });
});

test('PR hardware selection and full nightly/release profile selection stay unchanged', () => {
  const pr = event_name => ({ event_name, event: { pull_request: {} } });
  assert.deepEqual(selection(pr('pull_request_target'), ['crates/skippy-cli/src/main.rs']),
    { automation: true, code: true, packages: true, native: false, hosted_requires_native: false });
  assert.deepEqual(selection(pr('pull_request'), ['crates/skippy-cli/src/main.rs']),
    { automation: true, code: true, packages: false, native: false, hosted_requires_native: true });
  assert.equal(selection(pr('pull_request_target'), ['scripts/tests/inference_contracts.py']).packages, false);
  assert.equal(selection(pr('pull_request_target'), ['crates/skippy-package-builder/src/lib.rs']).packages, false);
  const profile = workflow.match(/SKIPPY_CI_PROFILE: \$\{\{ (.+) \}\}/)[1];
  const selectProfile = new Function('github', 'inputs', `return (${profile});`);
  assert.equal(selectProfile(main, {}), 'pr');
  assert.equal(selectProfile({ ...main, event_name: 'schedule' }, {}), 'full');
  assert.equal(selectProfile({ ...main, event_name: 'workflow_call' }, { profile: 'full' }), 'full');
  const inference = readFileSync(join(__dirname, '../workflows/inference.yml'), 'utf8');
  assert.doesNotMatch(inference, /^  push:/m);
});

test('main path selection and package jobs remain bound to the exact merged SHA', () => {
  assert.match(workflow, /ref: \$\{\{ github.event.pull_request.base.sha \|\| github.sha \}\}/);
  assert.match(workflow, /fetch-depth: \$\{\{ github.event_name == 'push' && '0' \|\| '1' \}\}/);
  assert.match(workflow, /base: \$\{\{ steps.main_base.outputs.base \}\}\n          ref: \$\{\{ github.sha \}\}/);
  assert.match(workflow, /SKIPPY_CI_SOURCE_REF: .+\|\| github.sha \}\}/);
  for (const job of ['package_darwin_aarch64_metal', 'package_linux_aarch64_cuda13']) {
    const body = workflow.split(`\n  ${job}:\n`)[1].split(/\n  \w+:\n/)[0];
    assert.match(body, /needs: \[plan, quality,/);
    assert.match(body, /ref: \$\{\{ env.SKIPPY_CI_SOURCE_REF \}\}/);
    assert.match(body, /build-cli: \$\{\{ needs.plan.outputs.profile == 'pr' \}\}/);
  }
});
