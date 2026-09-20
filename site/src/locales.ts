import { HOME_CONTENT, type HomeContent } from "./home-locales";

export interface SiteUi {
  readonly overview: string;
  readonly docs: string;
  readonly releases: string;
  readonly search: string;
  readonly searchPlaceholder: string;
  readonly noResults: string;
  readonly language: string;
  readonly appearance: string;
  readonly menu: string;
  readonly documentation: string;
  readonly onThisPage: string;
  readonly previous: string;
  readonly next: string;
  readonly lastUpdated: string;
  readonly englishOnly: string;
  readonly preRelease: string;
  readonly getStarted: string;
  readonly download: string;
  readonly primaryNavigation: string;
  readonly home: string;
  readonly pagination: string;
}

export interface SiteLocale {
  readonly path: string;
  readonly lang: string;
  readonly label: string;
  readonly dir?: "rtl";
  readonly ui: SiteUi;
  readonly home: HomeContent;
}

const base: SiteUi = {
  overview: "Overview", docs: "Docs", releases: "Releases", search: "Search",
  searchPlaceholder: "Search pages and headings", noResults: "No results found",
  language: "Language", appearance: "Appearance", menu: "Menu",
  documentation: "Documentation", onThisPage: "On this page", previous: "Previous",
  next: "Next", lastUpdated: "Last updated",
  englishOnly: "This technical documentation is currently available in English only.",
  preRelease: "Pre-release", getStarted: "Get started", download: "Download v0.3.0",
  primaryNavigation: "Primary navigation", home: "Kura home", pagination: "Pagination",
};

function ui(values: Partial<SiteUi>): SiteUi { return { ...base, ...values }; }

