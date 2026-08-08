import { TerminalChrome } from "./TerminalChrome";

type TerminalLine = { prompt?: string; text: string };

export function TerminalWindow({
  title,
  lines,
}: {
  title?: string;
  lines: TerminalLine[];
}) {
  return (
    <TerminalChrome title={title}>
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
    </TerminalChrome>
  );
}
