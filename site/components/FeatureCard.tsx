import type { ReactNode } from "react";

export function FeatureCard({
  title,
  icon,
  children,
}: {
  title: string;
  icon?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="rounded-lg border border-zinc-200 bg-white p-5 dark:border-zinc-800 dark:bg-zinc-900/50">
      <h3 className="flex items-center gap-2 font-mono text-base font-semibold text-zinc-900 dark:text-zinc-100">
        <span className="text-accent-dim dark:text-accent" aria-hidden="true">
          {icon ?? "›"}
        </span>
        {title}
      </h3>
      <div className="mt-2 text-sm leading-relaxed text-zinc-600 dark:text-zinc-400">
        {children}
      </div>
    </div>
  );
}
