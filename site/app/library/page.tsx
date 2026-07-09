import type { Metadata } from "next";
import { CodeBlock } from "@/components/CodeBlock";
import { site } from "@/lib/site";

export const metadata: Metadata = {
  title: "Library — huck",
  description:
    "huck ships as reusable Rust crates: huck-syntax (a shell-free lexer/parser/AST) and huck-engine (a terminal-free execution core you can embed).",
};

const syntaxExample = `use huck_syntax::lexer::{Lexer, LexerOptions};
use huck_syntax::parser::parse_sequence;

let src = "echo hello | wc -l";
let mut lx = Lexer::new(src, &Default::default(), LexerOptions::default());

// A Sequence is huck's command AST: pipelines, and-or lists, redirections.
let seq = parse_sequence(&mut lx).expect("valid syntax").expect("non-empty");
assert!(!seq.background);`;

const engineRunExample = `use huck_engine::Engine;

let mut e = Engine::new();
e.set_var("NAME", "world");
assert_eq!(e.run(r#"echo "hi $NAME""#), 0); // prints: hi world

// capture() splits stdout, stderr, and the exit code.
let out = e.capture("echo $((6 * 7)); echo done >&2");
assert_eq!(out.stdout, "42\\n");
assert_eq!(out.stderr, "done\\n");
assert_eq!(out.exit_code, 0);`;

const engineExecExample = `use std::time::Duration;

// Stream each line as the script runs...
let mut lines: Vec<String> = Vec::new();
let exit = e.prepare("for i in 1 2 3; do echo $i; done")
    .on_stdout_line(|line| lines.push(line.to_string()))
    .run();
assert_eq!(lines, vec!["1", "2", "3"]);

// ...or run it sandboxed: a scratch cwd, restricted mode, a time budget.
let out = e.prepare(untrusted_script)
    .cwd(scratch_dir)
    .restricted(true)
    .timeout(Duration::from_secs(5))
    .capture();`;

const cargoExample = `[dependencies]
# Not on crates.io yet — pull from git:
huck-syntax = { git = "https://github.com/jdstanhope/huck" }
huck-engine = { git = "https://github.com/jdstanhope/huck" }`;

function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="space-y-4">
      <h2 className="text-xl font-semibold text-zinc-900 dark:text-zinc-100">{title}</h2>
      <div className="space-y-4 text-sm leading-relaxed text-zinc-600 dark:text-zinc-400">
        {children}
      </div>
    </section>
  );
}

export default function LibraryPage() {
  return (
    <div className="space-y-16 py-12 sm:py-16">
      <header className="space-y-3">
        <h1 className="text-3xl font-bold tracking-tight text-zinc-900 sm:text-4xl dark:text-zinc-100">
          huck as a library
        </h1>
        <p className="max-w-2xl text-lg leading-relaxed text-zinc-600 dark:text-zinc-400">
          huck isn&apos;t only a shell you run — it&apos;s built as layered Rust
          crates, and two of them are public libraries you can depend on
          directly. Parse shell syntax, or embed a whole shell, inside your own
          program.
        </p>
      </header>

      <Section title="huck-syntax — the shell-free frontend">
        <p>
          A standalone lexer, command-AST parser, brace expander, and source
          generator, with <em>no</em> dependency on huck&apos;s runtime — so
          it&apos;s a clean base for linters, formatters, and other shell
          tooling. Bytes become a <code>Vec&lt;Token&gt;</code> plus a{" "}
          <code>Word</code> AST; tokens become a <code>Sequence</code> /{" "}
          <code>Command</code> tree; and <code>generate</code> turns a tree back
          into canonical source for a round-trip.
        </p>
        <p>
          The public AST enums (<code>Token</code>, <code>WordPart</code>,{" "}
          <code>Command</code>, <code>ParseError</code>, …) are{" "}
          <code>#[non_exhaustive]</code>, so new variants in future releases
          stay SemVer-compatible — match with a <code>_ =&gt;</code> arm.
        </p>
        <CodeBlock code={syntaxExample} lang="rust" />
      </Section>

      <Section title="huck-engine — the terminal-free execution core">
        <p>
          <code>Engine</code> is the embedding entry point: a persistent shell
          session with no terminal or line-editor attached. Run or capture
          script strings, run files, and get or set variables and positional
          parameters. Shells signal failure through exit codes, so these methods
          return an <code>i32</code> status rather than a <code>Result</code>.
        </p>
        <CodeBlock code={engineRunExample} lang="rust" />
        <p>
          For finer control, <code>Engine::prepare</code> returns an{" "}
          <code>ExecBuilder</code>: feed it stdin, redirect the working
          directory, stream each output line to a callback as it&apos;s written,
          run under <code>restricted</code> mode, or cap execution with a{" "}
          <code>timeout</code> — useful for running untrusted or generated
          scripts.
        </p>
        <CodeBlock code={engineExecExample} lang="rust" />
      </Section>

      <Section title="Adding it to your project">
        <p>
          The crates aren&apos;t published to crates.io yet, so depend on them
          from git. <code>huck-cli</code> (the rustyline-based REPL) layers on
          top of these two and isn&apos;t meant to be embedded — reach for{" "}
          <code>huck-engine</code> to run scripts and <code>huck-syntax</code> to
          parse them.
        </p>
        <CodeBlock code={cargoExample} lang="toml" />
        <p>
          Both crates carry runnable doc examples and{" "}
          <code>examples/</code> programs. Browse the full API surface on{" "}
          <a
            href={site.repo}
            target="_blank"
            rel="noreferrer"
            className="text-accent-dim underline underline-offset-2 hover:text-accent dark:text-accent"
          >
            GitHub
          </a>
          .
        </p>
      </Section>
    </div>
  );
}
