"use client";

import Image from "next/image";
import Link from "next/link";
import { useEffect, useState, useSyncExternalStore } from "react";
import {
  RiGithubFill,
  RiDiscordFill,
  RiBookOpenLine,
  RiMenuLine,
  RiCloseLine,
  RiShieldCheckLine,
  RiAppleFill,
  RiWindowsFill,
  RiDownloadLine,
  RiSaveLine,
  RiSpeedLine,
  RiTerminalBoxLine,
  RiEyeLine,
} from "@remixicon/react";
import { LauncherPreview } from "@/components/launcher-hero";

// Feature Card Component with premium styling
function FeatureCard({
  icon: Icon,
  title,
  description,
}: {
  icon: React.ElementType;
  title: string;
  description: string;
}) {
  return (
    <div className="group card card-highlight rounded-xl p-5 hover:shadow-[var(--shadow-md)]">
      {/* Icon container with glow */}
      <div className="mb-3 inline-flex rounded-lg bg-[rgb(var(--accent))]/10 p-2.5 shadow-[0_0_20px_rgba(232,168,85,0.1)]">
        <Icon className="h-5 w-5 text-[rgb(var(--accent))]" />
      </div>
      <h3 className="mb-1.5 text-sm font-semibold text-[rgb(var(--foreground))]">
        {title}
      </h3>
      <p className="text-sm text-[rgb(var(--foreground-secondary))] leading-relaxed">
        {description}
      </p>
    </div>
  );
}

const rotatingPhrases = [
  "VelGrinor",
  "Tired of 50GB of duplicate mods?",
  "One library. Infinite profiles.",
  "Open source · Rust + Tauri",
];

// Platform detection hook
function usePlatform() {
  return useSyncExternalStore(
    () => () => {},
    () => {
    const userAgent = navigator.userAgent.toLowerCase();
    if (userAgent.includes("mac")) {
        return "mac";
      }
      if (userAgent.includes("win")) {
        return "windows";
      }
      return "other";
    },
    () => "other",
  );
}

// Platform-aware download icon component
function DownloadIcon({ className }: { className?: string }) {
  const platform = usePlatform();

  if (platform === "mac") {
    return <RiAppleFill className={className} />;
  } else if (platform === "windows") {
    return <RiWindowsFill className={className} />;
  }
  return <RiDownloadLine className={className} />;
}

