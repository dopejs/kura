export type PageLayout = "doc" | "home";

export interface TableOfContentsItem {
  readonly id: string;
  readonly level: 2 | 3;
  readonly title: string;
}

export interface SitePage {
  readonly route: string;
  readonly href: string;
  readonly title: string;
  readonly description: string;
  readonly layout: PageLayout;
  readonly html: string;
  readonly tableOfContents: readonly TableOfContentsItem[];
  readonly lastUpdated: string;
  /** Translations keyed by BCP 47 language (`zh-Hans`, …); absent = English only. */
  readonly localized?: Readonly<Record<string, LocalizedPage>>;
}

export interface LocalizedPage {
  readonly title: string;
  readonly description: string;
  readonly html: string;
  readonly tableOfContents: readonly TableOfContentsItem[];
}

export interface PageSummary {
  readonly route: string;
  readonly href: string;
  readonly title: string;
  readonly description: string;
  readonly headings: readonly string[];
  readonly text: string;
}

export interface PageLink {
  readonly href: string;
  readonly title: string;
  readonly localized?: Readonly<Record<string, string>>;
}

export interface SitePayload {
  readonly page: SitePage;
  /** Every documentation page, in order, with localized titles for the sidebar. */
  readonly navigation?: readonly PageLink[];
  readonly previous?: PageLink;
  readonly next?: PageLink;
}
