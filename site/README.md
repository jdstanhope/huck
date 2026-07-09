# huck.dev

The marketing site + blog for [huck](https://github.com/jdstanhope/huck), a
bash-compatible shell written in Rust. Built with Next.js 16 (App Router),
React 19, Tailwind CSS v4, and [Velite](https://velite.js.org/) for
MDX content.

## Prerequisites

- Node.js 20+ (this repo was developed against Node 24)
- npm

## Getting started

```bash
npm install
npm run dev
```

`npm run dev` runs Velite (which builds the content collections from
`content/`) and then starts the Next.js dev server. Open
[http://localhost:3000](http://localhost:3000).

## Scripts

| Command | What it does |
| --- | --- |
| `npm run dev` | `velite && next dev` — builds content, then starts the dev server with hot reload |
| `npm run build` | `velite && next build` — builds content, then produces the production build (static export of every route) |
| `npm run start` | Serves the production build (`next start`) |
| `npm run lint` | Runs ESLint (`eslint-config-next`) |
| `npm run velite` | Runs the Velite content build on its own |

Code syntax highlighting (`rehype-pretty-code` + `shiki`) runs at build
time inside async server components — there is no client-side highlighter
shipped to the browser.

## Content model

Blog posts live under `content/blog/` as MDX files, one per post, named
`<slug>.mdx`. The route `/blog/<slug>` is generated from the filename.

Each post needs frontmatter:

```yaml
---
title: "Post title"
date: 2026-07-09
summary: "One or two sentences shown on the /blog index."
tags: ["intro", "process"]
version: null       # or a huck version as a string, e.g. "276", when relevant
draft: false        # true hides the post from the built site
---
```

The schema is enforced by `velite.config.ts` — Velite will fail the build
if a post's frontmatter doesn't match.

To add a post: drop a new `content/blog/<slug>.mdx` file with valid
frontmatter and MDX body, then run `npm run dev` (or `npm run build`) to
pick it up. No code changes are needed for a new post.

## Blog authoring workflow

Going forward, posts are written collaboratively:

1. The maintainer proposes a topic (e.g. "write up the vNN iteration" or
   "explain why huck exists").
2. Claude drafts the post as MDX under `content/blog/`, following the
   frontmatter schema above.
3. The maintainer reviews the draft and gives feedback; Claude revises.
4. On approval, the post is committed to a branch and merged to `main`.
   Merging to `main` triggers a new Vercel production deployment, which
   publishes the post live.

See `DEPLOY.md` for how deployments work.
