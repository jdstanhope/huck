"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { site } from "@/lib/site";
import { ThemeToggle } from "@/components/ThemeToggle";

export function Nav() {
  const pathname = usePathname();

  return (
    <header className="sticky top-0 z-50 border-b border-zinc-200 bg-white/80 backdrop-blur dark:border-zinc-800 dark:bg-zinc-950/80">
      <div className="mx-auto flex max-w-5xl items-center justify-between gap-4 px-4 py-3">
        <Link
          href="/"
          className="font-mono text-lg font-semibold tracking-tight text-zinc-900 dark:text-zinc-100"
        >
          <span className="text-accent-dim dark:text-accent">$</span> {site.name}
        </Link>

        <nav className="flex flex-1 flex-wrap items-center justify-end gap-x-5 gap-y-1 font-mono text-sm">
          {site.nav.map((item) => {
            const isActive =
              item.href === "/" ? pathname === "/" : pathname.startsWith(item.href);
            return (
              <Link
                key={item.href}
                href={item.href}
                aria-current={isActive ? "page" : undefined}
                className={
                  isActive
                    ? "text-zinc-900 dark:text-zinc-100"
                    : "text-zinc-500 transition-colors hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100"
                }
              >
                {item.label}
              </Link>
            );
          })}
          <a
            href={site.repo}
            target="_blank"
            rel="noreferrer"
            className="text-zinc-500 transition-colors hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100"
          >
            GitHub
          </a>
        </nav>

        <ThemeToggle />
      </div>
    </header>
  );
}
