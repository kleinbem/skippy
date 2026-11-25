const { spawnSync } = require('node:child_process');

// Include changes from coalesced or failed pushes until a main run covers them.
module.exports = async function mainCIBase({ github, context, core, isAncestor = (base, head) =>
  spawnSync('git', ['merge-base', '--is-ancestor', base, head]).status === 0 }) {
  if (context.eventName !== 'push' || context.ref !== 'refs/heads/main' ||
      `${context.repo.owner}/${context.repo.repo}` !== 'Mesh-LLM/skippy' ||
      !/^[a-f0-9]{40}$/.test(context.sha)) {
    throw new Error('Main CI comparison requires a pinned push to protected main');
  }
  try {
    const { data } = await github.rest.actions.listWorkflowRuns({
      ...context.repo, workflow_id: 'ci.yml', branch: 'main', event: 'push',
      status: 'success', per_page: 100,
    });
    const runs = data.workflow_runs.filter(run =>
      run.id < context.runId && run.path === '.github/workflows/ci.yml' &&
      run.event === 'push' && run.head_branch === 'main' &&
      run.status === 'completed' && run.conclusion === 'success' &&
      run.head_repository?.full_name === 'Mesh-LLM/skippy' &&
      /^[a-f0-9]{40}$/.test(run.head_sha) && run.head_sha !== context.sha
    ).sort((a, b) => b.id - a.id);
    for (const run of runs) {
      if (!isAncestor(run.head_sha, context.sha)) continue;
      core.info(`Classifying main changes since successful CI run ${run.id} (${run.head_sha})`);
      core.setOutput('base', run.head_sha);
      core.setOutput('force', 'false');
      return;
    }
  } catch (error) {
    core.warning(`Could not verify previous main CI coverage: ${error.message}`);
  }
  core.info('No successful ancestor run verified; requiring both device smokes and Graph Contracts');
  core.setOutput('base', context.sha);
  core.setOutput('force', 'true');
};