export default function HomePage() {
  const [mobileMenuOpen, setMobileMenuOpen] = useState(false);
  const [phraseIndex, setPhraseIndex] = useState(0);

  useEffect(() => {
    const interval = setInterval(() => {
      setPhraseIndex((prev) => (prev + 1) % rotatingPhrases.length);
    }, 3000);
    return () => clearInterval(interval);
  }, []);

  return (
    <div className="min-h-screen bg-mesh-subtle">
      {/* Header with glass effect and tint */}
      <header className="sticky top-0 z-50 glass-tinted border-b border-[rgb(var(--border))]">
        <div className="mx-auto flex max-w-5xl items-center justify-between px-4 py-3">
          <Link href="/" className="flex items-center gap-2 group">
            <span className="text-lg font-semibold text-[rgb(var(--foreground))] tracking-tight">
              VelGrinor
            </span>
          </Link>

          <nav className="hidden md:flex items-center gap-6">
            <Link
              href="https://github.com/Sqrilizz/VelGrinor"
              className="text-xs font-medium text-[rgb(var(--foreground-secondary))] hover:text-[rgb(var(--foreground))] transition-colors duration-150"
            >
              GitHub
            </Link>
            <Link
              href="https://discord.gg/2ng6q3JNQ7"
              className="text-xs font-medium text-[rgb(var(--foreground-secondary))] hover:text-[rgb(var(--foreground))] transition-colors duration-150"
            >
              Discord
            </Link>
            <Link
              href="/docs"
              className="text-xs font-medium text-[rgb(var(--foreground-secondary))] hover:text-[rgb(var(--foreground))] transition-colors duration-150"
            >
              Documentation
            </Link>
          </nav>

          <div className="flex items-center gap-2">
            <Link
              href="/docs"
              className="hidden sm:inline-flex items-center rounded-lg btn-primary px-3.5 py-1.5 text-xs font-medium"
            >
              View Docs
            </Link>
            {/* Mobile menu button */}
            <button
              onClick={() => setMobileMenuOpen(!mobileMenuOpen)}
              className="md:hidden p-2 rounded-lg hover:bg-[rgb(var(--foreground))]/10 transition-colors duration-150"
              aria-label={mobileMenuOpen ? "Close menu" : "Open menu"}
            >
              {mobileMenuOpen ? (
                <RiCloseLine className="h-5 w-5 text-[rgb(var(--foreground))]" />
              ) : (
                <RiMenuLine className="h-5 w-5 text-[rgb(var(--foreground))]" />
              )}
            </button>
          </div>
        </div>

        {/* Mobile menu dropdown */}
        {mobileMenuOpen && (
          <div className="md:hidden border-t border-[rgb(var(--border))] surface-1 animate-slide-up">
            <nav className="mx-auto max-w-5xl px-4 py-3 flex flex-col gap-1">
              <Link
                href="https://github.com/Sqrilizz/VelGrinor"
                className="flex items-center gap-2 px-3 py-2.5 rounded-lg text-sm font-medium text-[rgb(var(--foreground-secondary))] hover:text-[rgb(var(--foreground))] hover:bg-[rgb(var(--foreground))]/5 transition-all duration-150"
                onClick={() => setMobileMenuOpen(false)}
              >
                <RiGithubFill className="h-4 w-4" />
                GitHub
              </Link>
              <Link
                href="https://discord.gg/2ng6q3JNQ7"
                className="flex items-center gap-2 px-3 py-2.5 rounded-lg text-sm font-medium text-[rgb(var(--foreground-secondary))] hover:text-[rgb(var(--foreground))] hover:bg-[rgb(var(--foreground))]/5 transition-all duration-150"
                onClick={() => setMobileMenuOpen(false)}
              >
                <RiDiscordFill className="h-4 w-4" />
                Discord
              </Link>
              <Link
                href="/docs"
                className="flex items-center gap-2 px-3 py-2.5 rounded-lg text-sm font-medium text-[rgb(var(--foreground-secondary))] hover:text-[rgb(var(--foreground))] hover:bg-[rgb(var(--foreground))]/5 transition-all duration-150"
                onClick={() => setMobileMenuOpen(false)}
              >
                <RiBookOpenLine className="h-4 w-4" />
                Documentation
              </Link>
              <div className="mt-2 pt-2 border-t border-[rgb(var(--border))]">
                <Link
                  href="/docs"
                  className="flex items-center justify-center rounded-lg btn-primary px-3 py-2.5 text-sm font-medium"
                  onClick={() => setMobileMenuOpen(false)}
                >
                  View Docs
                </Link>
              </div>
            </nav>
          </div>
        )}
      </header>

      <main>
        {/* Hero Section */}
        <section className="mx-auto max-w-5xl px-4 py-16">
          <div className="text-center mb-12">
            {/* Logo with subtle glow */}
            <div className="relative inline-block mb-6">
              <div className="absolute inset-0 blur-3xl bg-[rgb(var(--accent))]/10 rounded-full scale-150" />
              <Image
                src="/logo.png"
                alt="VelGrinor"
                width={128}
                height={128}
                className="relative"
                priority
              />
            </div>
            <div className="h-10 mb-8 flex items-center justify-center">
              <h1
                key={phraseIndex}
                className="text-2xl md:text-3xl font-semibold text-[rgb(var(--foreground))] tracking-tight animate-text-rotate"
              >
                {rotatingPhrases[phraseIndex]}
              </h1>
            </div>

            {/* Download buttons with premium styling */}
            <div className="flex flex-wrap items-center justify-center gap-3 mb-12">
              <Link
                href="/download"
                className="inline-flex items-center gap-2 rounded-lg btn-primary px-5 py-2.5 text-sm font-medium"
              >
                <DownloadIcon className="h-4 w-4" />
                Download Desktop
              </Link>
              <Link
                href="/docs/cli"
                className="inline-flex items-center gap-2 rounded-lg btn-secondary px-5 py-2.5 text-sm font-medium"
              >
                <RiTerminalBoxLine className="h-4 w-4" />
                Install CLI
              </Link>
              <Link
                href="https://github.com/Sqrilizz/VelGrinor"
                className="inline-flex items-center gap-2 rounded-lg btn-secondary px-5 py-2.5 text-sm font-medium"
              >
                <RiGithubFill className="h-4 w-4" />
                GitHub
              </Link>
            </div>
          </div>

          {/* Launcher Preview */}
          <LauncherPreview />
        </section>

        {/* Features Section */}
        <section className="mx-auto max-w-5xl px-4 py-12">
          <div className="text-center mb-8">
            <span className="inline-block rounded-lg bg-[rgb(var(--accent))]/10 px-2.5 py-1 text-[10px] font-semibold uppercase tracking-wider text-[rgb(var(--accent))] shadow-[0_0_15px_rgba(232,168,85,0.1)]">
              Features
            </span>
            <h2 className="mt-3 text-xl font-semibold text-[rgb(var(--foreground))] tracking-tight">
              Why choose VelGrinor?
            </h2>
          </div>

          <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3 max-w-4xl mx-auto">
            <FeatureCard
              icon={RiSaveLine}
              title="Save Disk Space"
              description="Mods are stored once by SHA-256 hash and shared across profiles. No more duplicate files eating up your drive."
            />
            <FeatureCard
              icon={RiShieldCheckLine}
              title="Reproducible Profiles"
              description="Your setup is a JSON file. Version control it, share it, diff it, restore it. It just works."
            />
            <FeatureCard
              icon={RiSpeedLine}
              title="Fast & Lightweight"
              description="Built in Rust with Tauri. Responsive UI and predictable behavior with no hidden database state."
            />
            <FeatureCard
              icon={RiEyeLine}
              title="No Hidden State"
              description="Plain JSON on disk. Predictable directory layout. Fully inspectable, no magic sync or mystery files."
            />
            <FeatureCard
              icon={RiTerminalBoxLine}
              title="CLI-First"
              description="Every feature works from the command line. Script it, automate it, integrate it into your workflow."
            />
            <FeatureCard
              icon={RiGithubFill}
              title="Open Source"
              description="Inspect every line of code. No telemetry, no launcher account required. Your data stays local."
            />
          </div>
        </section>

      </main>

      {/* Footer */}
      <footer className="border-t border-[rgb(var(--border))]">
        <div className="mx-auto max-w-5xl px-4 py-6">
          <div className="flex flex-col items-center justify-between gap-3 md:flex-row">
            <p className="text-xs text-[rgb(var(--foreground-muted))]">
              © {new Date().getFullYear()}{" "}
              <Link
                href="https://thomas.md"
                className="text-[rgb(var(--foreground-tertiary))] hover:text-[rgb(var(--foreground-secondary))] transition-colors duration-150"
              >
                Thomas Marchand
              </Link>
            </p>
            <Link
              href="https://github.com/Sqrilizz/VelGrinor"
              className="text-[rgb(var(--foreground-muted))] hover:text-[rgb(var(--foreground-tertiary))] transition-colors duration-150"
            >
              <RiGithubFill className="h-4 w-4" />
            </Link>
          </div>
        </div>
      </footer>
    </div>
  );
}
