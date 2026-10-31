import { defineConfig } from "astro/config";

// The domain is not final. Canonical links, Open Graph, robots and the sitemap all
// read this one value (robots.txt and sitemap.xml are generated from it in src/pages).
export default defineConfig({
  site: "https://pantau-app-swart.vercel.app",
  trailingSlash: "ignore",
  devToolbar: { enabled: false },
});
