import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "huck — a bash-compatible shell in Rust",
  description: "A bash-compatible shell written in Rust, verified byte-for-byte against real bash.",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body>{children}</body>
    </html>
  );
}
