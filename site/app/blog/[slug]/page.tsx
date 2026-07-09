import type { Metadata } from "next";
import { notFound } from "next/navigation";
import { posts } from "@/.velite";
import { MDXContent } from "@/components/MDXContent";

function findPost(slug: string) {
  const isProd = process.env.NODE_ENV === "production";
  return posts.find((post) => post.slug === slug && (isProd ? !post.draft : true));
}

export function generateStaticParams() {
  return posts.filter((post) => !post.draft).map((post) => ({ slug: post.slug }));
}

export async function generateMetadata({
  params,
}: {
  params: Promise<{ slug: string }>;
}): Promise<Metadata> {
  const { slug } = await params;
  const post = findPost(slug);
  if (!post) return {};

  return {
    title: `${post.title} — huck`,
    description: post.summary,
  };
}

export default async function BlogPostPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  const { slug } = await params;
  const post = findPost(slug);
  if (!post) notFound();

  const date = new Date(post.date).toLocaleDateString("en-US", {
    year: "numeric",
    month: "long",
    day: "numeric",
  });

  return (
    <article className="mx-auto max-w-3xl space-y-8 py-12 sm:py-16">
      <header className="space-y-3">
        <div className="font-mono text-xs text-zinc-500 dark:text-zinc-400">
          <time dateTime={post.date}>{date}</time>
        </div>
        <h1 className="text-3xl font-bold tracking-tight text-zinc-900 sm:text-4xl dark:text-zinc-100">
          {post.title}
        </h1>
        {post.tags.length > 0 ? (
          <ul className="flex flex-wrap gap-2 font-mono text-xs">
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
      </header>

      <div className="prose prose-zinc max-w-none dark:prose-invert prose-pre:p-0 prose-pre:bg-transparent prose-pre:border-0">
        <MDXContent code={post.body} />
      </div>
    </article>
  );
}
