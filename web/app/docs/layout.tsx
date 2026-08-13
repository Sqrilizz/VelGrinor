import type { ReactNode } from "react";
import { Layout, Navbar } from "nextra-theme-docs";
import { getPageMap } from "nextra/page-map";
import "nextra-theme-docs/style.css";
import "./docs.css";

// Custom logo component
function Logo() {
  return (
    <div style={{ display: "inline-flex", alignItems: "center", gap: 10 }}>
      <span
        style={{
          fontWeight: 600,
          fontSize: 16,
          color: "rgba(245, 240, 235, 0.95)",
        }}
      >
        VelGrinor
      </span>
    </div>
  );
}

export default async function DocsLayout({
  children,
}: {
  children: ReactNode;
}) {
  const navbar = (
    <Navbar
      logo={<Logo />}
      logoLink="/docs"
      projectLink="https://github.com/Sqrilizz/VelGrinor"
    />
  );
  // Get only the docs page map
  const pageMap = await getPageMap("/docs");
  return (
    <Layout
      navbar={navbar}
      editLink="Edit this page on GitHub"
      docsRepositoryBase="https://github.com/Sqrilizz/VelGrinor/blob/main/web"
      sidebar={{ defaultMenuCollapseLevel: 1 }}
      pageMap={pageMap}
      footer={null}
    >
      {children}
    </Layout>
  );
}
