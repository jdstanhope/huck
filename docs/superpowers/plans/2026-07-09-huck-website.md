# huck Website Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a static marketing + blog website for the huck shell in `site/`, using Next.js (App Router) + TypeScript + Tailwind, a Velite MDX content layer for the blog, and Shiki build-time code highlighting; deploy on Vercel. Resolves [#88](https://github.com/jdstanhope/huck/issues/88).

**Architecture:** A self-contained Next.js app in `site/`, isolated from the Rust workspace (Cargo and the Rust CI never touch it). All pages are statically rendered (SSG). Blog posts are MDX files in `site/content/blog/`; Velite compiles them to typed data at build. Shiki (via `rehype-pretty-code` for MDX and a small async helper for standalone snippets) highlights code at build time — no client-side highlighter. Vercel preview deployments are the site's CI.

**Tech Stack:** Next.js 15 (App Router), React 19, TypeScript, Tailwind v4, Velite (content layer), Shiki + rehype-pretty-code (highlighting), next-themes (dark/light toggle), next/font. Node 24 / npm 11 (verified available locally).

## Global Constraints

- The site lives entirely in `site/`. Do NOT modify the Rust workspace, `Cargo.*`, `crates/`, or `.github/workflows/ci.yml`.
- All routes are statically rendered (SSG). No API routes, no server-at-request-time, no database, no auth.
- Dark-first "developer terminal" aesthetic with a working light/dark toggle; fully responsive; wide code/tables scroll inside their own container (page body never scrolls horizontally).
- Code is highlighted at BUILD time (Shiki). No client-side syntax highlighting library.
- Vercel preview deploys are the site's CI. Do NOT add a GitHub Actions job for the site.
- Every task's build gate is `npm run build` (which runs `velite` then `next build`) succeeding from inside `site/` with no type errors.
- Run all npm commands from inside `site/`. Node 24 / npm 11.
- Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Work on branch `site-huck-website`. Do NOT push to `main`, do NOT self-merge; the iteration lands via a PR (`Closes #N`) the maintainer reviews.

---

### Task 1: Scaffold the Next.js app and get a green build

**Files:**
- Create: the `site/` app via `create-next-app`, then add `site/velite.config.ts`, edit `site/next.config.ts`→`site/next.config.mjs`, `site/package.json` (scripts), `site/tsconfig.json` (path), `site/.gitignore` (append), `site/app/globals.css` (theme tokens), `site/app/layout.tsx` + `site/app/page.tsx` (minimal).

**Interfaces:**
- Produces: a buildable Next.js app in `site/`; the Velite `posts` collection typed export importable as `import { posts } from "@/.velite"`; npm scripts `dev`/`build`/`lint` that run Velite before Next.

- [ ] **Step 1: Scaffold with create-next-app (non-interactive)**

From the repo root run:

```bash
npx --yes create-next-app@latest site \
  --ts --tailwind --eslint --app --no-src-dir \
  --import-alias "@/*" --use-npm --turbopack --yes
```

This creates `site/` with Next 15 + React 19 + Tailwind v4 + TypeScript, its own `.gitignore` (already ignoring `node_modules`, `.next`), and does NOT re-init git (it detects the parent repo).

- [ ] **Step 2: Install content + highlighting deps**

```bash
cd site
npm install velite rehype-pretty-code shiki next-themes
```

- [ ] **Step 3: Add the Velite config**

Create `site/velite.config.ts`:

```ts
import { defineConfig, defineCollection, s } from "velite";
import rehypePrettyCode from "rehype-pretty-code";

const posts = defineCollection({
  name: "Post",
  pattern: "blog/**/*.mdx",
  schema: s
    .object({
      title: s.string().max(120),
      date: s.isodate(),
      summary: s.string().max(300),
      tags: s.array(s.string()).default([]),
      version: s.string().nullable().default(null),
      draft: s.boolean().default(false),
      slug: s.path(),
      body: s.mdx(),
    })
    .transform((data) => ({
      ...data,
      slug: data.slug.replace(/^blog\//, ""),
      url: `/blog/${data.slug.replace(/^blog\//, "")}`,
    })),
});

