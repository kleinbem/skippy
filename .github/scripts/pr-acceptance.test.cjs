const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const validate = require('./pr-acceptance.cjs');
const template = readFileSync(join(__dirname, '../PULL_REQUEST_TEMPLATE.md'), 'utf8');
const complete = template.slice(template.indexOf('## Acceptance criteria'))
  .replace(/<!--[\s\S]*?-->/g, '').replaceAll('- [ ]', '- [x]');

test('the unchanged PR template blocks until every standard criterion is checked', () => {
  assert.throws(() => validate(template), /Acceptance criteria incomplete: I ran/);
  assert.throws(() => validate(template.replace('- [ ]', '- [x]')), /Acceptance criteria incomplete: I agree/);
  assert.doesNotThrow(() => validate(complete));
  assert.doesNotThrow(() => validate(complete.replaceAll('[x]', '[X]').replaceAll('\n', '\r\n')));
});

test('missing, empty, deleted and duplicate criteria cannot bypass the gate', () => {
  for (const body of [null, '', '## Summary\n- [x] Done', '## Acceptance criteria\n',
    '## Acceptance criteria\n- [x] Something else',
    complete.split('\n').slice(0, -2).join('\n'), `${complete}\n${complete}`]) {
    assert.throws(() => validate(body), /Acceptance criteria/);
  }
});

test('every additional task must be checked, even when standard criteria are complete', () => {
  for (const marker of [' ', '', '?', 'X ', '-']) {
    assert.throws(() => validate(`${complete}\n- [${marker}] Regression reproduced`), /incomplete: Regression reproduced/);
  }
  assert.throws(() => validate(`${complete}\n- [x]`), /empty task/);
  assert.doesNotThrow(() => validate(`${complete}\n1. [x] Regression reproduced`));
  assert.doesNotThrow(() => validate(`${complete}\n### Regression\n* [X] Verified`));
  assert.doesNotThrow(() => validate(`${complete}\n## Follow-up\n- [ ] Separate future work`));
});

test('comments, fenced examples, blockquotes and indented code do not supply required criteria', () => {
  for (const example of [`<!--\n${complete}\n-->`, `\`\`\`md\n${complete}\n\`\`\``,
    `~~~~md\n${complete}\n~~~~`, complete.replace(/^/gm, '> '), complete.replace(/^/gm, '    ')]) {
    assert.throws(() => validate(example), /Acceptance criteria/);
    assert.doesNotThrow(() => validate(`${example}\n${complete}`));
  }
  assert.throws(() => validate(`## Acceptance criteria\n<!--\n${complete}\n-->`), /checklist is empty/);
  assert.doesNotThrow(() => validate(`${complete}\n\`\`\`md\n- [ ] Example only\n\`\`\``));
  assert.doesNotThrow(() => validate(`${complete}\n<!-- - [ ] Maintainer note -->`));
  assert.doesNotThrow(() => validate(`\`\`\`html\n<!-- Example comment opener\n\`\`\`\n${complete}`));
});

test('nested tasks are enforced while indented code examples stay ignored', () => {
  assert.throws(() => validate(`${complete}\n    - [ ] Nested requirement`), /incomplete: Nested requirement/);
  assert.throws(() => validate(`${complete}\n- Evidence\n    - [ ] Nested requirement`), /incomplete: Nested requirement/);
  assert.doesNotThrow(() => validate(`${complete}\n    - [x] Nested requirement`));
  assert.doesNotThrow(() => validate(`${complete}\n\n      - [ ] Indented code example`));
});

test('section headings tolerate casing and closing hashes; examples cannot end the real section', () => {
  assert.doesNotThrow(() => validate(complete.replace('## Acceptance criteria', '## ACCEPTANCE CRITERIA ##')));
  assert.throws(() => validate(`${complete}\n\`\`\`\n## Other\n\`\`\`\n- [ ] Still required`), /incomplete: Still required/);
});
