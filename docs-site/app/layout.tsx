import type { Metadata } from "next";
import { Layout, Navbar } from "nextra-theme-docs";
import { Head } from "nextra/components";
import { getPageMap } from "nextra/page-map";
import "nextra-theme-docs/style.css";
import "./styles.css";

export const metadata: Metadata = {
  title: {
    default: "Skippy Docs",
    template: "%s – Skippy Docs",
  },
  description:
    "Install, operate, configure, and contribute to the Skippy inference engine.",
};

const logo = (
  <span className="skippy-brand">
    <img
      className="skippy-logo"
      src="/skippy-logo.png"
      alt="Skippy"
      width="2172"
      height="724"
    />
  </span>
);

const navbar = (
  <Navbar
    logo={logo}
    projectLink="https://github.com/Mesh-LLM/skippy"
  />
);

export default async function RootLayout({
  children,
}: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en" dir="ltr" suppressHydrationWarning>
      <Head />
      <body>
        <Layout
          navbar={navbar}
          pageMap={await getPageMap()}
          docsRepositoryBase="https://github.com/Mesh-LLM/skippy/tree/main/docs-site/content"
          editLink="Edit this page on GitHub"
          feedback={{ content: "Question or correction? Open an issue →" }}
        >
          {children}
        </Layout>
      </body>
    </html>
  );
}
