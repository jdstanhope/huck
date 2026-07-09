import { codeToHtml } from "shiki";

export async function CodeBlock({ code, lang = "bash" }: { code: string; lang?: string }) {
  const html = await codeToHtml(code.trim(), {
    lang,
    themes: { dark: "github-dark-dimmed", light: "github-light" },
    defaultColor: false,
  });
  return (
    <div
      className="overflow-x-auto rounded-lg border border-zinc-200 dark:border-zinc-800 text-sm [&_pre]:p-4"
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}
