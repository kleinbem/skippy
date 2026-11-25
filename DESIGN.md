# Skippy documentation design

The documentation is a code-led engineering reference, not a marketing site.
It uses the standard Nextra documentation interaction model so engineers can
scan the navigation, search, copy commands, and move between related pages
without learning custom UI.

## Brand

- The supplied black kangaroo-and-wordmark is the primary identifier.
- The wordmark appears at 132 pixels wide on desktop and 108 pixels on small
  screens. It is inverted in dark mode without altering the source asset.
- Skippy blue (`#2457d6` light, `#8eaeff` dark) is reserved for links, focus,
  selection, and small status accents.
- Neutral Nextra surfaces carry the rest of the interface. Avoid decorative
  gradients, illustrations, or dashboard-style cards.

## Typography and density

- Use the Nextra system sans stack for prose and navigation.
- Use the theme monospace stack for commands, configuration, and measured
  values only.
- Prose is limited to roughly 72 characters per line with a 1.72 line height.
- The first viewport should contain the runnable quickstart before conceptual
  explanation.

## Navigation and content

- Top-level order is Getting Started, Running Models, Pipeline Parallelism,
  KV Caching, Configuration Reference, then Contributing.
- Each page begins with one task or concept and uses copyable examples.
- Pagefind search is available in the header and indexes prose rather than code
  blocks, keeping results focused on operator intent.
- Public pages describe standalone Skippy. MeshLLM material is adaptation input,
  never a product dependency or source of current commands.

## Interaction and accessibility

- Preserve visible keyboard focus with a two-pixel Skippy-blue outline.
- Respect reduced-motion preferences and disable smooth scrolling when set.
- Maintain Nextra's responsive sidebar, table of contents, theme switcher, skip
  navigation, and contrast behavior.
- Every raster asset needs provenance next to the source asset.

## Deployment

The site is a static Next.js export. Pagefind builds a client-side index into
`out/_pagefind`, and the `cf` CLI publishes the complete `out` directory as a
Cloudflare Workers Static Assets application. No runtime database or
server-side rendering is part of the design.
