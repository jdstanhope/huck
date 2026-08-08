/**
 * The macOS-style window frame: rounded border, a title bar with the three
 * traffic-light dots, and a body slot.
 *
 * One owner for the chrome, shared by `TerminalWindow` (hand-written transcript
 * lines on the marketing pages) and `MdxPre` (highlighted shell fences in blog
 * posts). The two differ only in what goes in the body.
 */
export function TerminalChrome({
  title,
  children,
}: {
  title?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="overflow-hidden rounded-lg border border-zinc-200 bg-zinc-50 shadow-sm dark:border-zinc-800 dark:bg-zinc-900">
      <div className="flex items-center gap-2 border-b border-zinc-200 bg-zinc-100 px-4 py-2 dark:border-zinc-800 dark:bg-zinc-900/80">
        <span className="size-2.5 rounded-full bg-red-400/80" />
        <span className="size-2.5 rounded-full bg-yellow-400/80" />
        <span className="size-2.5 rounded-full bg-green-400/80" />
        {title ? (
          <span className="ml-2 font-mono text-xs text-zinc-500 dark:text-zinc-400">
            {title}
          </span>
        ) : null}
      </div>
      {children}
    </div>
  );
}