export const SITE_LOCALES: readonly SiteLocale[] = [
  { path: "en", lang: "en", label: "English", ui: base, home: HOME_CONTENT.en },
  { path: "", lang: "zh-Hans", label: "简体中文", ui: ui({ overview: "概览", docs: "文档", releases: "版本", search: "搜索", searchPlaceholder: "搜索页面和标题", noResults: "没有找到结果", language: "语言", appearance: "外观", menu: "菜单", documentation: "文档", onThisPage: "本页内容", previous: "上一页", next: "下一页", lastUpdated: "最后更新", englishOnly: "本技术文档目前仅提供英文版本。", preRelease: "预发布", getStarted: "开始使用", download: "下载 v0.3.0", primaryNavigation: "主导航", home: "Kura 首页", pagination: "分页导航" }), home: HOME_CONTENT["zh-Hans"] },
  { path: "zh-Hant", lang: "zh-Hant", label: "繁體中文", ui: ui({ overview: "概覽", docs: "文件", releases: "版本", search: "搜尋", searchPlaceholder: "搜尋頁面與標題", noResults: "找不到結果", language: "語言", appearance: "外觀", menu: "選單", documentation: "文件", onThisPage: "本頁內容", previous: "上一頁", next: "下一頁", lastUpdated: "最後更新", englishOnly: "本技術文件目前僅提供英文版本。", preRelease: "預發佈", getStarted: "開始使用", download: "下載 v0.3.0", primaryNavigation: "主導覽", home: "Kura 首頁", pagination: "分頁導覽" }), home: HOME_CONTENT["zh-Hant"] },
  { path: "es", lang: "es", label: "Español", ui: ui({ overview: "Resumen", docs: "Documentación", releases: "Versiones", search: "Buscar", searchPlaceholder: "Buscar páginas y títulos", noResults: "No se encontraron resultados", language: "Idioma", appearance: "Apariencia", menu: "Menú", documentation: "Documentación", onThisPage: "En esta página", previous: "Anterior", next: "Siguiente", lastUpdated: "Última actualización", englishOnly: "Esta documentación técnica solo está disponible en inglés.", preRelease: "Versión preliminar", getStarted: "Comenzar", download: "Descargar v0.3.0", primaryNavigation: "Navegación principal", home: "Inicio de Kura", pagination: "Paginación" }), home: HOME_CONTENT.es },
  { path: "fr", lang: "fr", label: "Français", ui: ui({ overview: "Aperçu", docs: "Documentation", releases: "Versions", search: "Rechercher", searchPlaceholder: "Rechercher des pages et titres", noResults: "Aucun résultat", language: "Langue", appearance: "Apparence", menu: "Menu", onThisPage: "Sur cette page", previous: "Précédent", next: "Suivant", lastUpdated: "Dernière mise à jour", englishOnly: "Cette documentation technique est actuellement disponible uniquement en anglais.", preRelease: "Préversion", getStarted: "Commencer", download: "Télécharger v0.3.0", primaryNavigation: "Navigation principale", home: "Accueil Kura", pagination: "Pagination" }), home: HOME_CONTENT.fr },
  { path: "de", lang: "de", label: "Deutsch", ui: ui({ overview: "Übersicht", docs: "Dokumentation", releases: "Versionen", search: "Suchen", searchPlaceholder: "Seiten und Überschriften durchsuchen", noResults: "Keine Ergebnisse", language: "Sprache", appearance: "Darstellung", menu: "Menü", documentation: "Dokumentation", onThisPage: "Auf dieser Seite", previous: "Zurück", next: "Weiter", lastUpdated: "Zuletzt aktualisiert", englishOnly: "Diese technische Dokumentation ist derzeit nur auf Englisch verfügbar.", preRelease: "Vorabversion", getStarted: "Loslegen", download: "v0.3.0 herunterladen", primaryNavigation: "Hauptnavigation", home: "Kura-Startseite", pagination: "Seitennavigation" }), home: HOME_CONTENT.de },
  { path: "ru", lang: "ru", label: "Русский", ui: ui({ overview: "Обзор", docs: "Документация", releases: "Релизы", search: "Поиск", searchPlaceholder: "Поиск страниц и заголовков", noResults: "Ничего не найдено", language: "Язык", appearance: "Оформление", menu: "Меню", documentation: "Документация", onThisPage: "На этой странице", previous: "Назад", next: "Далее", lastUpdated: "Обновлено", englishOnly: "Техническая документация пока доступна только на английском языке.", preRelease: "Предварительная версия", getStarted: "Начать", download: "Скачать v0.3.0", primaryNavigation: "Основная навигация", home: "Главная Kura", pagination: "Навигация по страницам" }), home: HOME_CONTENT.ru },
  { path: "he", lang: "he", label: "עברית", dir: "rtl", ui: ui({ overview: "סקירה", docs: "תיעוד", releases: "גרסאות", search: "חיפוש", searchPlaceholder: "חיפוש בדפים ובכותרות", noResults: "לא נמצאו תוצאות", language: "שפה", appearance: "מראה", menu: "תפריט", documentation: "תיעוד", onThisPage: "בעמוד זה", previous: "הקודם", next: "הבא", lastUpdated: "עודכן לאחרונה", englishOnly: "התיעוד הטכני זמין כרגע באנגלית בלבד.", preRelease: "גרסה מוקדמת", getStarted: "התחלה", download: "הורדת v0.3.0", primaryNavigation: "ניווט ראשי", home: "דף הבית של Kura", pagination: "ניווט בין עמודים" }), home: HOME_CONTENT.he },
  { path: "ar", lang: "ar", label: "العربية", dir: "rtl", ui: ui({ overview: "نظرة عامة", docs: "الوثائق", releases: "الإصدارات", search: "بحث", searchPlaceholder: "البحث في الصفحات والعناوين", noResults: "لا توجد نتائج", language: "اللغة", appearance: "المظهر", menu: "القائمة", documentation: "الوثائق", onThisPage: "في هذه الصفحة", previous: "السابق", next: "التالي", lastUpdated: "آخر تحديث", englishOnly: "هذه الوثائق التقنية متاحة حاليًا باللغة الإنجليزية فقط.", preRelease: "إصدار تجريبي", getStarted: "ابدأ", download: "تنزيل v0.3.0", primaryNavigation: "التنقل الرئيسي", home: "صفحة Kura الرئيسية", pagination: "التنقل بين الصفحات" }), home: HOME_CONTENT.ar },
  { path: "ja", lang: "ja", label: "日本語", ui: ui({ overview: "概要", docs: "ドキュメント", releases: "リリース", search: "検索", searchPlaceholder: "ページと見出しを検索", noResults: "結果が見つかりません", language: "言語", appearance: "外観", menu: "メニュー", documentation: "ドキュメント", onThisPage: "このページの目次", previous: "前へ", next: "次へ", lastUpdated: "最終更新", englishOnly: "技術ドキュメントは現在英語版のみです。", preRelease: "プレリリース", getStarted: "始める", download: "v0.3.0 をダウンロード", primaryNavigation: "メインナビゲーション", home: "Kura ホーム", pagination: "ページナビゲーション" }), home: HOME_CONTENT.ja },
  { path: "ko", lang: "ko", label: "한국어", ui: ui({ overview: "개요", docs: "문서", releases: "릴리스", search: "검색", searchPlaceholder: "페이지와 제목 검색", noResults: "검색 결과가 없습니다", language: "언어", appearance: "화면", menu: "메뉴", documentation: "문서", onThisPage: "이 페이지", previous: "이전", next: "다음", lastUpdated: "최근 업데이트", englishOnly: "기술 문서는 현재 영어로만 제공됩니다.", preRelease: "시험판", getStarted: "시작하기", download: "v0.3.0 다운로드", primaryNavigation: "기본 탐색", home: "Kura 홈", pagination: "페이지 탐색" }), home: HOME_CONTENT.ko },
];

export function localeForPath(path: string): SiteLocale {
  return SITE_LOCALES.find((locale) => locale.path === path) ?? SITE_LOCALES[0];
}
