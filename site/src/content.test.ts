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
});
