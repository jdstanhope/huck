import * as runtime from "react/jsx-runtime";
import { CodeBlock } from "./CodeBlock";

type MDXComponents = Record<string, React.ComponentType<any>>; // eslint-disable-line @typescript-eslint/no-explicit-any -- MDX passes each component very different prop shapes (e.g. CodeBlock's {code, lang}); a shared map can't be typed more precisely than this.

const useMDX = (code: string) => {
  // Velite compiles each post's MDX body to a self-contained JS module string
  // (its `body` field) that exports a default component function written
  // against the classic JSX runtime. Evaluating that string is how Velite's
  // own docs recommend rendering compiled MDX — there's no untrusted input
  // here, it's build-time output of our own content pipeline. (This project's
  // ESLint config doesn't flag `no-new-func`/`no-implied-eval` here, but the
  // reasoning is recorded in case that changes.)
  const fn = new Function(code);
  return fn({ ...runtime }).default as React.ComponentType<{
    components?: MDXComponents;
  }>;
};

const components: MDXComponents = { CodeBlock };

export function MDXContent({ code }: { code: string }) {
  const Component = useMDX(code);
  // This is a Server Component rendered once at build time (no client state,
  // no re-renders to worry about) — `Component` necessarily varies per post
  // since it's compiled from that post's MDX source.
  // eslint-disable-next-line react-hooks/static-components -- see comment above
  return <Component components={components} />;
}
