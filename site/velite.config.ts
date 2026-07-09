import { defineConfig, defineCollection, s } from "velite";
import rehypePrettyCode from "rehype-pretty-code";

const posts = defineCollection({
  name: "Post",
  pattern: "blog/**/*.mdx",
  schema: s
    .object({
      title: s.string().max(120),
      date: s.isodate(),
      summary: s.string().max(300),
      tags: s.array(s.string()).default([]),
      version: s.string().nullable().default(null),
      draft: s.boolean().default(false),
      slug: s.path(),
      body: s.mdx(),
    })
    .transform((data) => ({
      ...data,
      slug: data.slug.replace(/^blog\//, ""),
      url: `/blog/${data.slug.replace(/^blog\//, "")}`,
    })),
});

export default defineConfig({
  root: "content",
  output: { data: ".velite", assets: "public/static", base: "/static/", clean: true },
  collections: { posts },
  mdx: {
    rehypePlugins: [
      [rehypePrettyCode, { theme: { dark: "github-dark-dimmed", light: "github-light" }, keepBackground: false }],
    ],
  },
});
