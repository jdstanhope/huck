import Link from "next/link";
import type { Post } from "@/.velite";

export function PostCard({ post }: { post: Post }) {
  const date = new Date(post.date).toLocaleDateString("en-US", {
    year: "numeric",
    month: "long",
    day: "numeric",
  });

  return (
    <article className="rounded-lg border border-zinc-200 p-5 transition-colors hover:border-zinc-300 dark:border-zinc-800 dark:hover:border-zinc-700">
      <div className="font-mono text-xs text-zinc-500 dark:text-zinc-400">
        <time dateTime={post.date}>{date}</time>
      </div>
      <h3 className="mt-1 text-lg font-semibold text-zinc-900 dark:text-zinc-100">
        <Link href={post.url} className="hover:underline">
          {post.title}
        </Link>
      </h3>
      <p className="mt-2 text-sm leading-relaxed text-zinc-600 dark:text-zinc-400">
        {post.summary}
      </p>
      {post.tags.length > 0 ? (
        <ul className="mt-3 flex flex-wrap gap-2 font-mono text-xs">
          {post.tags.map((tag) => (
            <li
              key={tag}
              className="rounded-full border border-zinc-200 px-2 py-0.5 text-zinc-500 dark:border-zinc-800 dark:text-zinc-400"
            >
              #{tag}
            </li>
          ))}
        </ul>
      ) : null}
    </article>
  );
}
