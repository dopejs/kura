import { describe, expect, it } from "vitest";

import { loadSiteContent } from "../content.mjs";

describe("site content", () => {
  it("generates the canonical home and documentation routes", async () => {
    const content = await loadSiteContent();
    expect(content.pages).toHaveLength(13);
    expect(content.pages.map((page) => page.route)).toContain("/docs/architecture");
    expect(content.pages.every((page) => page.href === "/" || page.href.endsWith("/"))).toBe(true);
  });

  it("renders anchored headings and search records", async () => {
    const content = await loadSiteContent();
    const plugins = content.pages.find((page) => page.route === "/docs/plugins");
    expect(plugins?.html).toContain('id="introspection"');
    expect(plugins?.tableOfContents.some((heading) => heading.id === "introspection")).toBe(true);
    expect(content.searchIndex).toHaveLength(content.pages.length);
  });

  it("carries Simplified and Traditional Chinese translations for every documentation page", async () => {
    const content = await loadSiteContent();
    const docs = content.pages.filter((page) => page.layout === "doc");
    for (const page of docs) {
      const localized = (page as { localized?: Record<string, { html: string; title: string }> }).localized ?? {};
      expect(Object.keys(localized), page.route).toEqual(expect.arrayContaining(["zh-Hans", "zh-Hant"]));
      expect(localized["zh-Hans"].html.length, page.route).toBeGreaterThan(200);
      expect(localized["zh-Hant"].title, page.route).not.toEqual(page.title);
    }
    const payload = content.payloadForPath("/docs/plugins/");
    expect(payload.navigation?.length).toBe(docs.length);
    expect(payload.navigation?.[0].localized?.["zh-Hans"]).toBe("快速开始");
  });
});