export default defineConfig({
  root: "content",
  output: { data: ".velite", assets: "public/static", base: "/static/", clean: true },
  collections: { posts },
  mdx: {
    rehypePlugins: [
      [rehypePrettyCode, { theme: { dark: "github-dark-dimmed", light: "github-light" }, keepBackground: false }],
    ],
  },
});
```

- [ ] **Step 4: Wire Velite into the build + scripts**

Rename `site/next.config.ts` to `site/next.config.mjs` with:

```js
/** @type {import('next').NextConfig} */
const nextConfig = {
  // Velite writes ./.velite before next build/dev via the npm scripts below.
};
export default nextConfig;
```

Edit `site/package.json` `scripts` to run Velite first:

```json
{
  "scripts": {
    "dev": "velite && next dev",
    "build": "velite && next build",
    "start": "next start",
    "lint": "next lint",
    "velite": "velite"
  }
}
```

- [ ] **Step 5: tsconfig path + gitignore**

Confirm `site/tsconfig.json` has `"paths": { "@/*": ["./*"] }` (create-next-app sets this). Append to `site/.gitignore`:

```
# velite content output
.velite
```

Also append to the REPO ROOT `/home/john/projects/huck/.gitignore`:

```
# huck website (Next.js app)
site/node_modules
site/.next
site/.velite
site/out
```

- [ ] **Step 6: Theme tokens in globals.css**

Replace `site/app/globals.css` with Tailwind v4 entry + terminal theme tokens (dark-first):

```css
@import "tailwindcss";

@theme {
  --font-sans: var(--font-inter), ui-sans-serif, system-ui, sans-serif;
  --font-mono: var(--font-jetbrains), ui-monospace, "SFMono-Regular", monospace;
  --color-accent: #7ee787;      /* terminal green */
  --color-accent-dim: #3fb950;
}

:root { color-scheme: light dark; }

html { scroll-behavior: smooth; }
body { @apply bg-white text-zinc-900 antialiased; }
.dark body { @apply bg-zinc-950 text-zinc-100; }
```

- [ ] **Step 7: Minimal layout + home placeholder**

Replace `site/app/layout.tsx`:

```tsx
import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "huck — a bash-compatible shell in Rust",
  description: "A bash-compatible shell written in Rust, verified byte-for-byte against real bash.",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body>{children}</body>
    </html>
  );
}
```

Replace `site/app/page.tsx`:

```tsx
export default function Home() {
  return <main className="p-8"><h1 className="text-2xl font-bold">huck</h1></main>;
}
```

- [ ] **Step 8: Build to verify**

Run: `cd site && npm run build`
Expected: Velite runs (creating `.velite/`), then `next build` completes with no type errors and lists the `/` route as static. If `.velite` import types are needed by later tasks, run `npx velite` once so `.velite/` exists for the editor.

- [ ] **Step 9: Commit**

```bash
cd /home/john/projects/huck
git add site .gitignore
git commit -m "$(printf 'site: scaffold Next.js app + Velite content pipeline\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: Design system and shared components

**Files:**
- Create: `site/lib/site.ts`, `site/components/{Nav,Footer,ThemeToggle,ThemeProvider,TerminalWindow,CodeBlock,FeatureCard,PostCard}.tsx`.
- Modify: `site/app/layout.tsx` (fonts, ThemeProvider, Nav, Footer).

**Interfaces:**
- Consumes: `posts` type (for `PostCard`), Tailwind theme tokens from Task 1.
- Produces: `<Nav/>`, `<Footer/>`, `<ThemeToggle/>`, `<ThemeProvider/>`, `<TerminalWindow title? prompt? lines>`, `async <CodeBlock code lang>`, `<FeatureCard title icon? children>`, `<PostCard post>` where `post` has `{ title, date, summary, tags, url, readingTime? }`.

- [ ] **Step 1: Site metadata + nav data**

Create `site/lib/site.ts`:

```ts
export const site = {
  name: "huck",
  tagline: "A bash-compatible shell written in Rust.",
  description:
    "huck implements most of bash's surface — expansions, control flow, functions, arrays, job control, line editing, completion — and verifies it byte-for-byte against real bash.",
  repo: "https://github.com/jdstanhope/huck",
  issues: "https://github.com/jdstanhope/huck/issues",
  nav: [
    { href: "/", label: "Home" },
    { href: "/features", label: "Features" },
    { href: "/install", label: "Install" },
    { href: "/blog", label: "Blog" },
  ],
};
```

