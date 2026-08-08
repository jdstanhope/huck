import { TerminalChrome } from "./TerminalChrome";

/**
 * Renders a fenced code block from a post.
 *
 * Shell fences get the terminal frame — the posts are overwhelmingly shell
 * transcripts, and the `# before` / `# after` pairs read as sessions. Anything
 * else (Rust, config) gets the same card WITHOUT the traffic lights: a Rust
 * snippet in a fake terminal window would be claiming something untrue about
 * where that code runs.
 *
 * `rehype-pretty-code` puts the language on the `<pre>` as `data-language`, so
 * the decision is per-fence and needs nothing from the author.
 */
const TERMINAL_LANGS = new Set(["bash", "sh", "shell", "zsh", "console", "shell-session"]);

// The post page's prose wrapper already zeroes `pre` padding/background/border
// (`prose-pre:p-0` etc.), so the frame here is the only thing styling them.
const BODY = "overflow-x-auto px-4 py-3.5 font-mono text-sm leading-relaxed";

export function MdxPre({ children, ...props }: React.ComponentProps<"pre">) {
  const lang = (props as Record<string, unknown>)["data-language"];
  const pre = (
    <pre {...props} className={BODY}>
      {children}
    </pre>
  );

  if (typeof lang === "string" && TERMINAL_LANGS.has(lang)) {
    return <TerminalChrome title={lang}>{pre}</TerminalChrome>;
  }

  return (
    <div className="overflow-hidden rounded-lg border border-zinc-200 bg-zinc-50 shadow-sm dark:border-zinc-800 dark:bg-zinc-900">
      {pre}
    </div>
  );
}
