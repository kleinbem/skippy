const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { spawnSync } = require('node:child_process');

const workflow = readFileSync(join(__dirname, '../workflows/ci.yml'), 'utf8');
const section = workflow.split('\n  gate:\n')[1];
const script = section.split('          script: |\n')[1].replace(/^ {12}/gm, '');
const evaluate = new Function('process', script);
const declared = section.match(/    needs: \[([\s\S]+?)\]/)[1].split(',').map(s => s.trim());
const targets = JSON.parse(readFileSync(join(__dirname, '../../ci/targets.json'), 'utf8'));
const rows = targets.runtimes.filter(row => row.enabled !== false && targets.enabled_hosts.includes(row.host));
const packageJobs = rows.map(row => `package_${row.id.replaceAll('-', '_')}`);
const prPackages = ['package_darwin_aarch64_metal', 'package_linux_aarch64_cuda13'];
const releaseSmokes = ['release_smoke_linux_x86_64_cuda12', 'release_smoke_linux_aarch64_cuda12', 'release_smoke_linux_x86_64_cuda13'];

function results({ automation = true, code = true, packages = false, native = false, full = false, release = false, hostedRequiresNative = false } = {}) {
  const selected = code || packages;
  const required = ['changes'];
  if (automation) required.push('automation');
  if (selected) required.push('plan');
  if (code) required.push('quality');
  if (full) required.push('native_contracts', ...packageJobs);
  else if (packages) required.push(...prPackages);
  if (native) required.push('native_contracts');
  if (release) required.push(...releaseSmokes);
  const needs = Object.fromEntries(declared.map(job => [job, { result: required.includes(job) ? 'success' : 'skipped' }]));
  needs.changes.outputs = { automation: String(automation), code: String(code), packages: String(packages), native: String(native), hosted_requires_native: String(hostedRequiresNative) };
  if (selected) needs.plan.outputs = { profile: full ? 'full' : 'pr', runtimes: JSON.stringify({ include: rows }) };
  return needs;
}

function gate(needs, release = false) {
  evaluate({ env: { CHECK_RESULTS: JSON.stringify(needs), RELEASE_SMOKE: String(release) } });
}

test('protected pull_request_target skips CI when trusted admission skips changes', () => {
  const condition = section.match(/^    if: (.+)$/m)[1].replace('always()', 'true');
  const shouldRun = new Function('needs', `return (${condition})`);
  assert.equal(shouldRun({ changes: { result: 'skipped' } }), false);
  assert.equal(shouldRun({ changes: { result: 'success' } }), true);
});

test('all eight runtime packages match the planned matrix and depend on the canary', () => {
  assert.equal(packageJobs.length, 8);
  assert.deepEqual(declared.filter(job => job.startsWith('package_')).sort(), packageJobs.sort());
  for (const row of rows) {
    const job = `package_${row.id.replaceAll('-', '_')}`;
    const body = workflow.split(`\n  ${job}:\n`)[1].split(/\n  \w+:\n/)[0];
    assert.match(body, /^    needs: \[plan, quality[\],]/m);
    assert.ok(body.includes(`runtime-id: ${row.id}\n`));
    if (row.backend !== 'cpu') assert.ok(body.includes(`package_${row.host.replaceAll('-', '_')}_cpu`));
    assert.equal(body.includes("needs.changes.outputs.packages == 'true'"), prPackages.includes(job));
  }
  assert.doesNotThrow(() => gate(results({ full: true })));
});

test('only package-affecting pull requests require the two package and smoke jobs', () => {
  for (const options of [{}, { code: false }, { packages: true }, { code: false, packages: true }]) {
    assert.doesNotThrow(() => gate(results(options)));
  }
  const needs = results({ packages: true });
  for (const job of prPackages) {
    const missingSmoke = structuredClone(needs);
    missingSmoke[job].result = 'skipped';
    assert.throws(() => gate(missingSmoke), new RegExp(job));
  }
  const unexpectedSmoke = results();
  unexpectedSmoke[prPackages[0]].result = 'success';
  assert.throws(() => gate(unexpectedSmoke), /must be skipped/);
});

test('automation-only PRs require successful offline checks and skip all builds', () => {
  assert.doesNotThrow(() => gate(results({ code: false })));
  assert.doesNotThrow(() => gate(results({ automation: false, code: false })));
  for (const result of ['failure', 'cancelled', 'skipped', undefined]) {
    const needs = results({ code: false });
    if (result === undefined) delete needs.automation;
    else needs.automation.result = result;
    assert.throws(() => gate(needs), /automation/);
  }
  for (const decision of ['automation', 'code', 'packages', 'native', 'hosted_requires_native']) {
    const needs = results({ code: false });
    delete needs.changes.outputs[decision];
    assert.throws(() => gate(needs), /Path selection/);
  }
});

