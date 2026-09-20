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

// Translated documents live at site/src/content/<lang>/<slug>.md. A locale
// listed here with a file for a slug is served in that language; a missing
// file falls back to English with the "English only" notice. URLs stay
// language-neutral: the translation travels inside the page payload and the
// client picks it by the reader's language preference.
const LOCALIZED = {
  "zh-Hans": {
    "getting-started": ["快速开始", "安装 Kura 并启动守护进程、终端界面和 Web 客户端。"],
    usage: ["使用", "通过 CLI、终端界面、Web 客户端和聊天频道使用 Kura。"],
    configuration: ["配置", "配置 Kura 的 profile、模型提供方、存储和运行策略。"],
    plugins: ["插件", "理解 Kura 的插件内核与内置能力。"],
    "external-plugins": ["外部插件", "用任意语言编写隔离运行的插件来扩展 Kura。"],
    memory: ["记忆", "查看 Kura 可归因、可撤销的分层记忆。"],
    "context-session": ["上下文与会话", "了解 Kura 如何组装带引用的上下文与持久会话。"],
    "skills-improvement": ["技能与自我改进", "治理由 agent 编写的技能与配置改进。"],
    channels: ["频道", "安全地把 Kura 接入支持的消息频道。"],
    api: ["API 参考", "使用 Kura 的本地 HTTP API。"],
    deployment: ["部署", "面向团队运行 Kura：TLS、令牌、指标、备份。"],
    architecture: ["架构", "阅读作为事实来源的插件架构设计。"],
  },
  "zh-Hant": {
    "getting-started": ["快速開始", "安裝 Kura 並啟動守護程序、終端介面與 Web 客戶端。"],
    usage: ["使用", "透過 CLI、終端介面、Web 客戶端與聊天頻道使用 Kura。"],
    configuration: ["設定", "設定 Kura 的 profile、模型供應商、儲存與執行策略。"],
    plugins: ["外掛", "理解 Kura 的外掛核心與內建能力。"],
    "external-plugins": ["外部外掛", "用任意語言撰寫隔離執行的外掛來擴充 Kura。"],
    memory: ["記憶", "檢視 Kura 可歸因、可撤銷的分層記憶。"],
    "context-session": ["上下文與工作階段", "了解 Kura 如何組裝帶引用的上下文與持久工作階段。"],
    "skills-improvement": ["技能與自我改進", "治理由 agent 撰寫的技能與設定改進。"],
    channels: ["頻道", "安全地把 Kura 接入支援的訊息頻道。"],
    api: ["API 參考", "使用 Kura 的本機 HTTP API。"],
    deployment: ["部署", "面向團隊執行 Kura：TLS、權杖、指標、備份。"],
    architecture: ["架構", "閱讀作為事實來源的外掛架構設計。"],
  },
};

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

  const renderDocument = (source) => {
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
    return {
      html: markdown.renderer.render(tokens, markdown.options, {}),
      headings, tableOfContents,
      text: tokens.map(inlineText).filter(Boolean).join(" ").slice(0, 10_000),
    };
  };

  for (const [slug, title, description, sourcePath] of DOCUMENTS) {
    const absolute = path.join(repositoryRoot, sourcePath);
    const [source, metadata] = await Promise.all([readFile(absolute, "utf8"), stat(absolute)]);
    const rendered = renderDocument(source);
    const localized = {};
    let lastUpdated = metadata.mtime;
    let text = rendered.text;
    let headings = rendered.headings;
    for (const [lang, titles] of Object.entries(LOCALIZED)) {
      const entry = titles[slug];
      if (entry === undefined) continue;
      const translated = path.join(siteRoot, "src", "content", lang, `${slug}.md`);
      let translatedSource;
      let translatedStat;
      try {
        [translatedSource, translatedStat] = await Promise.all([readFile(translated, "utf8"), stat(translated)]);
      } catch {
        continue;
      }
      const page = renderDocument(translatedSource);
      localized[lang] = { title: entry[0], description: entry[1], html: page.html, tableOfContents: page.tableOfContents };
      if (translatedStat.mtime > lastUpdated) lastUpdated = translatedStat.mtime;
      text = `${text} ${page.text}`.slice(0, 20_000);
      headings = [...headings, ...page.headings];
    }
    const route = `/docs/${slug}`;
    pages.push({
      route, href: hrefForRoute(route), title, description, layout: "doc",
      html: rendered.html, tableOfContents: rendered.tableOfContents, localized,
      lastUpdated: lastUpdated.toISOString(), headings, text,
    });
  }

  const publicPage = (page) => ({
    route: page.route, href: page.href, title: page.title, description: page.description,
    layout: page.layout, html: page.html, tableOfContents: page.tableOfContents,
    lastUpdated: page.lastUpdated, ...(page.localized === undefined ? {} : { localized: page.localized }),
  });
  const localizedTitles = (page) => Object.fromEntries(Object.entries(page.localized ?? {}).map(([lang, value]) => [lang, value.title]));
  const link = (page) => ({ href: page.href, title: page.title, localized: localizedTitles(page) });
  const navigation = pages.filter((page) => page.layout === "doc").map(link);
  const payloadForPage = (page) => {
    const index = pages.indexOf(page);
    return {
      page: publicPage(page),
      navigation,
      ...(index > 1 ? { previous: link(pages[index - 1]) } : {}),
      ...(index > 0 && index < pages.length - 1 ? { next: link(pages[index + 1]) } : {}),
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
