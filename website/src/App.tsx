import { useEffect, useRef, useState } from "react";
import {
  ArrowDown,
  ArrowRight,
  ArrowUpRight,
  Check,
  ChevronDown,
  CursorEditor,
  Dictionary,
  LockKeyhole,
  Mail,
  Menu,
  MessageCircle,
  Notion,
  Slack,
  SnippetArrow,
  VisualStudioCode,
  Wechat,
  X,
} from "./Icons";
import { content, links, type Locale } from "./content";
import { AppleIcon, GitHubIcon, VoiceMark } from "./Icons";
import Demo from "./Demo";
import brandIcon from "./assets/voiceflow.svg";

const localeKey = "voiceflow-website-locale";

function readLocale(): Locale {
  try {
    return localStorage.getItem(localeKey) === "en" ? "en" : "zh";
  } catch {
    return "zh";
  }
}

function DownloadLink({
  locale,
  className = "",
  compact = false,
}: {
  locale: Locale;
  className?: string;
  compact?: boolean;
}) {
  const copy = content[locale];
  return (
    <a className={`button button-primary ${className}`} href={links.download} aria-label={compact ? copy.nav.download : copy.hero.download}>
      <AppleIcon size={20} />
      <span>{compact ? copy.nav.download : copy.hero.download}</span>
      {!compact && <ArrowUpRight size={18} />}
    </a>
  );
}

function Brand({ locale }: { locale: Locale }) {
  return (
    <a href="#top" className="brand" aria-label={content[locale].nav.home}>
      <img src={brandIcon} width="34" height="34" alt="" />
      <span>VoiceFlow</span>
    </a>
  );
}

function Header({
  locale,
  onLocaleChange,
}: {
  locale: Locale;
  onLocaleChange: () => void;
}) {
  const copy = content[locale];
  const [menuOpen, setMenuOpen] = useState(false);
  const [mobileNavigation, setMobileNavigation] = useState(
    () => window.matchMedia("(max-width: 760px)").matches,
  );
  const headerRef = useRef<HTMLElement>(null);
  const menuRef = useRef<HTMLButtonElement>(null);
  const navigationRef = useRef<HTMLElement>(null);

  useEffect(() => {
    const media = window.matchMedia("(max-width: 760px)");
    function onChange() {
      setMobileNavigation(media.matches);
      if (!media.matches) setMenuOpen(false);
    }
    media.addEventListener("change", onChange);
    onChange();
    return () => media.removeEventListener("change", onChange);
  }, []);

  useEffect(() => {
    if (!menuOpen) return;
    navigationRef.current?.querySelector("a")?.focus({ preventScroll: true });
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        setMenuOpen(false);
        menuRef.current?.focus({ preventScroll: true });
      }
    }
    function onPointerDown(event: PointerEvent) {
      if (event.target instanceof Node && !headerRef.current?.contains(event.target)) setMenuOpen(false);
    }
    function onFocusIn(event: FocusEvent) {
      if (event.target instanceof Node && !navigationRef.current?.contains(event.target) && event.target !== menuRef.current) setMenuOpen(false);
    }
    window.addEventListener("keydown", onKeyDown);
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("focusin", onFocusIn);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("focusin", onFocusIn);
    };
  }, [menuOpen]);

  return (
    <header ref={headerRef} className="site-header">
      <div className="header-inner container">
        <Brand locale={locale} />
        <nav
          ref={navigationRef}
          className={`main-nav ${menuOpen ? "is-open" : ""}`}
          id="main-navigation"
          aria-label={copy.nav.label}
          aria-hidden={mobileNavigation && !menuOpen ? true : undefined}
          inert={mobileNavigation && !menuOpen}
        >
          <a href="#features" onClick={() => { setMenuOpen(false); if (mobileNavigation) menuRef.current?.focus({ preventScroll: true }); }}>
            {copy.nav.features}
          </a>
          <a href="#how-it-works" onClick={() => { setMenuOpen(false); if (mobileNavigation) menuRef.current?.focus({ preventScroll: true }); }}>
            {copy.nav.how}
          </a>
          <a href="#privacy" onClick={() => { setMenuOpen(false); if (mobileNavigation) menuRef.current?.focus({ preventScroll: true }); }}>
            {copy.nav.privacy}
          </a>
        </nav>
        <div className="header-actions">
          <button
            className="language-button"
            type="button"
            lang={locale === "zh" ? "en" : "zh-CN"}
            onClick={() => {
              setMenuOpen(false);
              onLocaleChange();
            }}
            aria-label={copy.nav.language}
          >
            <span>{locale === "zh" ? "EN" : "中"}</span>
            <span className="language-slash" aria-hidden="true">
              /
            </span>
            <span className="language-current" aria-hidden="true">
              {locale === "zh" ? "中" : "EN"}
            </span>
          </button>
          <DownloadLink locale={locale} compact className="header-download" />
          <button
            ref={menuRef}
            type="button"
            className="menu-button"
            aria-label={menuOpen ? copy.nav.close : copy.nav.open}
            aria-expanded={menuOpen}
            aria-controls="main-navigation"
            onClick={() => setMenuOpen(!menuOpen)}
          >
            {menuOpen ? <X size={20} /> : <Menu size={20} />}
          </button>
        </div>
      </div>
    </header>
  );
}

