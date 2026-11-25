<!-- BEGIN:nextjs-agent-rules -->

## This is NOT the Next.js you know

This version has breaking changes — APIs, conventions, and file structure may all differ from your training data. Read the relevant guide in `node_modules/next/dist/docs/` (resolved from this file's directory; in monorepos the `next` package may not be visible from the repo root) before writing any code. Heed deprecation notices.

This block is written and re-added by `next dev` — verify at `node_modules/next/dist/server/lib/generate-agent-files.js`. Removing it from a diff only re-creates the uncommitted change; committing it with your work keeps the tree clean.

<!-- END:nextjs-agent-rules -->

## Documentation pages

Every public documentation page must answer these six questions:

1. What is this?
2. Why would I use it?
3. What is the shortest working example?
4. What does Skippy do internally?
5. What commonly goes wrong?
6. Where are the exact options and defaults?

Organize each page so a reader encounters the answers in that order. The
questions do not need to appear as literal headings, but the answers must be
easy to find. Put the shortest copyable example near the beginning, explain
behavior and failure modes after the task is clear, and end with links or tables
for the authoritative commands, settings, accepted values, and defaults.

Keep examples valid for standalone Skippy. Verify commands, options, defaults,
and behavioral claims against the current repository sources before publishing.
