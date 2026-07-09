import Link from "next/link";
import { TerminalWindow } from "@/components/TerminalWindow";
import { CodeBlock } from "@/components/CodeBlock";
import { FeatureCard } from "@/components/FeatureCard";
import { site } from "@/lib/site";

const demoLines = [
  { prompt: "$", text: "huck" },
  { prompt: "huck$", text: 'for f in *.rs; do echo "${f%.rs}"; done' },
  { prompt: "huck$", text: "name=(alice bob); echo \"${name[@]^}\"" },
  { text: "Alice Bob" },
  {
    prompt: "huck$",
    text: "diff <(huck -c 'echo ${x:-hi}') <(bash -c 'echo ${x:-hi}') && echo identical",
  },
  { text: "identical" },
];

const sumExample = `nums=(3 1 4 1 5 9)
total=0
for n in "\${nums[@]}"; do
  (( total += n ))
done
echo "sum: $total"`;

const whyHuck = [
  {
    title: "Byte-identical, bash-diff verified",
    body: "Every feature ships with a bash-diff harness that runs the same fragment through both shells and asserts identical output — not just \"looks right\".",
  },
  {
    title: "Near-bash speed",
    body: "Command-substitution-heavy scripts run at near-bash speed: each $() Shell clone is O(1) via copy-on-write, so nvm-heavy startup files stay fast.",
  },
  {
    title: "Sources a real ~/.bashrc",
    body: "huck loads bash-completion, a git prompt, nvm, and mise activation without errors, and drives interactive tab completion against the system bash-completion package.",
  },
];

const features = [
  {
    title: "Expansions",
    body: "Parameter expansion with the full modifier set (${v:-w}, ${v/p/r}, ${v^^}, ${v@Q}), arithmetic $((…)), command substitution $(…) / `…`, brace expansion, tilde, and pathname globbing including extglob.",
  },
  {
    title: "Control flow & functions",
    body: "if/elif/else, while/until, for (word-list, C-style, \"$@\"), select, and case, plus functions in name() or function name form with local scoping and the [[ … ]] extended test.",
  },
  {
    title: "Variables & arrays",
    body: "Scalars, indexed arrays, and associative arrays (declare -A), with integer, readonly, and export attributes, declare -g, and printf -v.",
  },
  {
    title: "Job control",
    body: "Foreground and background process groups with terminal handoff, so vim, less, and Ctrl-Z all work, plus jobs/fg/bg/wait/kill/disown with the full %N job-spec syntax.",
  },
  {
    title: "Line editing, history & completion",
    body: "A line editor with persisted history, bash-style history expansion (!!, !$, …), and programmable tab completion that drives the system bash-completion framework.",
  },
  {
    title: "Builtins & options",
    body: "cd, printf, read, test/[, [[, export, declare/typeset, set (-e/-u/-x/-o pipefail/…), shopt, trap, alias, and the rest of the builtins real scripts depend on.",
  },
];

export default function Home() {
  return (
    <div className="space-y-20 py-12 sm:py-16">
      <section className="space-y-6">
        <h1 className="text-3xl font-bold tracking-tight text-zinc-900 sm:text-4xl dark:text-zinc-100">
          {site.tagline}
        </h1>
        <p className="max-w-2xl text-lg leading-relaxed text-zinc-600 dark:text-zinc-400">
          {site.description}
        </p>
        <div className="flex flex-wrap gap-3 pt-2">
          <Link
            href="/install"
            className="rounded-md bg-accent-dim px-5 py-2.5 font-mono text-sm font-semibold text-zinc-950 transition-opacity hover:opacity-90 dark:bg-accent"
          >
            Install huck
          </Link>
          <a
            href={site.repo}
            target="_blank"
            rel="noreferrer"
            className="rounded-md border border-zinc-300 px-5 py-2.5 font-mono text-sm font-semibold text-zinc-700 transition-colors hover:border-zinc-400 hover:text-zinc-900 dark:border-zinc-700 dark:text-zinc-300 dark:hover:border-zinc-600 dark:hover:text-zinc-100"
          >
            View on GitHub
          </a>
        </div>
      </section>

      <section>
        <TerminalWindow title="huck" lines={demoLines} />
      </section>

      <section className="grid gap-6 sm:grid-cols-3">
        {whyHuck.map((item) => (
          <div key={item.title} className="space-y-2">
            <h2 className="font-mono text-sm font-semibold text-accent-dim dark:text-accent">
              {item.title}
            </h2>
            <p className="text-sm leading-relaxed text-zinc-600 dark:text-zinc-400">
              {item.body}
            </p>
          </div>
        ))}
      </section>

      <section className="space-y-6">
        <h2 className="text-xl font-semibold text-zinc-900 dark:text-zinc-100">
          What huck supports
        </h2>
        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {features.map((f) => (
            <FeatureCard key={f.title} title={f.title}>
              {f.body}
            </FeatureCard>
          ))}
        </div>
        <p className="text-sm text-zinc-500 dark:text-zinc-400">
          See the full breakdown, with examples for every group, on the{" "}
          <Link href="/features" className="text-accent-dim underline underline-offset-2 dark:text-accent">
            features page
          </Link>
          .
        </p>
      </section>

      <section className="space-y-4">
        <h2 className="text-xl font-semibold text-zinc-900 dark:text-zinc-100">
          Real arrays, real arithmetic
        </h2>
        <CodeBlock code={sumExample} />
      </section>
    </div>
  );
}
