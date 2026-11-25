const { readFileSync } = require('node:fs');
const { join } = require('node:path');

const producers = new Map([
  ['ci.yml', 'pull_request'],
  ['inference.yml', 'pull_request_target'],
  ['docs.yml', 'pull_request'],
  ['ci-images.yml', 'pull_request'],
]);

// Read the literal PR path lists in the protected workflow files. Reject
// unsupported YAML or glob syntax so a trigger edit cannot silently pass PRs.
function workflowPaths(workflow, eventName) {
  const yaml = readFileSync(join(__dirname, '../workflows', workflow), 'utf8');
  const event = yaml.match(new RegExp(`^  ${eventName}:\\n((?:    .*\\n|\\n)*)`, 'm'))?.[1];
  const paths = event?.match(/^    paths:(?: \[([^\n]+)\]\n|\n((?:      .*\n)+))/m);
  if (!paths) throw new Error(`Missing native PR paths for ${workflow}`);
  const entries = paths[1]
    ? paths[1].split(/,\s*/)
    : paths[2].trimEnd().split('\n').map(line => line.replace(/^      - /, ''));
  const rules = entries.map(entry => {
    const match = /^'(!?)([\w./*-]+)'$/.exec(entry);
    if (!match) throw new Error(`Unsupported PR path entry in ${workflow}: ${entry}`);
    const [, negated, glob] = match;
    const expression = glob.split(/(\*\*\/|\*\*|\*)/).map(part => {
      if (part === '**/') return '(?:[^/]+/)*';
      if (part === '**') return '.*';
      if (part === '*') return '[^/]*';
      return part.replaceAll('.', '\\.');
    }).join('');
    return { include: !negated, regex: new RegExp(`^${expression}$`, 's') };
  });
  if (!rules.some(rule => rule.include)) throw new Error(`No included PR paths for ${workflow}`);
  return rules;
}

function expectedWorkflows(files, sameRepositoryPR = false, depot = false) {
  const filenames = files.flatMap(({ filename, previous_filename }) => [filename, previous_filename]).filter(Boolean);
  const expected = new Map();
  for (const [workflow, defaultEvent] of producers) {
    const eventName = workflow === 'ci.yml' && sameRepositoryPR ? 'pull_request_target' : defaultEvent;
    const producer = depot && eventName === 'pull_request' ? workflow.replace('.yml', '-pr.yml') : workflow;
    const rules = workflowPaths(producer, eventName);
    // GitHub applies matching path patterns in order, with the last match
    // determining whether each changed file is included.
    if (filenames.some(filename => rules.findLast(rule => rule.regex.test(filename))?.include)) {
      expected.set(`.github/workflows/${producer}`, eventName);
    }
  }
  return expected;
}

module.exports = expectedWorkflows;