- [ ] **Step 2: Theme provider + toggle (next-themes)**

Create `site/components/ThemeProvider.tsx`:

```tsx
"use client";
import { ThemeProvider as NextThemes } from "next-themes";
export function ThemeProvider({ children }: { children: React.ReactNode }) {
  return <NextThemes attribute="class" defaultTheme="dark" enableSystem>{children}</NextThemes>;
}
```

Create `site/components/ThemeToggle.tsx` — a `"use client"` button that reads `useTheme()` and toggles between `"light"` and `"dark"`, guarded against hydration mismatch (render nothing until mounted). Use a sun/moon glyph (inline SVG or unicode ☀/☾). Accessible `aria-label`.

- [ ] **Step 3: Nav + Footer**

Create `site/components/Nav.tsx` — a sticky top bar: the `huck` wordmark (mono, accent-colored `$` prefix) linking home, the `site.nav` links (highlight active route via `usePathname`), a GitHub link, and `<ThemeToggle/>`. Mobile: collapse links into a simple menu or wrap. Use `next/link`.

Create `site/components/Footer.tsx` — repo link, issues link, license (MIT), and a short "built with huck's own dev workflow" line linking `/blog`.

- [ ] **Step 4: TerminalWindow + CodeBlock**

Create `site/components/TerminalWindow.tsx` — a presentational card with terminal chrome (three traffic-light dots + optional `title`), monospace body, that renders an array of `{ prompt?: string; text: string }` lines (prompt shown in accent color). Props: `{ title?: string; lines: { prompt?: string; text: string }[] }`.

Create `site/components/CodeBlock.tsx` — an **async server component** that highlights a standalone snippet at build time with Shiki:

```tsx
import { codeToHtml } from "shiki";

export async function CodeBlock({ code, lang = "bash" }: { code: string; lang?: string }) {
  const html = await codeToHtml(code.trim(), {
    lang,
    themes: { dark: "github-dark-dimmed", light: "github-light" },
    defaultColor: false,
  });
  return (
    <div
      className="overflow-x-auto rounded-lg border border-zinc-200 dark:border-zinc-800 text-sm [&_pre]:p-4"
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}
```

