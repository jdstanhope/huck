import type { Metadata } from "next";
import { CodeBlock } from "@/components/CodeBlock";
import { TerminalWindow } from "@/components/TerminalWindow";

export const metadata: Metadata = {
  title: "Install — huck",
  description:
    "Install huck via Homebrew, a Debian/Ubuntu .deb, cargo install, or from source.",
};

const homebrewExample = `brew install jdstanhope/huck/huck`;

const debianExample = `curl -fsSL https://raw.githubusercontent.com/jdstanhope/huck/main/scripts/install.sh | sh
# or, manually:
sudo apt install ./huck_<version>_<arch>.deb`;

const cargoInstallExample = `cargo install --git https://github.com/jdstanhope/huck huck`;

const fromSourceExample = `cargo build --release
cargo run                # interactive REPL`;

const firstRunLines = [
  { prompt: "$", text: "huck" },
  { prompt: "huck>", text: "echo hello from huck" },
  { text: "hello from huck" },
  { prompt: "huck>", text: "exit" },
];

function Method({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="space-y-3">
      <h2 className="text-lg font-semibold text-zinc-900 dark:text-zinc-100">{title}</h2>
      {children}
    </section>
  );
}

export default function InstallPage() {
  return (
    <div className="space-y-14 py-12 sm:py-16">
      <header className="space-y-3">
        <h1 className="text-3xl font-bold tracking-tight text-zinc-900 sm:text-4xl dark:text-zinc-100">
          Install huck
        </h1>
        <p className="max-w-2xl text-lg leading-relaxed text-zinc-600 dark:text-zinc-400">
          Pick whichever fits your platform. All methods produce the same{" "}
          <code>huck</code> binary.
        </p>
      </header>

      <div className="space-y-10">
        <Method title="Homebrew (macOS/Linux)">
          <CodeBlock code={homebrewExample} />
        </Method>

        <Method title="Debian/Ubuntu (.deb)">
          <CodeBlock code={debianExample} />
        </Method>

        <Method title="cargo install">
          <CodeBlock code={cargoInstallExample} />
        </Method>

        <Method title="From source">
          <p className="text-sm leading-relaxed text-zinc-600 dark:text-zinc-400">
            Clone the repo and build it with a stable Rust toolchain:
          </p>
          <CodeBlock code={fromSourceExample} />
        </Method>
      </div>

      <section className="space-y-3">
        <h2 className="text-lg font-semibold text-zinc-900 dark:text-zinc-100">
          First run
        </h2>
        <p className="text-sm leading-relaxed text-zinc-600 dark:text-zinc-400">
          Running <code>huck</code> with no arguments drops you into an
          interactive REPL — a line editor with history and tab completion,
          just like bash. It sources your existing <code>~/.bashrc</code>-class
          startup files, so your prompt, aliases, and completions carry over.
        </p>
        <TerminalWindow title="huck" lines={firstRunLines} />
      </section>
    </div>
  );
}
