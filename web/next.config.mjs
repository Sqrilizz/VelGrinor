import nextra from "nextra";

const withNextra = nextra({
  latex: true,
  search: {
    codeblocks: false,
  },
  contentDirBasePath: "/docs",
});

export default withNextra({
  reactStrictMode: true,
  turbopack: {
    root: import.meta.dirname,
  },
  experimental: {
    optimizeCss: false,
  },
  images: {
    formats: ["image/avif", "image/webp"],
  },
  async redirects() {
    return [
      {
        source: "/download",
        destination: "https://github.com/Sqrilizz/VelGrinor/releases/latest",
        permanent: false,
      },
    ];
  },
});
