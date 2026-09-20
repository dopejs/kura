import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { App } from "./App";
import { SITE_LOCALES } from "./locales";
import type { SitePayload } from "./types";

const homePayload: SitePayload = {
  page: {
    route: "/", href: "/", title: "Kura", description: "English fallback",
    layout: "home", html: "", tableOfContents: [], lastUpdated: new Date(0).toISOString(),
  },
};

describe("localized marketing site", () => {
  it("renders the complete home page in every advertised language", () => {
    const english = SITE_LOCALES[0].home;
    for (const locale of SITE_LOCALES) {
      const html = renderToStaticMarkup(<App payload={homePayload} initialLocalePath={locale.path} />);
      expect(locale.home.features, locale.lang).toHaveLength(6);
      expect(html, locale.lang).toContain(locale.home.headlineEmphasis);
      expect(html, locale.lang).toContain(locale.home.lead);
      expect(html, locale.lang).toContain(locale.home.features[5].title);
      expect(html, locale.lang).toContain(locale.home.footerTagline);
      expect(html, locale.lang).toContain(`aria-label="${locale.ui.primaryNavigation}"`);
      if (locale.lang !== "en") {
        expect(locale.home.headlineEmphasis, locale.lang).not.toBe(english.headlineEmphasis);
        expect(locale.home.features[0].title, locale.lang).not.toBe(english.features[0].title);
      }
    }
  });

  it("renders right-to-left marketing pages in RTL mode", () => {
    for (const path of ["he", "ar"]) {
      const html = renderToStaticMarkup(<App payload={homePayload} initialLocalePath={path} />);
      expect(html).toContain('dir="rtl"');
    }
  });

  it("keeps Chinese hero headlines free of trailing punctuation", () => {
    for (const lang of ["zh-Hans", "zh-Hant"]) {
      const locale = SITE_LOCALES.find((candidate) => candidate.lang === lang);
      expect(locale?.home.headlineTail, lang).toBe("");
    }
  });
});