test('package smoke waits for automation and every applicable source check', () => {
  for (const job of prPackages) {
    const body = workflow.split(`\n  ${job}:\n`)[1].split(/\n  \w+:\n/)[0];
    const condition = body.match(/    if: >-\n([\s\S]+?)\n    runs-on:/)[1]
      .replace('${{', '').replace('}}', '');
    const selected = new Function('needs', 'cancelled', `return (${condition})`);
    for (const code of [false, true]) {
      assert.equal(selected(results({ code, packages: true }), () => false), true);
      for (const result of ['failure', 'cancelled', 'skipped']) {
        const needs = results({ code, packages: true });
        needs.automation.result = result;
        assert.equal(selected(needs, () => false), false);
      }
      const needs = results({ code, packages: true });
      needs.quality.result = code ? 'skipped' : 'failure';
      assert.equal(selected(needs, () => false), false);
    }
    assert.equal(selected(results(), () => false), false);
    assert.equal(selected(results({ full: true }), () => false), true);
    assert.equal(selected(results({ packages: true }), () => true), false);
  }
});

test('ARM64 owns routine CUDA device smoke and x64 retains exact release device smoke', () => {
  const arm = workflow.split('\n  package_linux_aarch64_cuda13:\n')[1].split(/\n  \w+:\n/)[0];
  const row = rows.find(row => row.id === 'linux-aarch64-cuda13');
  assert.ok(arm.includes(`group: ${row.runner_group}`));
  assert.ok(arm.includes(`labels: ${JSON.stringify(row.runner).replaceAll(',', ', ')}`));
  assert.match(arm, /host-id: linux-aarch64\n/);
  assert.match(arm, /build-cli:.*profile == 'pr'/);
  assert.doesNotMatch(arm, /smoke: artifact/);
  const x64 = workflow.split('\n  package_linux_x86_64_cuda13:\n')[1].split(/\n  \w+:\n/)[0];
  assert.match(x64, /skippy-ci-cuda13@/);
  assert.match(x64, /smoke: artifact\n/);
  const release = workflow.split('\n  release_smoke_linux_x86_64_cuda13:\n')[1].split(/\n  \w+:\n/)[0];
  assert.match(release, /needs: \[package_linux_x86_64_cuda13\]/);
  assert.match(release, /runtime_id: linux-x86_64-cuda13\n/);
  assert.match(release, /download-package: 'true'/);
});

test('targeted main validation requires device smokes and selected Graph Contracts', () => {
  const needs = results({ packages: true, native: true });
  assert.doesNotThrow(() => gate(needs));
  for (const job of ['quality', 'native_contracts', ...prPackages]) {
    for (const result of ['skipped', 'failure', 'cancelled', undefined]) {
      const failed = structuredClone(needs);
      if (result === undefined) delete failed[job];
      else failed[job].result = result;
      assert.throws(() => gate(failed), undefined, `${job}: ${result}`);
    }
  }
  const missingSelection = structuredClone(needs);
  delete missingSelection.changes.outputs.native;
  assert.throws(() => gate(missingSelection), /Path selection failed/);
  const nativeJob = workflow.split('\n  native_contracts:\n')[1].split('\n  package_')[0];
  assert.match(nativeJob, /needs: \[plan, quality, changes\]/);
  const expression = nativeJob.match(/^    if: (.+)$/m)[1];
  const selected = new Function('needs', `return (${expression});`);
  assert.equal(selected(needs), true);
  assert.equal(selected(results()), false);
  assert.equal(selected(results({ full: true })), true);
});

test('missing, skipped, failed and cancelled required work cannot pass full CI', () => {
  for (const job of ['changes', 'automation', 'plan', 'quality', 'native_contracts', ...packageJobs]) {
    for (const result of ['skipped', 'failure', 'cancelled', undefined]) {
      const needs = results({ full: true });
      if (result === undefined) delete needs[job];
      else needs[job].result = result;
      assert.throws(() => gate(needs), undefined, `${job}: ${result}`);
    }
  }
  const invalid = results({ full: true });
  invalid.plan.outputs.runtimes = JSON.stringify({ include: rows.map(() => rows[0]) });
  assert.throws(() => gate(invalid), /active runtime matrix/);
});

