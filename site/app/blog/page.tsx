import type { Metadata } from "next";
import { posts } from "@/.velite";
import { PostCard } from "@/components/PostCard";

export const metadata: Metadata = {
  title: "Blog — huck",
  description: "Updates on building huck, a bash-compatible shell in Rust, in the open.",
};

function visiblePosts() {
  const isProd = process.env.NODE_ENV === "production";
  return posts
    .filter((post) => (isProd ? !post.draft : true))
    .sort((a, b) => (a.date < b.date ? 1 : a.date > b.date ? -1 : 0));
}

export default function BlogIndexPage() {
  const items = visiblePosts();

  return (
    <div className="space-y-10 py-12 sm:py-16">
      <header className="space-y-3">
        <h1 className="text-3xl font-bold tracking-tight text-zinc-900 sm:text-4xl dark:text-zinc-100">
          Updates — building huck in the open
        </h1>
        <p className="max-w-2xl text-lg leading-relaxed text-zinc-600 dark:text-zinc-400">
          Notes on the design, the Rust, and what it&apos;s like building a shell
          almost entirely with Claude, one numbered iteration at a time.
        </p>
      </header>

      {items.length > 0 ? (
        <div className="grid gap-4 sm:grid-cols-2">
          {items.map((post) => (
            <PostCard key={post.slug} post={post} />
          ))}
        </div>
      ) : (
        <p className="text-sm text-zinc-500 dark:text-zinc-400">
          Nothing posted yet — check back soon.
        </p>
      )}
    </div>
  );
}
