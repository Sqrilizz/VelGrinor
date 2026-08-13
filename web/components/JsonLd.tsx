export function JsonLd() {
  const softwareApplication = {
    "@context": "https://schema.org",
    "@type": "SoftwareApplication",
    name: "VelGrinor",
    description:
      "A minimal, content-addressed Minecraft launcher focused on stability and reproducibility. One library. Infinite profiles.",
    url: "https://sqrilizz.tech",
    applicationCategory: "GameApplication",
    operatingSystem: ["macOS", "Windows", "Linux"],
    offers: {
      "@type": "Offer",
      price: "0",
      priceCurrency: "USD",
    },
    author: {
      "@type": "Person",
      name: "Sqrilizz",
      url: "https://sqrilizz.tech",
    },
    downloadUrl: "https://sqrilizz.tech/docs/getting-started",
    keywords: [
      "Minecraft",
      "launcher",
      "mod manager",
      "Fabric",
      "Forge",
      "Quilt",
      "NeoForge",
      "Modrinth",
      "CurseForge",
    ],
    featureList: [
      "Content-addressed storage for mods and resource packs",
      "Multiple profile support",
      "Modrinth and CurseForge integration",
      "Fabric, Forge, Quilt, and NeoForge support",
      "CLI-first design",
    ],
  };

  const organization = {
    "@context": "https://schema.org",
    "@type": "Organization",
    name: "VelGrinor",
    url: "https://sqrilizz.tech",
    logo: "https://sqrilizz.tech/icon-512.png",
    sameAs: ["https://github.com/Sqrilizz/VelGrinor"],
  };

  return (
    <>
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: JSON.stringify(softwareApplication) }}
      />
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: JSON.stringify(organization) }}
      />
    </>
  );
}
