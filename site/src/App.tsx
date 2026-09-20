import { useEffect, useState, type ReactNode } from "react";

import { LanguageMenu } from "./LanguageMenu";
import { writeLanguagePreference } from "./language-preference";
import { localeForPath, type SiteLocale } from "./locales";
import { SearchDialog } from "./SearchDialog";
import type { PageLink, SitePage, SitePayload } from "./types";

const REPO = "https://github.com/dopejs/kura";

interface AppProps {
  readonly payload: SitePayload;
  readonly initialLocalePath: string;
}

const DOCS = [
  ["Getting Started", "/docs/getting-started/"], ["Usage", "/docs/usage/"],
  ["Configuration", "/docs/configuration/"], ["Plugins", "/docs/plugins/"],
  ["External Plugins", "/docs/external-plugins/"], ["Memory", "/docs/memory/"],
  ["Context & Session", "/docs/context-session/"], ["Skills & Self-Improvement", "/docs/skills-improvement/"],
  ["Channels", "/docs/channels/"], ["API Reference", "/docs/api/"],
  ["Deployment", "/docs/deployment/"], ["Architecture", "/docs/architecture/"],
] as const;

function KuraMark({ className }: { readonly className: string }): ReactNode {
  return <span className={`kura-mark ${className}`} aria-hidden="true">
    <img className="kura-mark__light" src="/kura-mark.svg?v=20260820-2" alt="" />
    <img className="kura-mark__dark" src="/kura-mark-inverse.svg?v=20260820-2" alt="" />
  </span>;
}