test('release validation requires the three other GPU smokes', () => {
  assert.doesNotThrow(() => gate(results({ full: true, release: true }), true));
  assert.throws(() => gate(results({ full: true }), true), /release_smoke_/);
  assert.throws(() => gate(results({ full: true, release: true })), /release_smoke_/);
  assert.throws(() => gate(results(), true), /requires full CI/);
});

test('hosted CI cannot satisfy a native package requirement', () => {
  assert.throws(() => gate(results({ hostedRequiresNative: true })), /require native hardware qualification on an isolated runner/);
});

test('the repository switch moves every hosted job to Depot', () => {
  const workflows = ['ci', 'ci-images', 'ci-image-builds', 'docs', 'inference', 'llama-upgrade', 'pr-checks', 'release'];
  for (const name of workflows) {
    const yaml = readFileSync(join(__dirname, '../workflows', name + '.yml'), 'utf8');
    const labels = [...yaml.matchAll(/^    runs-on: (.+)\n((?:      .+\n)*)/gm)]
      .map(([, label, continuation]) => `${label} ${continuation.trim().replaceAll(/\s+/g, ' ')}`.trim());
    assert.ok(labels.some(label => label.includes('SKIPPY_DEPOT_RUNNERS_ENABLED')), name);
    for (const label of labels) {
      if (name.startsWith('llama-upgrade') && label === '${{ matrix.runner }}') continue;
      assert.match(label, /vars.SKIPPY_DEPOT_RUNNERS_ENABLED == 'true'.*depot-/, name);
    }
  }
});

test('ordinary PRs call protected workflows with scoped cache access and read-only permissions', () => {
  const workflows = { 'ci-pr': 'ci', 'ci-images-pr': 'ci', 'docs-pr': 'docs' };
  for (const [caller, callee] of Object.entries(workflows)) {
    const yaml = readFileSync(join(__dirname, '../workflows', caller + '.yml'), 'utf8');
    assert.ok(yaml.includes(`uses: Mesh-LLM/skippy/.github/workflows/${callee}.yml@main`));
    assert.match(yaml, caller === 'ci-pr' ? /^    cache-mode: read$/m : /^    cache-mode: none$/m);
    assert.doesNotMatch(yaml, /runs-on:|secrets:|: write$/m);
    const reusable = readFileSync(join(__dirname, '../workflows', callee + '.yml'), 'utf8');
    assert.doesNotMatch(reusable, /^(?:    )+\w+: write$/m);
  }
});

test('the protected image PR bridge routes only the image workflow', () => {
  const bridge = workflow.split('\n  pr_image_builds:\n')[1].split('\n  changes:\n')[0];
  const admission = bridge.match(/if: >-\n([\s\S]+?)\n    uses:/)[1];
  const eligible = new Function('github', 'vars', 'startsWith', `return (${admission});`);
  const event = {
    repository: 'Mesh-LLM/skippy', event_name: 'pull_request',
    workflow_ref: 'Mesh-LLM/skippy/.github/workflows/ci-images-pr.yml@refs/pull/136/merge',
  };
  const vars = { SKIPPY_DEPOT_RUNNERS_ENABLED: 'true' };
  const startsWith = (value, prefix) => value.startsWith(prefix);
  assert.equal(eligible(event, vars, startsWith), true);
  assert.equal(eligible(event, {}, startsWith), false);
  assert.equal(eligible({ ...event, workflow_ref: event.workflow_ref.replace('ci-images-pr', 'ci-pr') }, vars, startsWith), false);
  assert.match(bridge, /uses: \.\/\.github\/workflows\/ci-image-builds\.yml/);
});

test('host CPU packages install Just on GitHub and Depot runners', () => {
  for (const row of rows) {
    const job = `package_${row.id.replaceAll('-', '_')}`;
    const body = workflow.split(`\n  ${job}:\n`)[1].split(/\n  \w+:\n/)[0];
    assert.equal(body.includes("setup-host: 'true'"), row.backend === 'cpu', job);
  }
  const action = readFileSync(join(__dirname, '../actions/ci-package/action.yml'), 'utf8');
  assert.match(action, /uses: \.\/\.github\/actions\/setup\n      if: inputs.setup-host == 'true'/);
});

test('manual compiler image smoke can skip publication on either runner provider', () => {
  const images = readFileSync(join(__dirname, '../workflows/ci-images.yml'), 'utf8');
  const repository = 'Mesh-LLM/skippy';
  const ref = 'refs/heads/main';
  for (const kind of ['cpu', 'gpu']) {
    const publish = images.split(`\n  ${kind}-publish:\n`)[1];
    const publishPolicy = publish.match(/if: >-\n([\s\S]+?)\n    runs-on:/)[1];
    const publishImage = new Function('github', 'inputs', `return (${publishPolicy});`);
    assert.equal(publishImage({ repository, ref, event_name: 'workflow_dispatch' }, { build_only: true }), false);
    assert.equal(publishImage({ repository, ref, event_name: 'workflow_dispatch' }, { build_only: false }), true);
    assert.equal(publishImage({ repository, ref, event_name: 'push' }, {}), true);
    assert.equal(publishImage({ repository, ref, event_name: 'pull_request' }, {}), false);
  }
});

