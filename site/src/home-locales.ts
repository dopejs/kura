export interface HomeFeature {
  readonly title: string;
  readonly body: string;
}

export interface HomeContent {
  readonly pageTitle: string;
  readonly pageDescription: string;
  readonly headlineLead: string;
  readonly headlineEmphasis: string;
  readonly headlineTail: string;
  readonly lead: string;
  readonly installLabel: string;
  readonly features: readonly HomeFeature[];
  readonly footerTagline: string;
  readonly footerCopyright: string;
}

export const HOME_CONTENT: Readonly<Record<string, HomeContent>> = {
  en: {
    pageTitle: "Kura — A Personal Agent OS",
    pageDescription: "An inspectable personal agent OS for runtime, memory, context, tools, and policy.",
    headlineLead: "A ", headlineEmphasis: "personal agent OS", headlineTail: " you can open up and read.",
    lead: "A Rust daemon that owns runtime, memory, context, and policy — with a plugin architecture where session management, retrieval, and even the embedding model are swappable parts.",
    installLabel: "Install Kura",
    features: [
      { title: "Everything is a plugin", body: "31+ built-in plugins over a small trust-boundary kernel. Disable, configure, or replace capabilities from one profile." },
      { title: "Layered memory", body: "Episodes, atoms, scenarios, and personas remain attributable, reversible, and inspectable." },
      { title: "A context engine that cites", body: "Hybrid recall and symbolic compression under budget, with a citation attached to every injected source." },
      { title: "Sessions that never forget", body: "Frame-preserving windows capture evicted spans to memory before context is compacted." },
      { title: "External plugins, any language", body: "Attach manifests and isolated processes to the hook waterfall over a line-JSON protocol." },
      { title: "Governed autonomy", body: "Skills and configuration improvements require evidence, approval, audit history, and rollback." },
    ],
    footerTagline: "Kura — an inspectable personal agent OS.", footerCopyright: "© 2026 Kura contributors",
  },
  "zh-Hans": {
    pageTitle: "Kura — 个人 Agent 操作系统",
    pageDescription: "可检查、可理解的个人 Agent 操作系统，统一管理运行时、记忆、上下文、工具与策略。",
    headlineLead: "一个可以打开并读懂的", headlineEmphasis: "个人 Agent 操作系统", headlineTail: "。",
    lead: "一个掌管运行时、记忆、上下文与策略的 Rust 守护进程。基于插件架构，会话管理、检索乃至嵌入模型都可以替换。",
    installLabel: "安装 Kura",
    features: [
      { title: "一切皆插件", body: "在小型信任边界内核之上提供 31+ 个内置插件。通过一个配置文件即可停用、配置或替换能力。" },
      { title: "分层记忆", body: "情节、原子、场景与人格始终可追溯、可撤销、可检查。" },
      { title: "会引用来源的上下文引擎", body: "在预算内完成混合召回与符号压缩，并为每个注入来源附上引用。" },
      { title: "不会遗忘的会话", body: "保留会话帧的窗口会在压缩上下文前，将被移出的内容写入记忆。" },
      { title: "任意语言的外部插件", body: "通过 line-JSON 协议，把清单文件与隔离进程接入 hook 瀑布。" },
      { title: "受治理的自主能力", body: "技能与配置改进需要证据、审批、审计记录与回滚能力。" },
    ],
    footerTagline: "Kura — 可检查的个人 Agent 操作系统。", footerCopyright: "© 2026 Kura 贡献者",
  },
  "zh-Hant": {
    pageTitle: "Kura — 個人 Agent 作業系統",
    pageDescription: "可檢查、可理解的個人 Agent 作業系統，統一管理執行階段、記憶、上下文、工具與策略。",
    headlineLead: "一個可以打開並讀懂的", headlineEmphasis: "個人 Agent 作業系統", headlineTail: "。",
    lead: "一個掌管執行階段、記憶、上下文與策略的 Rust 守護程序。基於外掛架構，工作階段管理、檢索乃至嵌入模型都可以替換。",
    installLabel: "安裝 Kura",
    features: [
      { title: "一切皆外掛", body: "在小型信任邊界核心之上提供 31+ 個內建外掛。透過一個設定檔即可停用、設定或替換能力。" },
      { title: "分層記憶", body: "情節、原子、場景與人格始終可追溯、可撤銷、可檢查。" },
      { title: "會引用來源的上下文引擎", body: "在預算內完成混合召回與符號壓縮，並為每個注入來源附上引用。" },
      { title: "不會遺忘的工作階段", body: "保留工作階段框架的視窗會在壓縮上下文前，將被移出的內容寫入記憶。" },
      { title: "任意語言的外部外掛", body: "透過 line-JSON 協定，把資訊清單與隔離程序接入 hook 瀑布。" },
      { title: "受治理的自主能力", body: "技能與設定改進需要證據、核准、稽核記錄與回復能力。" },
    ],
    footerTagline: "Kura — 可檢查的個人 Agent 作業系統。", footerCopyright: "© 2026 Kura 貢獻者",
  },
  es: {
    pageTitle: "Kura — Un sistema operativo para agentes personales",
    pageDescription: "Un sistema operativo inspeccionable para agentes personales que gestiona ejecución, memoria, contexto, herramientas y políticas.",
    headlineLead: "Un ", headlineEmphasis: "sistema operativo para agentes personales", headlineTail: " que puedes abrir y comprender.",
    lead: "Un daemon en Rust que controla la ejecución, la memoria, el contexto y las políticas, con una arquitectura de plugins donde las sesiones, la recuperación y el modelo de embeddings son reemplazables.",
    installLabel: "Instalar Kura",
    features: [
      { title: "Todo es un plugin", body: "Más de 31 plugins integrados sobre un pequeño núcleo de confianza. Desactiva, configura o reemplaza capacidades desde un solo perfil." },
      { title: "Memoria por capas", body: "Episodios, átomos, escenarios y perfiles siguen siendo atribuibles, reversibles e inspeccionables." },
      { title: "Un motor de contexto que cita", body: "Recuperación híbrida y compresión simbólica con presupuesto, citando cada fuente inyectada." },
      { title: "Sesiones que no olvidan", body: "Las ventanas conservan el marco y guardan en memoria los fragmentos expulsados antes de compactar el contexto." },
      { title: "Plugins externos en cualquier lenguaje", body: "Conecta manifiestos y procesos aislados al flujo de hooks mediante un protocolo line-JSON." },
      { title: "Autonomía gobernada", body: "Las mejoras de habilidades y configuración requieren pruebas, aprobación, auditoría y reversión." },
    ],
    footerTagline: "Kura — un sistema operativo inspeccionable para agentes personales.", footerCopyright: "© 2026 Colaboradores de Kura",
  },
  fr: {
    pageTitle: "Kura — Un système d’exploitation pour agent personnel",
    pageDescription: "Un système inspectable pour agent personnel qui gère l’exécution, la mémoire, le contexte, les outils et les règles.",
    headlineLead: "Un ", headlineEmphasis: "système d’exploitation pour agent personnel", headlineTail: " que vous pouvez ouvrir et comprendre.",
    lead: "Un démon Rust qui gère l’exécution, la mémoire, le contexte et les règles, avec une architecture de plugins où les sessions, la recherche et même le modèle d’embedding sont remplaçables.",
    installLabel: "Installer Kura",
    features: [
      { title: "Tout est un plugin", body: "Plus de 31 plugins intégrés sur un noyau à frontière de confiance réduite. Désactivez, configurez ou remplacez des capacités depuis un seul profil." },
      { title: "Mémoire en couches", body: "Épisodes, atomes, scénarios et profils restent attribuables, réversibles et inspectables." },
      { title: "Un moteur de contexte qui cite ses sources", body: "Rappel hybride et compression symbolique sous contrainte budgétaire, avec une citation pour chaque source injectée." },
      { title: "Des sessions qui n’oublient pas", body: "Les fenêtres préservent le cadre et enregistrent en mémoire les passages évincés avant la compaction du contexte." },
      { title: "Plugins externes dans tout langage", body: "Reliez manifestes et processus isolés à la cascade de hooks via un protocole line-JSON." },
      { title: "Autonomie gouvernée", body: "Les améliorations de compétences et de configuration exigent preuves, approbation, audit et retour arrière." },
    ],
    footerTagline: "Kura — un système d’exploitation inspectable pour agent personnel.", footerCopyright: "© 2026 Contributeurs de Kura",
  },
  de: {
    pageTitle: "Kura — Ein Betriebssystem für persönliche Agenten",
    pageDescription: "Ein überprüfbares Betriebssystem für persönliche Agenten, das Laufzeit, Speicher, Kontext, Werkzeuge und Richtlinien verwaltet.",
    headlineLead: "Ein ", headlineEmphasis: "Betriebssystem für persönliche Agenten", headlineTail: ", das du öffnen und verstehen kannst.",
    lead: "Ein Rust-Daemon für Laufzeit, Speicher, Kontext und Richtlinien – mit einer Plugin-Architektur, in der Sitzungsverwaltung, Suche und sogar das Embedding-Modell austauschbar sind.",
    installLabel: "Kura installieren",
    features: [
      { title: "Alles ist ein Plugin", body: "Mehr als 31 integrierte Plugins auf einem kleinen Vertrauenskern. Fähigkeiten lassen sich über ein Profil deaktivieren, konfigurieren oder ersetzen." },
      { title: "Mehrschichtiger Speicher", body: "Episoden, Atome, Szenarien und Personas bleiben zuordenbar, umkehrbar und überprüfbar." },
      { title: "Eine Kontext-Engine mit Quellen", body: "Hybrider Abruf und symbolische Komprimierung im Budget, mit einem Beleg für jede eingefügte Quelle." },
      { title: "Sitzungen, die nichts vergessen", body: "Rahmenerhaltende Fenster sichern verdrängte Abschnitte im Speicher, bevor der Kontext komprimiert wird." },
      { title: "Externe Plugins in jeder Sprache", body: "Manifeste und isolierte Prozesse werden über ein line-JSON-Protokoll an die Hook-Kaskade angebunden." },
      { title: "Geregelte Autonomie", body: "Verbesserungen an Fähigkeiten und Konfiguration benötigen Belege, Freigabe, Auditverlauf und Rollback." },
    ],
    footerTagline: "Kura — ein überprüfbares Betriebssystem für persönliche Agenten.", footerCopyright: "© 2026 Kura-Mitwirkende",
  },
  ru: {
    pageTitle: "Kura — Операционная система персонального агента",
    pageDescription: "Проверяемая ОС персонального агента для среды выполнения, памяти, контекста, инструментов и политик.",
    headlineLead: "", headlineEmphasis: "Операционная система персонального агента", headlineTail: ", которую можно открыть и понять.",
    lead: "Демон на Rust управляет средой выполнения, памятью, контекстом и политиками. Архитектура плагинов позволяет заменять управление сессиями, поиск и даже модель эмбеддингов.",
    installLabel: "Установить Kura",
    features: [
      { title: "Всё является плагином", body: "Более 31 встроенного плагина поверх небольшого доверенного ядра. Возможности можно отключать, настраивать и заменять одним профилем." },
      { title: "Многоуровневая память", body: "Эпизоды, атомы, сценарии и персоны остаются атрибутируемыми, обратимыми и проверяемыми." },
      { title: "Контекстный движок со ссылками", body: "Гибридный поиск и символьное сжатие в рамках бюджета, со ссылкой на каждый добавленный источник." },
      { title: "Сессии, которые не забывают", body: "Окна с сохранением фрейма записывают вытесненные фрагменты в память до сжатия контекста." },
      { title: "Внешние плагины на любом языке", body: "Манифесты и изолированные процессы подключаются к каскаду хуков по протоколу line-JSON." },
      { title: "Управляемая автономность", body: "Улучшения навыков и конфигурации требуют доказательств, одобрения, аудита и возможности отката." },
    ],
    footerTagline: "Kura — проверяемая ОС персонального агента.", footerCopyright: "© 2026 Участники Kura",
  },
  he: {
    pageTitle: "Kura — מערכת הפעלה לסוכן אישי",
    pageDescription: "מערכת הפעלה ניתנת לבדיקה לסוכן אישי, המנהלת זמן ריצה, זיכרון, הקשר, כלים ומדיניות.",
    headlineLead: "", headlineEmphasis: "מערכת הפעלה לסוכן אישי", headlineTail: " שאפשר לפתוח ולהבין.",
    lead: "שירות Rust שמנהל זמן ריצה, זיכרון, הקשר ומדיניות, עם ארכיטקטורת תוספים שבה אפשר להחליף ניהול הפעלות, אחזור ואפילו את מודל ההטמעה.",
    installLabel: "התקנת Kura",
    features: [
      { title: "הכול הוא תוסף", body: "יותר מ־31 תוספים מובנים מעל ליבה קטנה של גבול אמון. ניתן להשבית, להגדיר או להחליף יכולות מפרופיל אחד." },
      { title: "זיכרון בשכבות", body: "אירועים, אטומים, תרחישים ודמויות נשארים מיוחסים, הפיכים וניתנים לבדיקה." },
      { title: "מנוע הקשר שמצטט מקורות", body: "אחזור היברידי ודחיסה סמלית במסגרת התקציב, עם ציטוט לכל מקור שמוזרק." },
      { title: "הפעלות שלא שוכחות", body: "חלונות המשמרים מסגרת שומרים בזיכרון מקטעים שהוצאו לפני דחיסת ההקשר." },
      { title: "תוספים חיצוניים בכל שפה", body: "חברו מניפסטים ותהליכים מבודדים למפל ה־hooks באמצעות פרוטוקול line-JSON." },
      { title: "אוטונומיה מנוהלת", body: "שיפורי מיומנויות ותצורה דורשים ראיות, אישור, היסטוריית ביקורת ואפשרות חזרה לאחור." },
    ],
    footerTagline: "Kura — מערכת הפעלה ניתנת לבדיקה לסוכן אישי.", footerCopyright: "© 2026 תורמי Kura",
  },
  ar: {
    pageTitle: "Kura — نظام تشغيل لوكيل شخصي",
    pageDescription: "نظام تشغيل قابل للفحص لوكيل شخصي يدير وقت التشغيل والذاكرة والسياق والأدوات والسياسات.",
    headlineLead: "", headlineEmphasis: "نظام تشغيل لوكيل شخصي", headlineTail: " يمكنك فتحه وفهمه.",
    lead: "خدمة Rust تدير وقت التشغيل والذاكرة والسياق والسياسات، مع بنية إضافات يمكن فيها استبدال إدارة الجلسات والاسترجاع وحتى نموذج التضمين.",
    installLabel: "تثبيت Kura",
    features: [
      { title: "كل شيء إضافة", body: "أكثر من 31 إضافة مدمجة فوق نواة صغيرة بحدود ثقة واضحة. عطّل القدرات أو اضبطها أو استبدلها من ملف تعريف واحد." },
      { title: "ذاكرة متعددة الطبقات", body: "تبقى الحلقات والذرات والسيناريوهات والشخصيات قابلة للإسناد والتراجع والفحص." },
      { title: "محرك سياق يذكر مصادره", body: "استرجاع هجين وضغط رمزي ضمن الميزانية، مع مرجع لكل مصدر تتم إضافته." },
      { title: "جلسات لا تنسى", body: "تحفظ النوافذ ذات الإطار الثابت المقاطع المستبعدة في الذاكرة قبل ضغط السياق." },
      { title: "إضافات خارجية بأي لغة", body: "اربط ملفات التعريف والعمليات المعزولة بتسلسل hooks عبر بروتوكول line-JSON." },
      { title: "استقلالية خاضعة للحوكمة", body: "تتطلب تحسينات المهارات والإعدادات أدلة وموافقة وسجل تدقيق وإمكانية تراجع." },
    ],
    footerTagline: "Kura — نظام تشغيل قابل للفحص لوكيل شخصي.", footerCopyright: "© 2026 مساهمو Kura",
  },
  ja: {
    pageTitle: "Kura — パーソナルエージェント OS",
    pageDescription: "ランタイム、メモリ、コンテキスト、ツール、ポリシーを管理する、検査可能なパーソナルエージェント OS。",
    headlineLead: "開いて中身を理解できる", headlineEmphasis: "パーソナルエージェント OS", headlineTail: "。",
    lead: "ランタイム、メモリ、コンテキスト、ポリシーを管理する Rust デーモン。セッション管理、検索、埋め込みモデルまで交換できるプラグイン構成です。",
    installLabel: "Kura をインストール",
    features: [
      { title: "すべてがプラグイン", body: "小さな信頼境界カーネル上に 31 以上の組み込みプラグイン。1 つのプロファイルで機能を無効化、設定、交換できます。" },
      { title: "階層化メモリ", body: "エピソード、アトム、シナリオ、ペルソナは、出所を追跡でき、取り消し可能で、検査できます。" },
      { title: "出典を示すコンテキストエンジン", body: "予算内でハイブリッド検索と記号圧縮を行い、注入するすべての情報源に引用を付けます。" },
      { title: "忘れないセッション", body: "フレームを保持するウィンドウが、コンテキスト圧縮前に追い出される範囲をメモリへ保存します。" },
      { title: "任意の言語による外部プラグイン", body: "line-JSON プロトコルで、マニフェストと隔離プロセスを hook の流れへ接続します。" },
      { title: "統制された自律性", body: "スキルと設定の改善には、証拠、承認、監査履歴、ロールバックが必要です。" },
    ],
    footerTagline: "Kura — 検査可能なパーソナルエージェント OS。", footerCopyright: "© 2026 Kura contributors",
  },
  ko: {
    pageTitle: "Kura — 개인 에이전트 OS",
    pageDescription: "런타임, 메모리, 컨텍스트, 도구와 정책을 관리하는 검사 가능한 개인 에이전트 OS.",
    headlineLead: "열어서 이해할 수 있는 ", headlineEmphasis: "개인 에이전트 OS", headlineTail: "입니다.",
    lead: "런타임, 메모리, 컨텍스트와 정책을 관리하는 Rust 데몬입니다. 플러그인 구조를 통해 세션 관리, 검색, 임베딩 모델까지 교체할 수 있습니다.",
    installLabel: "Kura 설치",
    features: [
      { title: "모든 것이 플러그인", body: "작은 신뢰 경계 커널 위에 31개 이상의 내장 플러그인을 제공합니다. 하나의 프로필에서 기능을 끄고, 설정하고, 교체할 수 있습니다." },
      { title: "계층형 메모리", body: "에피소드, 원자, 시나리오와 페르소나는 출처를 추적할 수 있고, 되돌릴 수 있으며, 검사할 수 있습니다." },
      { title: "출처를 밝히는 컨텍스트 엔진", body: "예산 안에서 하이브리드 검색과 기호 압축을 수행하고, 삽입된 모든 소스에 인용을 붙입니다." },
      { title: "잊지 않는 세션", body: "프레임을 보존하는 창이 컨텍스트를 압축하기 전에 밀려나는 구간을 메모리에 저장합니다." },
      { title: "어떤 언어로도 만드는 외부 플러그인", body: "line-JSON 프로토콜을 통해 매니페스트와 격리 프로세스를 hook 흐름에 연결합니다." },
      { title: "통제되는 자율성", body: "스킬과 설정 개선에는 증거, 승인, 감사 기록과 롤백이 필요합니다." },
    ],
    footerTagline: "Kura — 검사 가능한 개인 에이전트 OS.", footerCopyright: "© 2026 Kura 기여자",
  },
};
