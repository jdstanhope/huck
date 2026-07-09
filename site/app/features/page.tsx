import type { Metadata } from "next";
import { CodeBlock } from "@/components/CodeBlock";
import { TerminalWindow } from "@/components/TerminalWindow";

export const metadata: Metadata = {
  title: "Features — huck",
  description:
    "What huck supports: command syntax, expansions, control flow, arrays, job control, line editing, and the builtins real scripts depend on — verified byte-for-byte against bash.",
};

const commandSyntaxExample = `mkdir -p dist && cp *.rs dist/ || echo "nothing to copy"
cat <<EOF > dist/notes.txt
built at $(date +%F)
EOF
wc -l < dist/notes.txt`;

const expansionsExample = `name=world
echo "\${name^} says: \${name:0:3}"
echo {1..5}
echo $((2**10))`;

const controlFlowExample = `greet() {
  local who=\${1:-world}
  echo "hello, $who"
}
greet huck`;

const arraysExample = `declare -A colors=([sky]=blue [grass]=green)
for k in "\${!colors[@]}"; do
  echo "$k -> \${colors[$k]}"
done`;

const jobControlLines = [
  { prompt: "huck$", text: "sleep 30 &" },
  { text: "[1] 20481" },
  { prompt: "huck$", text: "jobs" },
  { text: "[1]+ Running                 sleep 30 &" },
  { prompt: "huck$", text: "fg %1" },
  { text: "sleep 30" },
];

const jobControlExample = `sleep 30 &
job_pid=$!
kill "$job_pid"
wait "$job_pid" 2>/dev/null
echo "job $job_pid stopped"`;

const historyExample = `huck$ echo building the site
building the site
huck$ !!
echo building the site
building the site
huck$ !echo:s/building/deploying/
echo deploying the site
deploying the site`;

const builtinsExample = `set -e
trap 'echo "cleaning up"' EXIT
declare -ri max=10
getopts ":v" opt || true
echo "max is $max"`;

const verifiedExample = `# tests/scripts/expansion_diff_check.sh (shape)
frag='name=world; echo "\${name^} says: \${name:0:3}"; echo {1..5}; echo $((2**10))'
diff <(bash -c "$frag") <(huck -c "$frag") && echo "byte-identical"`;

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

