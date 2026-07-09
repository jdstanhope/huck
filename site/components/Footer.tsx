import Link from "next/link";
import { site } from "@/lib/site";

export function Footer() {
  return (
    <footer className="border-t border-zinc-200 dark:border-zinc-800">
      <div className="mx-auto flex max-w-5xl flex-col gap-3 px-4 py-8 font-mono text-sm text-zinc-500 dark:text-zinc-400 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex flex-wrap gap-x-5 gap-y-1">
          <a
            href={site.repo}
            target="_blank"
            rel="noreferrer"
            className="transition-colors hover:text-zinc-900 dark:hover:text-zinc-100"
          >
            Repo
          </a>
          <a
            href={site.issues}
            target="_blank"
            rel="noreferrer"
            className="transition-colors hover:text-zinc-900 dark:hover:text-zinc-100"
          >
            Issues
          </a>
          <span>MIT License</span>
        </div>
        <p>
          Built with huck&apos;s own dev workflow — read the{" "}
          <Link href="/blog" className="text-accent-dim underline underline-offset-4 dark:text-accent">
            blog
          </Link>
          .
        </p>
      </div>
    </footer>
  );
}
