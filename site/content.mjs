import { readFile, stat } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import MarkdownIt from "markdown-it";
import anchor from "markdown-it-anchor";

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const siteRoot = path.join(repositoryRoot, "site");

const DOCUMENTS = [
  ["getting-started", "Getting Started", "Install Kura and start the daemon, TUI, and web client.", "site/src/content/getting-started.md"],
  ["usage", "Usage", "Operate Kura from its CLI, terminal interface, web client, and channels.", "site/src/content/usage.md"],
  ["configuration", "Configuration", "Configure Kura profiles, providers, storage, and runtime policy.", "site/src/content/configuration.md"],
  ["plugins", "Plugins", "Understand Kura's plugin kernel and built-in capabilities.", "site/src/content/plugins.md"],
  ["external-plugins", "External Plugins", "Extend Kura with isolated plugins written in any language.", "site/src/content/external-plugins.md"],
  ["memory", "Memory", "Inspect Kura's attributable, reversible layered memory.", "site/src/content/memory.md"],
  ["context-session", "Context & Session", "Learn how Kura assembles cited context and durable sessions.", "site/src/content/context-session.md"],
  ["skills-improvement", "Skills & Self-Improvement", "Govern agent-authored skills and configuration improvements.", "site/src/content/skills-improvement.md"],
  ["channels", "Channels", "Connect Kura to supported messaging channels safely.", "site/src/content/channels.md"],
  ["api", "API Reference", "Use Kura's local HTTP API.", "site/src/content/api.md"],
  ["deployment", "Deployment", "Run Kura for a team: TLS, tokens, metrics, backups.", "site/src/content/deployment.md"],
  ["architecture", "Architecture", "Read the source-of-truth plugin architecture design.", "docs/harness/plugin-architecture.md"],
];

const markdown = new MarkdownIt({ html: true, linkify: true, typographer: true });
markdown.use(anchor, {
  slugify(value) {
    return value.trim().toLocaleLowerCase().replace(/[^\p{Letter}\p{Number}\s_-]/gu, "").replace(/[\s_]+/gu, "-").replace(/^-+|-+$/gu, "");
  },
});
const originalLink = markdown.renderer.rules.link_open;
markdown.renderer.rules.link_open = (tokens, index, options, environment, self) => {
  const href = tokens[index].attrGet("href");
  if (href !== null && /^https?:/u.test(href)) {
    tokens[index].attrSet("target", "_blank");
    tokens[index].attrSet("rel", "noreferrer");
  }
  return originalLink?.(tokens, index, options, environment, self) ?? self.renderToken(tokens, index, options);
};

function inlineText(token) {
  if (token.type !== "inline") return "";
  return token.content.replace(/<[^>]+>/gu, " ").replace(/\s+/gu, " ").trim();
}

function hrefForRoute(route) { return route === "/" ? "/" : `${route}/`; }

function requestRoute(pathname) {
  let decoded;
  try { decoded = decodeURIComponent(pathname); } catch { decoded = pathname; }
  if (decoded === "/" || decoded === "/index.html") return "/";
  if (decoded.endsWith("/index.html")) return decoded.slice(0, -11);
  return decoded.replace(/\/+$/u, "");
}

export async function loadSiteContent() {
  const pages = [{
    route: "/", href: "/", title: "Kura", description: "An inspectable personal agent OS.",
    layout: "home", html: "", tableOfContents: [], lastUpdated: new Date(0).toISOString(),
    headings: [], text: "personal agent OS plugins memory context sessions channels audit",
  }];

  for (const [slug, title, description, sourcePath] of DOCUMENTS) {
    const absolute = path.join(repositoryRoot, sourcePath);
    const [source, metadata] = await Promise.all([readFile(absolute, "utf8"), stat(absolute)]);
    const tokens = markdown.parse(source, {});
    const headings = [];
    const tableOfContents = [];
    for (let index = 0; index < tokens.length; index += 1) {
      const token = tokens[index];
      if (token.type !== "heading_open") continue;
      const heading = inlineText(tokens[index + 1]);
      const level = Number(token.tag.slice(1));
      if (heading !== "") headings.push(heading);
      if ((level === 2 || level === 3) && heading !== "") {
        tableOfContents.push({ id: token.attrGet("id") ?? "", level, title: heading });
      }
    }
    const route = `/docs/${slug}`;
    pages.push({
      route, href: hrefForRoute(route), title, description, layout: "doc",
      html: markdown.renderer.render(tokens, markdown.options, {}), tableOfContents,
      lastUpdated: metadata.mtime.toISOString(), headings,
      text: tokens.map(inlineText).filter(Boolean).join(" ").slice(0, 10_000),
    });
  }

  const publicPage = (page) => ({
    route: page.route, href: page.href, title: page.title, description: page.description,
    layout: page.layout, html: page.html, tableOfContents: page.tableOfContents,
    lastUpdated: page.lastUpdated,
  });
  const payloadForPage = (page) => {
    const index = pages.indexOf(page);
    return {
      page: publicPage(page),
      ...(index > 1 ? { previous: { href: pages[index - 1].href, title: pages[index - 1].title } } : {}),
      ...(index > 0 && index < pages.length - 1 ? { next: { href: pages[index + 1].href, title: pages[index + 1].title } } : {}),
    };
  };
  const byRoute = new Map(pages.map((page) => [page.route, page]));
  return {
    pages,
    searchIndex: pages.map((page) => ({ route: page.route, href: page.href, title: page.title, description: page.description, headings: page.headings, text: page.text })),
    payloadForPage,
    payloadForPath(pathname) { return payloadForPage(byRoute.get(requestRoute(pathname)) ?? pages[0]); },
  };
}
