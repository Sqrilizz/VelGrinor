import type { Metadata, Viewport } from "next";
import type { ReactNode } from "react";
import { Head } from "nextra/components";
import { ThemeProvider } from "@/components/theme-provider";
import { JsonLd } from "@/components/JsonLd";
import { GoogleAnalytics } from "@/components/GoogleAnalytics";
import "./globals.css";

export const viewport: Viewport = {
  themeColor: "#0c0b0a",
  colorScheme: "dark",
  width: "device-width",
  initialScale: 1,
  viewportFit: "cover",
};

export const metadata: Metadata = {
  metadataBase: new URL("https://sqrilizz.tech"),
  title: {
    default: "VelGrinor | Reproducible Minecraft launcher with a deduplicated library",
    template: "%s | VelGrinor",
  },
  description:
    "Open-source Minecraft launcher with declarative profiles, content-addressed storage, and Modrinth/CurseForge integration. CLI and desktop.",
  applicationName: "VelGrinor",
  generator: "Next.js",
  keywords: [
    "minecraft",
    "launcher",
    "mod manager",
    "fabric",
    "forge",
    "quilt",
    "neoforge",
    "modrinth",
    "curseforge",
    "open source",
  ],
  authors: [{ name: "Sqrilizz", url: "https://sqrilizz.tech" }],
  creator: "Sqrilizz",
  publisher: "VelGrinor",
  robots: {
    index: true,
    follow: true,
  },
  twitter: {
    card: "summary_large_image",
    title: "VelGrinor",
    description:
      "Open-source Minecraft launcher with declarative profiles, content-addressed storage, and Modrinth/CurseForge integration.",
  },
  openGraph: {
    type: "website",
    locale: "en_US",
    url: "https://sqrilizz.tech",
    siteName: "VelGrinor",
    title: "VelGrinor",
    description:
      "Open-source Minecraft launcher with declarative profiles, content-addressed storage, and Modrinth/CurseForge integration. CLI and desktop.",
  },
  icons: {
    icon: [
      { url: "/favicon.ico", sizes: "48x48" },
      { url: "/favicon-32x32.png", sizes: "32x32", type: "image/png" },
      { url: "/favicon-16x16.png", sizes: "16x16", type: "image/png" },
    ],
    apple: [
      { url: "/apple-touch-icon.png", sizes: "180x180", type: "image/png" },
    ],
  },
  // Safari-specific
  appleWebApp: {
    capable: true,
    statusBarStyle: "black-translucent",
    title: "VelGrinor",
  },
  // Additional meta for better integration
  other: {
    "msapplication-TileColor": "#0c0b0a",
  },
};

export default function RootLayout({
  children,
}: {
  children: ReactNode;
}) {
  return (
    <html lang="en" dir="ltr" suppressHydrationWarning>
      <Head>
        <JsonLd />
        {/* Additional theme-color variants for different contexts */}
        <meta
          name="theme-color"
          media="(prefers-color-scheme: light)"
          content="#0c0b0a"
        />
        <meta
          name="theme-color"
          media="(prefers-color-scheme: dark)"
          content="#0c0b0a"
        />
      </Head>
      <body className="min-h-dvh bg-mesh-subtle">
        <GoogleAnalytics />
        <ThemeProvider>{children}</ThemeProvider>
      </body>
    </html>
  );
}