test('image publication builds once while build-only and PR runs remain read-only', () => {
  const images = readFileSync(join(__dirname, '../workflows/ci-images.yml'), 'utf8');
  const cpu = images.split('\n  cpu-build:\n')[1].split('\n  gpu-build:\n')[0];
  const gpu = images.split('\n  gpu-build:\n')[1].split('\n  cpu-publish:\n')[0];
  const cpuPolicy = cpu.match(/if: >-\n([\s\S]+?)\n    uses:/)[1];
  const gpuPolicy = gpu.match(/build-images: \$\{\{ (.+) \}\}/)[1];
  const buildsCPU = new Function('github', 'inputs', 'vars', `return (${cpuPolicy});`);
  const buildsGPU = new Function('github', 'inputs', `return (${gpuPolicy});`);
  const main = { repository: 'Mesh-LLM/skippy', ref: 'refs/heads/main', event_name: 'push' };
  for (const depot of ['true', 'false']) {
    const vars = { SKIPPY_DEPOT_RUNNERS_ENABLED: depot };
    assert.equal(buildsCPU(main, {}, vars), false);
    assert.equal(buildsGPU(main, {}), false);
    const manual = { ...main, event_name: 'workflow_dispatch' };
    assert.equal(buildsCPU(manual, { build_only: true }, vars), true);
    assert.equal(buildsGPU(manual, { build_only: true }), true);
    assert.equal(buildsCPU(manual, { build_only: false }, vars), false);
    assert.equal(buildsGPU(manual, { build_only: false }), false);
    const pr = { ...main, ref: 'refs/pull/131/merge', event_name: 'pull_request' };
    assert.equal(buildsCPU(pr, {}, vars), depot !== 'true');
    assert.equal(buildsGPU(pr, {}), true);
  }
  const builders = readFileSync(join(__dirname, '../workflows/ci-image-builds.yml'), 'utf8');
  assert.doesNotMatch(builders, /packages: write|docker image push|upload-artifact|image-export/);
  for (const kind of ['cpu', 'gpu']) {
    const publish = images.split(`\n  ${kind}-publish:\n`)[1].split(/\n  [\w-]+:\n/)[0];
    assert.match(publish, /packages: write/);
    const build = publish.indexOf(`just --justfile ci/images.just --working-directory . ci-${kind === 'gpu' ? 'gpu-' : ''}image-build`);
    const smoke = publish.indexOf(`time just --justfile ci/images.just --working-directory . ci-${kind === 'gpu' ? 'gpu-' : ''}image-smoke`);
    const push = publish.indexOf('docker image push');
    assert.ok(build >= 0 && build < smoke && smoke < push);
    assert.doesNotMatch(publish, /download-artifact|image load/);
  }
});

test('container consumers pin images and Cargo snapshots to the selected digest', () => {
  const plan = workflow.split('\n  plan:\n')[1].split('\n  quality:\n')[0];
  const native = workflow.split('\n  native_contracts:\n')[1].split('\n  package_')[0];
  assert.match(native, /skippy-ci-cpu@\$\{\{ needs.plan.outputs.cpu_digest \}\}/);
  assert.match(native, /scope: native-contracts-\$\{\{ needs.plan.outputs.cpu_digest \}\}/);
  const packages = [
    ['linux_x86_64_cuda12', 'cuda12', 'cuda12:latest'],
    ['linux_x86_64_cuda13', 'cuda13', 'cuda13:latest'],
    ['linux_aarch64_cuda12', 'cuda12_arm', 'cuda12:latest-arm64'],
  ];
  for (const [runtime, digest, tag] of packages) {
    const body = workflow.split(`\n  package_${runtime}:\n`)[1].split(/\n  \w+:\n/)[0];
    const output = `\${{ needs.plan.outputs.${digest}_digest }}`;
    assert.ok(plan.includes(`resolve ${digest} ${tag}`));
    assert.ok(body.includes(`skippy-ci-${tag.split(':')[0]}@${output}`));
    assert.ok(body.includes(`compiler-image-digest: ${output}`));
  }
});