export default function FeaturesPage() {
  return (
    <div className="space-y-16 py-12 sm:py-16">
      <header className="space-y-3">
        <h1 className="text-3xl font-bold tracking-tight text-zinc-900 sm:text-4xl dark:text-zinc-100">
          What huck supports
        </h1>
        <p className="max-w-2xl text-lg leading-relaxed text-zinc-600 dark:text-zinc-400">
          huck implements most of bash&apos;s surface — syntax, expansions, control
          flow, arrays, job control, line editing, and completion — and checks
          every one of these against real bash output.
        </p>
      </header>

      <Section title="Command syntax & operators">
        <p>
          Simple commands and pipelines (<code>a | b</code>); lists with{" "}
          <code>;</code>, <code>&amp;&amp;</code>, <code>||</code>, and{" "}
          <code>&amp;</code> (background, including backgrounding an and-or
          group); grouping with <code>( … )</code> (subshell) and{" "}
          <code>{"{ …; }"}</code> (current shell); redirections{" "}
          <code>&lt;</code>, <code>&gt;</code>, <code>&gt;&gt;</code>,{" "}
          <code>&gt;|</code>, <code>2&gt;</code>, <code>2&gt;&gt;</code>,{" "}
          <code>&amp;&gt;</code>, fd duplication (<code>2&gt;&amp;1</code>),
          here-documents (<code>&lt;&lt;</code>, <code>&lt;&lt;-</code>), and
          here-strings (<code>&lt;&lt;&lt;</code>) — including redirections on
          compound commands. Comments, line continuation, and multi-line input
          all work the way bash does.
        </p>
        <CodeBlock code={commandSyntaxExample} />
      </Section>

      <Section title="Expansions">
        <p>
          Parameter expansion (<code>$VAR</code>, <code>${"{VAR}"}</code>,
          positional and special parameters), the full modifier set (
          <code>${"{v:-w}"}</code>, prefix/suffix strip, substring, pattern
          substitution, case modification, <code>@Q</code>/<code>@P</code>/
          <code>@U</code> transforms, indirection), arithmetic{" "}
          <code>$((…))</code>, command substitution <code>$(…)</code> and{" "}
          <code>`…`</code>, brace expansion, tilde expansion, and pathname
          globbing including POSIX classes and <code>extglob</code>.
        </p>
        <CodeBlock code={expansionsExample} />
      </Section>

      <Section title="Control flow & functions">
        <p>
          <code>if</code>/<code>elif</code>/<code>else</code>,{" "}
          <code>while</code>/<code>until</code>, <code>for</code> (word-list,{" "}
          <code>&quot;$@&quot;</code>, and C-style), <code>select</code>, and{" "}
          <code>case</code> (with <code>;;</code>/<code>;&amp;</code>/
          <code>;;&amp;</code>); <code>break N</code>/<code>continue N</code>.
          Functions in both <code>name() {"{ … }"}</code> and{" "}
          <code>function name {"{ … }"}</code> form, with positional args,{" "}
          <code>local</code>, <code>return</code>, and dynamic scoping. The{" "}
          <code>[[ … ]]</code> extended test supports glob and regex matching
          (<code>=~</code>, populating <code>BASH_REMATCH</code>).
        </p>
        <CodeBlock code={controlFlowExample} />
      </Section>

      <Section title="Variables & arrays">
        <p>
          Scalars, indexed arrays (<code>a=(x y)</code>, <code>a[i]=</code>,{" "}
          <code>a+=</code>, slicing), and associative arrays (
          <code>declare -A</code>), with integer (<code>-i</code>), readonly
          (<code>-r</code>), and export (<code>-x</code>) attributes,{" "}
          <code>declare -g</code>, and <code>printf -v</code>.
        </p>
        <CodeBlock code={arraysExample} />
      </Section>

      <Section title="Job control">
        <p>
          Foreground and background process groups with{" "}
          <code>tcsetpgrp</code> terminal handoff — so <code>vim</code>,{" "}
          <code>less</code>, and Ctrl-Z work — SIGCHLD reaping with{" "}
          <code>[N] Done</code> notices, and the full{" "}
          <code>jobs</code>/<code>fg</code>/<code>bg</code>/<code>wait</code>/
          <code>kill</code>/<code>disown</code> toolkit with{" "}
          <code>%N</code>/<code>%+</code>/<code>%%</code>/<code>%-</code>/
          <code>%cmd</code> job specifiers.
        </p>
        <TerminalWindow lines={jobControlLines} />
        <CodeBlock code={jobControlExample} />
      </Section>

      <Section title="Line editing, history & completion">
        <p>
          A line editor with history persisted to <code>$HISTFILE</code>,
          history expansion (<code>!!</code>, <code>!n</code>,{" "}
          <code>!str</code>, <code>!$</code>, <code>^old^new^</code>), and
          programmable tab completion: command/file/variable completion plus
          the full <code>complete</code>/<code>compgen</code>/
          <code>compopt</code> machinery, which drives the system{" "}
          <code>bash-completion</code> framework.
        </p>
        <CodeBlock code={historyExample} lang="text" />
      </Section>

      <Section title="Builtins & options">
        <p>
          <code>cd</code>, <code>pwd</code>, <code>echo</code>,{" "}
          <code>printf</code> (incl. <code>%q</code>), <code>read</code>,{" "}
          <code>test</code>/<code>[</code>, <code>[[</code>,{" "}
          <code>export</code>, <code>readonly</code>, <code>local</code>,{" "}
          <code>declare</code>/<code>typeset</code>, <code>unset</code>,{" "}
          <code>set</code> (<code>-e</code>/<code>-u</code>/<code>-x</code>/
          <code>-f</code>/<code>-o pipefail</code>/<code>-C</code>),{" "}
          <code>shopt</code>, <code>getopts</code>, <code>eval</code>,{" "}
          <code>command</code>, <code>hash</code>, <code>trap</code>{" "}
          (EXIT/ERR/DEBUG/RETURN + signals), <code>alias</code>/
          <code>unalias</code>, job-control builtins, <code>history</code>,{" "}
          <code>break</code>, <code>continue</code>, <code>return</code>,{" "}
          <code>exit</code>, and <code>complete</code>/<code>compgen</code>/
          <code>compopt</code>.
        </p>
        <CodeBlock code={builtinsExample} />
      </Section>

      <Section title="Verified against bash">
        <p>
          Every feature ships with a design spec, an implementation plan, a
          test suite, and a byte-identical bash-diff harness under{" "}
          <code>tests/scripts/*_diff_check.sh</code>: the same shell fragment
          runs through both bash and huck, and the harness asserts the two
          outputs are byte-for-byte identical. As of this writing that&apos;s
          around 3,400 unit/integration tests and 160 bash-diff harnesses, all
          green.
        </p>
        <CodeBlock code={verifiedExample} />
      </Section>
    </div>
  );
}
