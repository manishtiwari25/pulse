import type { NextConfig } from "next";

// Static export: the Go hub embeds web/hub/out with //go:embed all:out and
// serves out/<route>/index.html. The dev-only rewrite proxies /api to a
// running `pulse hub --no-browser --port 8787`; `output: "export"` ignores
// rewrites at build time, which is what we want (every API call is relative).
const hubPort = process.env.PULSE_HUB_PORT ?? "8787";

const dev = process.env.NODE_ENV === "development";

// Where the export lands. Unset (the default, `npm run build`) writes out/,
// the committed tree the binary embeds. `npm run check` sets it to
// .next/check-out so a verify run builds without rewriting out/; that dir is
// gitignored and `next build` deletes it with the rest of .next next time.
// Under `output: "export"`, Next reads distDir as the export directory and
// keeps its own build state in .next.
const outDir = process.env.PULSE_HUB_OUT_DIR;

const nextConfig: NextConfig = {
  output: "export",
  trailingSlash: true,
  images: { unoptimized: true },
  reactStrictMode: true,
  ...(outDir ? { distDir: outDir } : {}),
  ...(dev
    ? {
        // Without this, dev would 308 /api/v1/x -> /api/v1/x/ before proxying.
        skipTrailingSlashRedirect: true,
        async rewrites() {
          return [{ source: "/api/:path*", destination: `http://127.0.0.1:${hubPort}/api/:path*` }];
        },
      }
    : {}),
};

export default nextConfig;
