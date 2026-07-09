# huck website — design

**Goal:** A marketing + blog website for the huck shell, built in-repo with
Next.js (App Router), TypeScript, and Tailwind, deployed on Vercel. It
highlights huck's features and carries a blog/updates section that tracks the
development process. The blog is generated from static MDX files committed to
the repo — no CMS, no server, no database.

This is a new subsystem, independent of the Rust shell. It ships via a pull
request to `main` (the maintainer reviews and merges), consistent with the
project's issue+PR workflow.

## Goals & non-goals

**Goals**
- A fast, static, developer-facing site that explains what huck is and why it
  exists, with real huck code samples.
- A blog whose posts are plain MDX files (frontmatter + body) — "the static
  data" — rendered with full design control.
- Zero coupling to the Rust workspace: Cargo and the Rust CI never see the site.
- One-command local dev; every push gets a Vercel preview deploy.

**Non-goals**
- No server-side rendering at request time, no API routes, no database, no auth.
- No headless CMS or hosted blog service (content lives in the repo).
- No extra GitHub Actions job for the site — Vercel preview builds are the CI.
- Not a documentation site (the `docs/superpowers/` design trail stays in the
  repo; the site may *link* to GitHub, not mirror it).

## Architecture & repo placement

A self-contained Next.js app in **`site/`** at the repo root, a sibling of
`crates/` and `docs/`. Cargo has no visibility into it; the existing
`.github/workflows/ci.yml` cargo jobs never touch it.

```
site/
  app/
    layout.tsx               # root layout: nav, footer, theme, fonts
    page.tsx                 # home — hero + feature highlights
    features/page.tsx        # deeper feature dive
    install/page.tsx         # install / getting started
    blog/page.tsx            # post index (reverse-chron)
    blog/[slug]/page.tsx     # single post (generateStaticParams)
    globals.css              # Tailwind entry + base tokens
  components/
    Nav.tsx, Footer.tsx, ThemeToggle.tsx
    Hero.tsx, TerminalWindow.tsx    # terminal-styled hero
    FeatureCard.tsx, CodeBlock.tsx  # Shiki-highlighted, build-time
    PostCard.tsx
  content/
    blog/<slug>.mdx          # blog posts — the static data
  lib/
    site.ts                  # site metadata, nav links, external URLs
  velite.config.ts           # content collection schema → typed data
  next.config.mjs            # Next config (+ Velite build hook)
  tailwind.config.ts
  tsconfig.json
  package.json
```

**Stack:** Next.js (App Router, statically rendered), TypeScript, Tailwind
(v4), **Velite** for the MDX content layer, **Shiki** for build-time syntax
highlighting (no client-side highlighter JS). All pages are SSG.

## Sitemap & pages

- **Home (`/`)** — hero with the tagline ("A bash-compatible shell written in
  Rust"), the install one-liner, and a terminal-styled code sample; 4–6 feature
  highlights (bash-diff verified, near-bash speed, sources a real `~/.bashrc`,
  programmable completion, job control, arrays/expansions); a call-to-action to
  Install and to the GitHub repo.
- **Features (`/features`)** — grouped, deeper coverage (expansions, control
  flow, functions/arrays, job control, completion, the byte-identical
  compatibility harness), each with a real huck snippet.
- **Install (`/install`)** — Homebrew, Debian/Ubuntu `.deb`, `cargo install`,
  and first-run REPL notes. Mirrors the README's install section.
- **Blog (`/blog`, `/blog/[slug]`)** — reverse-chronological index with tags;
  individual post pages rendered from MDX.
- Global **nav** (Home, Features, Install, Blog, GitHub) and **footer**
  (GitHub repo, issue tracker, license).

## Blog content pipeline

Posts are `site/content/blog/<slug>.mdx`. Frontmatter schema (enforced by
Velite):

```yaml
---
title: "Hello, huck"
date: 2026-07-09
summary: "Why build a bash-compatible shell in Rust — and do it with Claude."
tags: ["intro", "process"]
version: null        # optional: the vNN this post is about, or null
draft: false         # optional: omit from the index when true
---
```

`velite.config.ts` declares a `posts` collection with a typed schema
(`title: string`, `date: date`, `summary: string`, `tags: string[]`,
`version: string | null`, `draft: boolean`, `slug`, `body`/compiled MDX,
`readingTime`). At build, Velite compiles the MDX and emits typed data into
`.velite/`; `app/blog/*` import that data directly — no runtime file reads.
MDX lets a post embed the shared `<CodeBlock>` component for huck-vs-bash
comparisons. Adding a post is: drop an `.mdx` file into `content/blog/` and
commit. `draft: true` keeps a post out of the production index.

## Visual design

A **developer-terminal aesthetic**: dark-first with a light toggle, a monospace
display face for headings/code and a clean sans for body, a single accent
color, and a reusable `<TerminalWindow>` hero (traffic-light chrome + prompt).
Code blocks are Shiki-highlighted at build time (no client JS). Fully
responsive; content max-width for readability; wide code/tables scroll inside
their own container. No heavy animation libraries — CSS transitions only.

## Deployment & operations

- **Vercel**: a new project connected to this GitHub repo with **Root Directory
  = `site`**; framework auto-detected as Next.js. Production branch = `main`.
  Every push and PR produces a **preview deployment** whose build is the site's
  gate (a broken build blocks the preview). Merges to `main` publish production.
  Exact dashboard settings are documented in the plan; a `site/vercel.json` is
  added only if a setting can't be expressed in the dashboard.
- **`.gitignore`**: add `site/node_modules`, `site/.next`, `site/.velite`,
  `site/out`.
- **Rust CI unchanged.** No site job in GitHub Actions; Vercel previews cover
  the site. (A paths-filtered `site-build` job can be added later if desired.)

## Seeded content: the intro post

One launch post, `content/blog/hello-huck.mdx` ("Hello, huck"), establishing the
blog's voice and the project's *why*:

- **Understanding a shell in depth** — building a bash-compatible shell forces
  understanding the details most users never see (word splitting, expansion
  order, job control, completion, the parse→expand→execute pipeline).
- **Learning Rust by building something real and demanding.**
- **Observing how Claude handles genuine long-term development** — huck is built
  one numbered iteration at a time (spec → plan → subagent-driven implementation
  → review → PR), now ~270+ iterations deep; the blog is partly a record of how
  that long-horizon, multi-month collaboration actually goes.

The maintainer reviews and edits the draft before it's published.

## Blog authoring workflow (going forward)

Documented in the spec so the process is explicit and repeatable:

1. The maintainer **proposes a topic**.
2. Claude **drafts the post** as an MDX file in `content/blog/` (frontmatter +
   body), with real code samples where relevant.
3. The maintainer **reviews and gives feedback**; Claude revises.
4. On **approval**, the post is committed and shipped via a **new Vercel
   deployment** (merge to `main` → production deploy; or a preview for a final
   look first).

## Testing & verification

- `npm run build` (which runs Velite then `next build`) succeeds with no type
  errors; `next lint` clean.
- Every route renders statically; `generateStaticParams` covers all posts;
  no broken internal links; the intro post renders with working code blocks.
- Local `npm run dev` serves the site; the Vercel preview deploy builds green.

## Rollout

- Spec + plan committed to `main`; a tracking issue (label `enhancement`, and a
  new `website` label) captures the work; implementation on a
  `site-<topic>` branch; a PR to `main` (`Closes #N`) for the maintainer to
  review and merge, after which Vercel publishes production.
