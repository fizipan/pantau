import type { APIRoute } from "astro";

export const GET: APIRoute = ({ site }) => {
  const en = new URL("/", site).href, id = new URL("/id/", site).href;
  const alt = `<xhtml:link rel="alternate" hreflang="en" href="${en}"/><xhtml:link rel="alternate" hreflang="id" href="${id}"/><xhtml:link rel="alternate" hreflang="x-default" href="${en}"/>`;
  const xml = `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:xhtml="http://www.w3.org/1999/xhtml">\n<url><loc>${en}</loc>${alt}</url>\n<url><loc>${id}</loc>${alt}</url>\n</urlset>\n`;
  return new Response(xml, { headers: { "Content-Type": "application/xml; charset=utf-8" } });
};
