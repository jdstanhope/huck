type TerminalLine = { prompt?: string; text: string };

export function TerminalWindow({
  title,
  lines,
}: {
  title?: string;
  lines: TerminalLine[];
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
      <div className="overflow-x-auto px-4 py-4">
        <pre className="font-mono text-sm leading-relaxed">
          {lines.map((line, i) => (
            <div key={i} className="whitespace-pre">
              {line.prompt ? (
                <span className="text-accent-dim dark:text-accent">{line.prompt} </span>
              ) : null}
              <span className="text-zinc-800 dark:text-zinc-200">{line.text}</span>
            </div>
          ))}
        </pre>
      </div>
    </div>
  );
}