function AppNames({ locale }: { locale: Locale }) {
  const copy = content[locale].apps;
  return (
    <section className="apps-section container" aria-label={copy.heading}>
      <p className="apps-heading">{copy.heading}</p>
      <div className="app-names">
        <span>
          <CursorEditor size={24} />
          Cursor
        </span>
        <span>
          <Notion size={24} />
          Notion
        </span>
        <span>
          <Slack size={24} />
          Slack
        </span>
        <span>
          <Mail size={24} />
          {copy.mail}
        </span>
        <span>
          <VisualStudioCode size={24} />
          VS Code
        </span>
        <span>
          <Wechat size={24} />
          {copy.wechat}
        </span>
      </div>
      <p className="apps-note">{copy.note}</p>
    </section>
  );
}

export default function App() {
  const [locale, setLocale] = useState<Locale>(readLocale);
  const rootRef = useRef<HTMLDivElement>(null);
  const copy = content[locale];

  useEffect(() => {
    const elements =
      rootRef.current?.querySelectorAll<HTMLElement>("[data-reveal]");
    if (!elements) return;
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (!entry.isIntersecting) continue;
          entry.target.classList.add("is-revealed");
          observer.unobserve(entry.target);
        }
      },
      { threshold: 0.12 },
    );
    for (const element of elements) {
      element.classList.add("reveal-ready");
      if (!element.classList.contains("is-revealed")) observer.observe(element);
    }
    return () => observer.disconnect();
  }, [locale]);

  useEffect(() => {
    document.documentElement.lang = locale === "zh" ? "zh-CN" : "en";
    document.title = copy.meta.title;
    document
      .querySelector('meta[name="description"]')
      ?.setAttribute("content", copy.meta.description);
    try {
      localStorage.setItem(localeKey, locale);
    } catch {
      // Language switching still works when persistent storage is unavailable.
    }
  }, [locale, copy.meta.title, copy.meta.description]);

  return (
    <div className="site" id="top" ref={rootRef}>
      <a className="skip-link" href="#main-content">
        {copy.nav.skip}
      </a>
      <Header
        locale={locale}
        onLocaleChange={() => setLocale(locale === "zh" ? "en" : "zh")}
      />
      <main id="main-content" tabIndex={-1}>
        <section className="hero container" aria-labelledby="hero-title">
          <p className="eyebrow hero-eyebrow">
            {copy.hero.eyebrow}
          </p>
          <h1 id="hero-title">
            <span>{copy.hero.title[0]}</span>
            <span className="hero-title-muted">{copy.hero.title[1]}</span>
          </h1>
          <p className="hero-description">{copy.hero.description}</p>
          <div className="hero-actions">
            <DownloadLink locale={locale} />
            <a className="button button-text" href="#demo">
              {copy.hero.demo}
              <ArrowDown size={16} aria-hidden="true" />
            </a>
          </div>
          <p className="compatibility">{copy.hero.compatibility}</p>
        </section>

        <Demo locale={locale} />
        <AppNames locale={locale} />

        <section
          className="how-section container section-space"
          id="how-it-works"
          aria-labelledby="how-title"
        >
          <div className="how-intro">
            <p className="eyebrow">{copy.how.eyebrow}</p>
            <h2 id="how-title">
              {copy.how.title[0]}
              <br />
              <span className="muted-heading">{copy.how.title[1]}</span>
            </h2>
            <p className="section-description">{copy.how.description}</p>
            <div className="shortcut-keys" aria-label={copy.how.shortcutLabel}>
              <kbd>⌘</kbd>
              <kbd>⌥</kbd>
              <kbd className="space-key">space</kbd>
            </div>
            <p className="shortcut-note">{copy.how.shortcut}</p>
          </div>
          <ol className="how-steps">
            {copy.how.steps.map((step, index) => (
              <li key={index}>
                <span className="step-number">0{index + 1}</span>
                <div>
                  <h3>{step.title}</h3>
                  <p>{step.description}</p>
                </div>
              </li>
            ))}
          </ol>
        </section>

        <section
          className="features-section container section-space"
          id="features"
          aria-labelledby="features-title"
        >
          <div className="section-heading">
            <p className="eyebrow">{copy.features.eyebrow}</p>
            <h2 id="features-title">
              {copy.features.title[0]}
              <span className="muted-heading">{copy.features.title[1]}</span>
            </h2>
            <p className="section-description">{copy.features.description}</p>
          </div>

          <article className="feature-story">
            <div className="feature-visual cleanup-visual" data-reveal="visual">
              <VoiceMark className="cleanup-watermark" />
              <div className="cleanup-original">
                <span className="mini-label">
                  {copy.features.cleanup.before}
                </span>
                <p>{copy.features.cleanup.original}</p>
              </div>
              <div className="cleanup-divider">
                <span />
                <ArrowDown size={18} aria-hidden="true" />
                <span />
              </div>
              <div className="cleanup-result">
                <span className="mini-label">
                  {copy.features.cleanup.after}
                </span>
                <p>{copy.features.cleanup.polished}</p>
              </div>
              <span className="feature-badge">
                <Check size={16} aria-hidden="true" />
                {copy.features.cleanup.badge}
              </span>
            </div>
            <div className="feature-copy" data-reveal="copy">
              <p className="eyebrow">{copy.features.cleanup.eyebrow}</p>
              <h3>
                {copy.features.cleanup.title[0]}
                <br />
                {copy.features.cleanup.title[1]}
              </h3>
              <p>{copy.features.cleanup.description}</p>
            </div>
            <p className="feature-note">{copy.features.cleanup.note}</p>
          </article>

          <article className="feature-story feature-story-reverse">
            <div
              className="feature-visual dictionary-visual"
              data-reveal="visual"
            >
              <span
                className="floating-word floating-word-one"
                aria-hidden="true"
              >
                TypeScript
              </span>
              <span
                className="floating-word floating-word-two"
                aria-hidden="true"
              >
                VoiceFlow
              </span>
              <div className="dictionary-sheet">
                <div className="dictionary-heading">
                  <span>{copy.features.dictionary.label}</span>
                  <Dictionary size={20} className="dictionary-symbol" />
                </div>
                <p className="dictionary-subtitle">
                  {copy.features.dictionary.add}
                </p>
                <ul>
                  {copy.features.dictionary.terms.map((term) => (
                    <li key={term}>
                      <span>{term}</span>
                      <Check size={16} aria-hidden="true" />
                    </li>
                  ))}
                </ul>
                <p className="dictionary-footer">
                  {copy.features.dictionary.footer}
                </p>
              </div>
            </div>
            <div className="feature-copy" data-reveal="copy">
              <p className="eyebrow">{copy.features.dictionary.eyebrow}</p>
              <h3>
                {copy.features.dictionary.title[0]}
                <br />
                {copy.features.dictionary.title[1]}
              </h3>
              <p>{copy.features.dictionary.description}</p>
            </div>
            <p className="feature-note">{copy.features.dictionary.note}</p>
          </article>

          <article className="feature-story">
            <div
              className="feature-visual snippets-visual"
              data-reveal="visual"
            >
              <div className="snippet-phrase">
                <span className="mini-label">
                  {copy.features.snippets.label}
                </span>
                <p>
                  <MessageCircle size={20} aria-hidden="true" />
                  {copy.features.snippets.phrase}
                </p>
              </div>
              <div className="snippet-expansion">
                <span className="mini-label">
                  {copy.features.snippets.expanded}
                </span>
                <p>{copy.features.snippets.expansion}</p>
                <VoiceMark className="signature-mark" />
              </div>
              <SnippetArrow size={32} className="snippet-connector" />
            </div>
            <div className="feature-copy" data-reveal="copy">
              <p className="eyebrow">{copy.features.snippets.eyebrow}</p>
              <h3>
                {copy.features.snippets.title[0]}
                <br />
                {copy.features.snippets.title[1]}
              </h3>
              <p>{copy.features.snippets.description}</p>
            </div>
            <p className="feature-note">{copy.features.snippets.note}</p>
          </article>
        </section>

        <section
          className="privacy-section"
          id="privacy"
          aria-labelledby="privacy-title"
        >
          <div className="privacy-inner container">
            <div className="privacy-intro">
              <LockKeyhole
                size={26}
                className="privacy-lock"
                aria-hidden="true"
              />
              <p className="eyebrow">{copy.privacy.eyebrow}</p>
              <h2 id="privacy-title">
                {copy.privacy.title[0]}
                <br />
                <span>{copy.privacy.title[1]}</span>
              </h2>
              <p>{copy.privacy.description}</p>
              <a
                className="inline-link"
                href={links.privacy}
                target="_blank"
                rel="noreferrer"
              >
                {copy.privacy.link}
                <ArrowUpRight size={16} aria-hidden="true" />
              </a>
            </div>
            <div className="privacy-points">
              {copy.privacy.points.map((point, index) => (
                <div key={index}>
                  <span className="privacy-number">0{index + 1}</span>
                  <h3>{point.title}</h3>
                  <p>{point.description}</p>
                </div>
              ))}
            </div>
          </div>
        </section>

        <section
          className="faq-section container section-space"
          aria-labelledby="faq-title"
        >
          <div>
            <p className="eyebrow">{copy.faq.eyebrow}</p>
            <h2 id="faq-title">{copy.faq.title}</h2>
            <p className="section-description">{copy.faq.description}</p>
          </div>
          <div className="faq-list">
            {copy.faq.items.map((item, index) => (
              <details key={index}>
                <summary>
                  <span>{item.question}</span>
                  <ChevronDown size={20} aria-hidden="true" />
                </summary>
                <div className="faq-answer">
                  <p>{item.answer}</p>
                </div>
              </details>
            ))}
          </div>
        </section>

        <section className="closing-section" aria-labelledby="closing-title">
          <VoiceMark className="closing-wave closing-wave-left" />
          <VoiceMark className="closing-wave closing-wave-right" />
          <div className="closing-content container">
            <p className="eyebrow">{copy.closing.eyebrow}</p>
            <h2 id="closing-title">{copy.closing.title}</h2>
            <p>{copy.closing.description}</p>
            <DownloadLink locale={locale} />
            <a
              className="closing-github inline-link"
              href={links.github}
              target="_blank"
              rel="noreferrer"
            >
              {copy.closing.github}
              <ArrowRight size={16} aria-hidden="true" />
            </a>
          </div>
        </section>
      </main>

      <footer className="site-footer container">
        <div className="footer-top">
          <div className="footer-brand">
            <Brand locale={locale} />
            <p>{copy.footer.description}</p>
          </div>
          <nav className="footer-column" aria-label={copy.footer.product}>
            <span className="mini-label">{copy.footer.product}</span>
            <a href="#features">{copy.nav.features}</a>
            <a href="#how-it-works">{copy.nav.how}</a>
            <a href={links.download}>{copy.nav.download}</a>
          </nav>
          <nav className="footer-column" aria-label={copy.footer.resources}>
            <span className="mini-label">{copy.footer.resources}</span>
            <a href={links.guide} target="_blank" rel="noreferrer">
              {copy.footer.guide}
              <ArrowUpRight size={16} aria-hidden="true" />
            </a>
            <a href={links.releases} target="_blank" rel="noreferrer">
              {copy.footer.releases}
              <ArrowUpRight size={16} aria-hidden="true" />
            </a>
            <a href={links.privacy} target="_blank" rel="noreferrer">
              {copy.nav.privacy}
              <ArrowUpRight size={16} aria-hidden="true" />
            </a>
          </nav>
        </div>
        <div className="footer-bottom">
          <span>{copy.footer.copyright}</span>
          <span>{copy.footer.made}</span>
          <a
            href={links.github}
            target="_blank"
            rel="noreferrer"
            aria-label="GitHub"
          >
            <GitHubIcon size={20} />
          </a>
        </div>
      </footer>
    </div>
  );
}
