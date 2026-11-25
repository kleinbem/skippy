// Protected-main controller. It reads candidate metadata but never executes it.
const expectedWorkflows = require('./pr-trigger-paths.cjs');
const validateAcceptance = require('./pr-acceptance.cjs');
const workflowPath = run => run.path?.split('@')[0];
const automaticPR = run => ['pull_request', 'pull_request_target'].includes(run.event);
const accepted = new Set(['success', 'skipped', 'neutral']);
const controllerName = 'Evaluate aggregate PR checks';

module.exports = async function aggregate({ github, context, core, depot = false }) {
  const repo = context.repo;
  const trigger = context.payload.workflow_run;
  const source = context.payload.pull_request?.head.sha || trigger?.head_sha ||
    context.payload.check_run?.head_sha || context.payload.sha;
  if (!/^[a-f0-9]{40}$/i.test(source || '')) throw new Error('Invalid PR source SHA');
  let candidates = context.payload.pull_request ? [context.payload.pull_request] :
    trigger?.pull_requests || context.payload.check_run?.pull_requests;
  if (!candidates?.length) {
    candidates = await github.paginate(github.rest.repos.listPullRequestsAssociatedWithCommit,
      { ...repo, commit_sha: source, per_page: 100 });
  }
  for (const candidate of candidates) {
    const { data: pr } = await github.rest.pulls.get({ ...repo, pull_number: candidate.number });
    if (pr.state !== 'open' || pr.head.sha !== source) {
      core.info('Ignoring a closed PR or superseded source');
      continue;
    }
    const status = (state, description) => github.rest.repos.createCommitStatus({
      ...repo, sha: source, context: 'PR checks', state, description,
      target_url: `https://github.com/${repo.owner}/${repo.repo}/actions/runs/${context.runId}`
    });
    try {
      await status('pending', 'Checking all applicable PR workflows');
      validateAcceptance(pr.body);
      const files = await github.paginate(github.rest.pulls.listFiles,
        { ...repo, pull_number: pr.number, per_page: 100 });
      if (files.length < pr.changed_files) {
        throw new Error('GitHub returned an incomplete PR file list');
      }
      const sameRepositoryPR = pr.head.repo?.id != null &&
        pr.head.repo.id === pr.base.repo?.id;
      const expected = expectedWorkflows(files, sameRepositoryPR, depot);
      const runs = await github.paginate(github.rest.actions.listWorkflowRunsForRepo,
        { ...repo, head_sha: source, per_page: 100 });
      const latest = new Map();
      for (const run of runs) {
        if (run.head_sha !== source || !automaticPR(run) ||
            workflowPath(run) === '.github/workflows/pr-checks.yml' ||
            (run.pull_requests?.length && !run.pull_requests.some(p => p.number === pr.number))) continue;
        const key = `${run.workflow_id}:${run.event}`;
        if (!latest.has(key) || latest.get(key).id < run.id) latest.set(key, run);
      }
      let pending = false;
      for (const run of latest.values()) {
        const path = workflowPath(run);
        if (expected.get(path) === run.event) expected.delete(path);
        if (run.status !== 'completed') pending = true;
        else if (!accepted.has(run.conclusion)) throw new Error(`${run.name}: ${run.conclusion || 'incomplete'}`);
      }
      // Include checks from other providers, and independent commit statuses.
      const checks = await github.paginate(github.rest.checks.listForRef,
        { ...repo, ref: source, filter: 'latest', per_page: 100 });
      const latestChecks = new Map();
      for (const check of checks) {
        const key = `${check.app?.id || check.app?.slug || ''}:${check.name}`;
        if (!latestChecks.has(key) || latestChecks.get(key).id < check.id) latestChecks.set(key, check);
      }
      for (const check of latestChecks.values()) {
        if (check.name === controllerName && check.app?.slug === 'github-actions') continue;
        if (check.status !== 'completed') pending = true;
        else if (!accepted.has(check.conclusion)) throw new Error(`${check.name}: ${check.conclusion || 'incomplete'}`);
      }
      const statuses = await github.paginate(github.rest.repos.listCommitStatusesForRef,
        { ...repo, ref: source, per_page: 100 });
      const seen = new Set(['PR checks']);
      for (const check of statuses) {
        if (seen.has(check.context)) continue;
        seen.add(check.context); // API returns newest status first for each context.
        if (check.state === 'pending') pending = true;
        else if (check.state !== 'success') throw new Error(`${check.context}: ${check.state}`);
      }
      // Re-read immediately before publishing: neither an old head nor body can green the PR.
      const { data: current } = await github.rest.pulls.get({ ...repo, pull_number: pr.number });
      if (current.state !== 'open' || current.head.sha !== source) continue;
      validateAcceptance(current.body);
      if (pending || expected.size) {
        await status('pending', 'Waiting for applicable PR checks to finish');
      } else {
        await status('success', 'All applicable PR checks passed or were intentionally skipped');
      }
    } catch (error) {
      await status('failure', String(error.message).slice(0, 140));
      core.setFailed(error.message);
    }
  }
};