(Shiki dual-theme emits both; add the small CSS in `globals.css` to select by `.dark` — include this rule in this step's globals edit: `.dark .shiki, .dark .shiki span { color: var(--shiki-dark) !important; background: var(--shiki-dark-bg) !important; }` and the light equivalent using `--shiki-light`.)

- [ ] **Step 5: FeatureCard + PostCard**

Create `site/components/FeatureCard.tsx` — `{ title: string; children: React.ReactNode }`: a bordered card, mono title with an accent marker, body text.

Create `site/components/PostCard.tsx` — takes one Velite post; shows date (formatted), title (link to `post.url`), summary, and tags. Import the post type from `@/.velite`.

- [ ] **Step 6: Fonts + wire layout**

In `site/app/layout.tsx`, load `Inter` and `JetBrains_Mono` via `next/font/google` (assign CSS vars `--font-inter`, `--font-jetbrains`), wrap children in `<ThemeProvider>`, and render `<Nav/>` above and `<Footer/>` below a `<main>` container (`max-w-5xl mx-auto px-4`). Add the font vars to `<html className>`.

- [ ] **Step 7: Build + commit**

Run: `cd site && npm run build` (expect clean). Then commit `site/` with message `site: design system + shared components` (+ trailer).

---

### Task 3: Marketing pages (home, features, install)

**Files:**
- Modify: `site/app/page.tsx`. Create: `site/app/features/page.tsx`, `site/app/install/page.tsx`.

**Interfaces:**
- Consumes: `TerminalWindow`, `CodeBlock`, `FeatureCard`, `site` metadata from Task 2.

- [ ] **Step 1: Home page**

Rewrite `site/app/page.tsx` as a static page with: a hero (`site.tagline` as an `<h1>`, a one-line subhead from `site.description`, primary CTA → `/install`, secondary → GitHub); a `<TerminalWindow>` showing a real huck session, e.g. lines:

```
$ huck
huck$ for f in *.rs; do echo "${f%.rs}"; done
huck$ name=(alice bob); echo "${name[@]^}"
Alice Bob
huck$ diff <(huck -c 'echo ${x:-hi}') <(bash -c 'echo ${x:-hi}') && echo identical
identical
```

Then a "Why huck" strip (3 short claims: byte-identical bash-diff verified; near-bash speed; sources a real `~/.bashrc`) and a 6-item `<FeatureCard>` grid summarizing: Expansions, Control flow & functions, Variables & arrays, Job control, Line editing/history/completion, Builtins & options — each 1–2 sentences drawn from `README.md`'s "What huck supports". Include one `<CodeBlock>` sample.

- [ ] **Step 2: Features page**

Create `site/app/features/page.tsx` — grouped sections mirroring the README's "What huck supports": Command syntax & operators, Expansions, Control flow & functions, Variables & arrays, Job control, Line editing/history/completion, Builtins & options, and a "Verified against bash" section explaining the byte-identical `*_diff_check.sh` harness. Each section: a heading, prose from the README, and at least one `<CodeBlock>` huck example. Add page `metadata` (title/description).

- [ ] **Step 3: Install page**

Create `site/app/install/page.tsx` — the three install methods from `README.md` as `<CodeBlock>`s (Homebrew `brew install jdstanhope/huck/huck`; the Debian/Ubuntu `install.sh` curl one-liner + `apt install ./huck_<version>_<arch>.deb`; `cargo install --git https://github.com/jdstanhope/huck huck`) plus a "From source" block (`cargo build --release`, `cargo run`) and a "First run" note (interactive REPL). Add page `metadata`.

- [ ] **Step 4: Build + commit**

Run: `cd site && npm run build` (expect all three routes static, clean). Commit `site/` with `site: marketing pages (home, features, install)` (+ trailer).

---

### Task 4: Blog pipeline + the intro post

**Files:**
- Create: `site/components/MDXContent.tsx`, `site/app/blog/page.tsx`, `site/app/blog/[slug]/page.tsx`, `site/content/blog/hello-huck.mdx`.

**Interfaces:**
- Consumes: `posts` from `@/.velite`; `PostCard`, `CodeBlock` from Task 2.
- Produces: `/blog` index and `/blog/[slug]` pages rendering MDX.

- [ ] **Step 1: MDX runtime component**

Create `site/components/MDXContent.tsx` — renders Velite's compiled MDX `body` string with a component map (so custom components are available in posts):

```tsx
import * as runtime from "react/jsx-runtime";
import { CodeBlock } from "./CodeBlock";

const useMDX = (code: string) => {
  const fn = new Function(code);
  return fn({ ...runtime }).default as React.ComponentType<{ components?: Record<string, React.ComponentType> }>;
};

const components = { CodeBlock };

export function MDXContent({ code }: { code: string }) {
  const Component = useMDX(code);
  return <Component components={components} />;
}
```

- [ ] **Step 2: Blog index**

Create `site/app/blog/page.tsx` — a static page importing `{ posts } from "@/.velite"`, filtering out `draft` (in production), sorting by `date` descending, and rendering a `<PostCard>` per post. Add a short intro heading ("Updates — building huck in the open") and page `metadata`.

- [ ] **Step 3: Single post page**

Create `site/app/blog/[slug]/page.tsx` — `generateStaticParams` from `posts` (map to `{ slug }`), a `generateMetadata` using the post title/summary, look up the post by slug (404 via `notFound()` if missing or draft in production), render title + formatted date + tags + `<MDXContent code={post.body} />`. Constrain content width and apply prose styling (a `.prose`-like set of Tailwind classes or `@tailwindcss/typography` if added — if you add the plugin, `npm install -D @tailwindcss/typography` and register it in `globals.css` via `@plugin "@tailwindcss/typography";`).

- [ ] **Step 4: The intro post**

Create `site/content/blog/hello-huck.mdx` with this content (the maintainer will review/edit before merge):

```mdx
---
title: "Hello, huck"
date: 2026-07-09
summary: "Why build a bash-compatible shell in Rust — and build it as a long-running experiment in working with Claude."
tags: ["intro", "process"]
version: null
draft: false
---

huck is a bash-compatible shell written in Rust. That one sentence hides three
separate reasons the project exists — and this blog is where I'll track how it
goes.

## Understanding a shell, in detail

Everyone uses a shell. Almost no one knows what it actually does. Word
splitting, the order expansions happen in, how `$IFS` interacts with globbing,
what job control really requires from the terminal, how programmable completion
is wired — these are the parts you never see until you try to reproduce them
exactly. The surest way to understand a system is to rebuild it and diff your
output against the original, byte for byte. That's the bar huck holds itself to:
every feature ships with a spec, a plan, tests, and a harness that runs the same
fragment through huck and real bash and asserts identical output.

<CodeBlock lang="bash" code={`diff <(huck -c 'echo \${x:-hi}') <(bash -c 'echo \${x:-hi}') && echo identical`} />

## Learning Rust by building something demanding

A shell is a great forcing function for a language. It touches parsing, process
management, signals, file descriptors, terminals, and performance all at once.
huck is where I'm learning Rust in the deep end — ownership across a
copy-on-write shell state, careful `unsafe` at the OS boundary, and a
parser/lexer architecture that had to be rebuilt more than once.

## Watching how Claude handles long-term development

The third reason is the most interesting to me. huck is built almost entirely
with Claude, one numbered iteration at a time: brainstorm a design, write a
spec, write an implementation plan, execute it with a fresh agent per task,
review, and merge. It's now more than 270 iterations deep. The open question
isn't "can an AI write a function" — it's whether that workflow holds up across
months of real, cumulative software development: regressions, architecture
changes, tech debt, and all. This blog is partly a logbook of that experiment.

More soon.
```

- [ ] **Step 5: Build + verify + commit**

Run: `cd site && npm run build`. Expected: `/blog` and `/blog/hello-huck` render statically; the code block in the post is highlighted. Commit `site/` with `site: blog pipeline + intro post` (+ trailer).

---

### Task 5: Deployment docs, tracking issue, final verification

**Files:**
- Create: `site/README.md` (dev + deploy notes), `site/DEPLOY.md` (Vercel settings).

**Interfaces:** none (leaf/documentation task).

- [ ] **Step 1: site/README.md**

Create `site/README.md` documenting: prerequisites (Node 20+), `npm install`, `npm run dev` (runs Velite then Next dev), `npm run build`, the content model (add a post = drop `content/blog/<slug>.mdx` + frontmatter), and the blog authoring workflow from the spec (maintainer proposes topic → Claude drafts → review/feedback → approve → publish via a new Vercel deployment).

- [ ] **Step 2: site/DEPLOY.md (Vercel)**

Create `site/DEPLOY.md` with the exact Vercel setup: New Project → import `jdstanhope/huck` → **Root Directory: `site`** → Framework Preset: Next.js (auto) → Build Command `npm run build` (default) → Output default → Production Branch `main`. Note that every push/PR yields a preview deploy (the site's CI) and merges to `main` publish production. Note the required Node version if Vercel needs pinning (Vercel uses a recent default; pin via `"engines": { "node": ">=20" }` in `site/package.json` if desired — add it).

- [ ] **Step 3: Full build + link check**

Run: `cd site && npm run build` and confirm zero errors and that every route (`/`, `/features`, `/install`, `/blog`, `/blog/hello-huck`) appears in the static output. Manually confirm no `<a href>`/`<Link>` points to a non-existent internal route.

- [ ] **Step 4: Commit**

The tracking issue (#88, labels `website` + `enhancement`) already exists; the PR will `Closes #88`. Commit `site/README.md` and `site/DEPLOY.md` with `site: dev + deploy docs` (+ trailer).

---

## Notes for the whole-branch review

- Confirm the site is fully isolated: `git status` shows changes only under `site/` and the root `.gitignore`; no `Cargo.*`/`crates/`/workflow edits.
- Confirm no client-side highlighter shipped (Shiki runs in async server components / Velite build only).
- Confirm `npm run build` is green and all five routes are static (SSG), and that dark/light both render (the Shiki dual-theme CSS selects correctly under `.dark`).
- The intro post copy is the maintainer's to edit before merge — flag any factual claim (iteration count "270+", performance numbers) to confirm against the README/memory.
- Deployment (connecting Vercel, setting Root Directory `site`) is a maintainer action performed in the Vercel dashboard per `site/DEPLOY.md`; it is not automatable from this branch.