test('both image recipe files expose the compiler image build and smoke commands', () => {
  const commands = [
    ['ci-image-build'], ['ci-image-smoke'],
    ['ci-gpu-image-build', 'cuda13', 'example:toolkit', '13.1.2', 'arm64'],
    ['ci-gpu-image-smoke', 'cuda13', 'arm64'],
  ];
  const imageJustfile = ['--justfile', 'ci/images.just', '--working-directory', '.'];
  for (const justfile of [[], imageJustfile]) {
    for (const command of commands) {
      const result = spawnSync('just', [...justfile, '--dry-run', ...command], {
        cwd: join(__dirname, '../..'), encoding: 'utf8',
      });
      assert.equal(result.status, 0, result.stderr);
      const recipe = result.stdout + result.stderr;
      assert.match(recipe, /docker /);
      if (command[0] === 'ci-gpu-image-smoke') {
        assert.match(recipe, /test -s \/usr\/local\/cuda\/EULA\.txt/);
      }
    }
  }
});

test('native CI trusts only an immutable same-repository PR from a writer', async () => {
  const admission = workflow.split('      - name: Verify the same-repository PR author\n')[1].split('\n  automation:\n')[0];
  const script = admission.split('          script: |\n')[1];
  const evaluate = new Function('github', 'context', 'return (async () => {\n' + script + '\n})()');
  const candidate = {
    head: { sha: 'a'.repeat(40), repo: { id: 42 } },
    base: { ref: 'codex/native-decode-port', repo: { id: 42 } },
    user: { login: 'maintainer' },
  };
  const assertAdmission = (pr, permission) => evaluate({ rest: { repos: {
    getCollaboratorPermissionLevel: async ({ username }) => {
      assert.equal(username, 'maintainer');
      return { data: { permission } };
    },
  } } }, { repo: {}, payload: { pull_request: pr, repository: { id: 42 } } });
  await assert.doesNotReject(() => assertAdmission(candidate, 'write'));
  await assert.rejects(() => assertAdmission(candidate, 'read'), /trusted PR author/);
  await assert.rejects(() => assertAdmission({ ...candidate, base: { ...candidate.base, repo: { id: 43 } } }, 'admin'), /same-repository PR/);
  await assert.rejects(() => assertAdmission({ ...candidate, head: { ...candidate.head, repo: { id: 43 } } }, 'admin'), /same-repository PR/);
  await assert.rejects(() => assertAdmission({ ...candidate, head: { ...candidate.head, sha: 'main' } }, 'admin'), /immutable source/);
});

test('main can reuse only a receipt for the exact release dispatch and candidate', async () => {
  const changes = workflow.split('\n  changes:\n')[1].split('\n  plan:\n')[0];
  const receiptScript = changes.split('          script: |\n')[1].split('\n      - uses:')[0];
  const evaluateReceipt = new Function('github', 'context', 'core',
    'return (async () => {\n' + receiptScript + '\n})()');
  const sha = 'a'.repeat(40);
  const base = 'b'.repeat(40);
  const full_name = 'Mesh-LLM/skippy';
  const commit = {
    commit: { message: 'release: prepare v0.81.0\n\nSkippy-Release-Run: 123' },
    parents: [{ sha: base }],
  };
  const run = {
    path: '.github/workflows/release.yml',
    event: 'workflow_dispatch',
    head_branch: 'main',
    head_sha: base,
    head_repository: { full_name },
  };
  const receipt = { name: 'validated-release-' + sha, expired: false };
  const context = { sha, repo: {}, payload: { before: base, repository: { full_name } } };
  async function checked(candidate, workflowRun, artifact) {
    const out = {};
    const github = { rest: {
      repos: { getCommit: async () => ({ data: candidate }) },
      actions: {
        getWorkflowRun: async ({ run_id }) => {
          assert.equal(run_id, '123');
          return { data: workflowRun };
        },
        listWorkflowRunArtifacts: async ({ name }) => {
          assert.equal(name, receipt.name);
          return { data: { artifacts: [artifact] } };
        },
      },
    } };
    const core = {
      info() {},
      warning(message) { assert.fail(message); },
      setOutput(key, value) { out[key] = value; },
    };
    await evaluateReceipt(github, context, core);
    return out.verified;
  }
  assert.equal(await checked(commit, run, receipt), 'true');
  assert.equal(await checked({ ...commit, parents: [{ sha }] }, run, receipt), undefined);
  assert.equal(await checked(commit, { ...run, head_branch: 'other' }, receipt), undefined);
  assert.equal(await checked(commit, run, { ...receipt, name: 'other-receipt' }), undefined);
  assert.equal(await checked(commit, run, { ...receipt, expired: true }), undefined);
});
