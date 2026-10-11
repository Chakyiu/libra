const BACKEND = process.env.LIBRA_BACKEND_URL || "http://127.0.0.1:3001";

module.exports = {
  // Static export: the Rust binary serves web/out, so one process runs the whole app.
  output: "export",
  images: { unoptimized: true },
  trailingSlash: true,
  env: { NEXT_PUBLIC_APP_VERSION: require("./package.json").version },
  // The Rust service owns everything under /api; Next only serves the UI (dev server proxy).
  async rewrites() {
    return [{ source: "/api/:path*", destination: `${BACKEND}/api/:path*` }];
  },
};
