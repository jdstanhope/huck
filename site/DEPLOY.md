# Deploying huck.dev (Vercel)

The site is deployed on [Vercel](https://vercel.com), directly from the
`jdstanhope/huck` GitHub repository. The Next.js app lives in the `site/`
subdirectory of that repo (a monorepo alongside the Rust shell), so the
one setting that matters is **Root Directory**.

## One-time project setup

In the Vercel dashboard:

1. **New Project** → **Import Git Repository** → select `jdstanhope/huck`.
2. **Root Directory**: set to `site` (Vercel builds only this
   subdirectory; the Rust crates at the repo root are ignored).
3. **Framework Preset**: `Next.js` — auto-detected once the root
   directory is set.
4. **Build Command**: leave the default (`npm run build`, which itself
   runs `velite && next build` per `site/package.json`).
5. **Output Directory**: leave the default (Next.js's own output — do not
   override).
6. **Install Command**: leave the default (`npm install`).
7. **Production Branch**: `main`.

Click **Deploy**. Vercel will build once to confirm the configuration.

## Ongoing workflow

Once the project is connected, no further Vercel configuration is
needed:

- **Every push and every pull request** against `jdstanhope/huck` that
  touches `site/` produces a **preview deployment** with its own URL.
  This preview build is effectively the site's CI: if `npm run build`
  fails (Velite content-schema error, TypeScript error, etc.), the
  preview fails and that's the signal something is broken — check it
  before merging.
- **Merging to `main`** triggers a production deployment, which publishes
  to the live domain.

There is no separate CI workflow file for the site; the Vercel preview
build is the gate.

## Node version

Vercel's default build image uses a recent Node LTS, which already
satisfies this project's Node 20+ requirement. `site/package.json` also
pins it explicitly for reproducibility:

```json
"engines": {
  "node": ">=20"
}
```

Vercel reads `engines.node` from `package.json` and uses a matching Node
version for the build.