function Header({ page, locale, onLocaleChange }: { readonly page: SitePage; readonly locale: SiteLocale; onLocaleChange(path: string): void }): ReactNode {
  const [searchOpen, setSearchOpen] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  useEffect(() => {
    const handler = (event: KeyboardEvent): void => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault(); setSearchOpen(true);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);
  const toggleTheme = (): void => {
    const next = document.documentElement.dataset.theme === "dark" ? "light" : "dark";
    document.documentElement.dataset.theme = next;
    localStorage.setItem("kura-theme", next);
  };
  return <>
    <header className="site-header">
      <a className="brand" href="/" aria-label="Kura home"><KuraMark className="brand__mark" /><span>Kura</span><small>{locale.ui.preRelease}</small></a>
      <nav className={menuOpen ? "top-nav top-nav--open" : "top-nav"} aria-label="Primary">
        <a href="/" aria-current={page.route === "/" ? "page" : undefined}>{locale.ui.overview}</a>
        <a href="/docs/getting-started/" aria-current={page.route.startsWith("/docs/") ? "page" : undefined}>{locale.ui.docs}</a>
        <a href={`${REPO}/releases`}>{locale.ui.releases}</a>
      </nav>
      <div className="header-tools">
        <button className="search-trigger" type="button" onClick={() => setSearchOpen(true)}><span aria-hidden="true">⌕</span><span>{locale.ui.search}</span><kbd>⌘ K</kbd></button>
        <LanguageMenu locale={locale} onChange={onLocaleChange} />
        <button className="icon-button" type="button" aria-label={locale.ui.appearance} title={locale.ui.appearance} onClick={toggleTheme}>◐</button>
        <a className="icon-button" href={REPO} aria-label="GitHub">GH</a>
        <button className="mobile-menu" type="button" aria-label={locale.ui.menu} aria-expanded={menuOpen} onClick={() => setMenuOpen((value) => !value)}>{menuOpen ? "×" : "☰"}</button>
      </div>
    </header>
    <SearchDialog open={searchOpen} locale={locale} onClose={() => setSearchOpen(false)} />
  </>;
}

function Home({ locale }: { readonly locale: SiteLocale }): ReactNode {
  return <main className="home-page">
    <section className="hero">
      <div className="hero__copy">
        <p className="eyebrow">model-visible = logged</p>
        <h1>A <em>personal agent OS</em> you can open up and read.</h1>
        <p className="hero__lead">A Rust daemon that owns runtime, memory, context, and policy — with a plugin architecture where session management, retrieval, and even the embedding model are swappable parts.</p>
        <div className="hero__actions"><a className="button button--brand" href="/docs/getting-started/">{locale.ui.getStarted}</a><a className="button" href={`${REPO}/releases`}>{locale.ui.download}</a></div>
      </div>
      <div className="hero__visual" aria-hidden="true"><KuraMark className="hero__mark" /><span className="orbit orbit--one" /><span className="orbit orbit--two" /></div>
    </section>
    <section className="terminal" aria-label="Install Kura"><pre><code>{`$ curl -fsSL https://kura.dopejs.com/install.sh | sh
[kura] checksum verified
[kura] installed: kura, kura-tui

$ kura daemon start
daemon started at http://127.0.0.1:19191

$ kura tui`}</code></pre></section>
    <section className="features">
      {[
        ["Everything is a plugin", "31+ built-in plugins over a small trust-boundary kernel. Disable, configure, or replace capabilities from one profile."],
        ["Layered memory", "Episodes, atoms, scenarios, and personas remain attributable, reversible, and inspectable."],
        ["A context engine that cites", "Hybrid recall and symbolic compression under budget, with a citation attached to every injected source."],
        ["Sessions that never forget", "Frame-preserving windows capture evicted spans to memory before context is compacted."],
        ["External plugins, any language", "Attach manifests and isolated processes to the hook waterfall over a line-JSON protocol."],
        ["Governed autonomy", "Skills and configuration improvements require evidence, approval, audit history, and rollback."],
      ].map(([title, body], index) => <article key={title}><span>{String(index + 1).padStart(2, "0")}</span><h2>{title}</h2><p>{body}</p></article>)}
    </section>
  </main>;
}

function Sidebar({ page, locale }: { readonly page: SitePage; readonly locale: SiteLocale }): ReactNode {
  return <aside className="sidebar" aria-label={locale.ui.documentation}><h2>{locale.ui.documentation}</h2>{DOCS.map(([title, href]) => <a key={href} href={href} aria-current={page.href === href ? "page" : undefined}>{title}</a>)}</aside>;
}

function Outline({ page, locale }: { readonly page: SitePage; readonly locale: SiteLocale }): ReactNode {
  if (page.tableOfContents.length === 0) return null;
  return <aside className="outline" aria-label={locale.ui.onThisPage}><h2>{locale.ui.onThisPage}</h2>{page.tableOfContents.map((item) => <a key={item.id} className={`outline-${String(item.level)}`} href={`#${item.id}`}>{item.title}</a>)}</aside>;
}

function Pagination({ previous, next, locale }: { readonly previous?: PageLink; readonly next?: PageLink; readonly locale: SiteLocale }): ReactNode {
  const link = (item: PageLink | undefined, direction: "previous" | "next") => item === undefined ? <span /> : <a className={`page-link page-link--${direction}`} href={item.href}><small>{direction === "previous" ? locale.ui.previous : locale.ui.next}</small><strong>{item.title}</strong></a>;
  return <nav className="pagination" aria-label="Pagination">{link(previous, "previous")}{link(next, "next")}</nav>;
}

function Footer(): ReactNode { return <footer className="site-footer"><span>Kura — an inspectable personal agent OS.</span><span>© 2026 Kura contributors</span></footer>; }

export function App({ payload, initialLocalePath }: AppProps): ReactNode {
  const [localePath, setLocalePath] = useState(initialLocalePath);
  const locale = localeForPath(localePath);
  const changeLocale = (path: string): void => {
    const next = localeForPath(path);
    writeLanguagePreference(next.path); setLocalePath(next.path);
    document.documentElement.lang = next.lang; document.documentElement.dir = next.dir ?? "ltr";
  };
  return <div className="site" dir={locale.dir ?? "ltr"}>
    <Header page={payload.page} locale={locale} onLocaleChange={changeLocale} />
    {payload.page.layout === "home" ? <Home locale={locale} /> : <div className="docs-grid">
      <Sidebar page={payload.page} locale={locale} />
      <main className="doc-main"><p className="language-notice">{locale.ui.englishOnly}</p><article className="doc-content" dangerouslySetInnerHTML={{ __html: payload.page.html }} /><p className="last-updated">{locale.ui.lastUpdated}: <time dateTime={payload.page.lastUpdated}>{payload.page.lastUpdated.slice(0, 10)}</time></p><Pagination previous={payload.previous} next={payload.next} locale={locale} /></main>
      <Outline page={payload.page} locale={locale} />
    </div>}
    <Footer />
  </div>;
}
