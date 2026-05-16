// Markdown -> sanitized HTML -> DocumentFragment, with Shiki code
// highlighting wired in on the browser side.
//
// Shape:
//   - `renderMarkdownHtml(text)` returns the raw markdown-it output. Used
//     by tests and any caller that wants the HTML string. No Shiki, no
//     DOMPurify -- safe to call from Node.
//   - `renderMarkdownFragment(text)` returns a sanitized DocumentFragment
//     with Shiki-highlighted code blocks (if the highlighter is ready).
//     Browser-only.
//   - `initShiki()` kicks off the async highlighter init. The webview
//     calls this on boot and re-renders when it resolves so previously-
//     plain fences pick up colors.

import DOMPurify from "dompurify";
import MarkdownIt from "markdown-it";
import {
  createHighlighter,
  type BundledLanguage,
  type BundledTheme,
  type Highlighter,
} from "shiki";

const markdown = MarkdownIt({
  html: false,
  breaks: false,
  linkify: false,
  typographer: false,
});

markdown.renderer.rules.softbreak = () => "\n";
markdown.renderer.rules.hardbreak = () => "<br>";

const ALLOWED_TAGS = [
  "a",
  "blockquote",
  "br",
  "code",
  "del",
  "em",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "hr",
  "li",
  "ol",
  "p",
  "pre",
  "span",
  "strong",
  "table",
  "tbody",
  "td",
  "th",
  "thead",
  "tr",
  "ul",
];

// `class` is allowed so Shiki's token classes survive sanitization.
//
// `style` is NOT allowed: DOMPurify's CSS scrubber only blocks
// javascript: / expression() payloads, not arbitrary CSS, so a
// hostile LLM markdown response could inject CSS like
//   position: fixed; inset: 0; background: black;
// to overlay the panel or repaint disabled buttons as enabled
// (chat-panel audit #4, 2026-05-16). Shiki's color output is grafted
// in AFTER DOMPurify by applyShikiHighlight (see code-block path
// below), so the sanitizer doesn't need to preserve `style` for
// assistant text.
const ALLOWED_ATTR = ["href", "target", "rel", "class", "tabindex"];

let purifyHooksInstalled = false;
function ensurePurifyHooks(): void {
  if (purifyHooksInstalled) {
    return;
  }
  purifyHooksInstalled = true;
  DOMPurify.addHook("afterSanitizeAttributes", (node) => {
    if (!(node instanceof Element)) {
      return;
    }
    if (node.tagName !== "A") {
      return;
    }
    const href = node.getAttribute("href");
    if (!href || !isSafeMarkdownHref(href)) {
      node.removeAttribute("href");
      node.removeAttribute("target");
      node.removeAttribute("rel");
      return;
    }
    node.setAttribute("target", "_blank");
    node.setAttribute("rel", "noreferrer noopener");
  });
}

export function renderMarkdownHtml(text: string): string {
  return markdown.render(normalizeMarkdownInput(text));
}

export function renderMarkdownFragment(text: string): DocumentFragment {
  ensurePurifyHooks();
  const html = renderMarkdownHtml(text);
  const sanitized = DOMPurify.sanitize(html, {
    ALLOWED_TAGS,
    ALLOWED_ATTR,
    RETURN_DOM_FRAGMENT: true,
  }) as unknown as DocumentFragment;
  applyShikiHighlight(sanitized);
  return sanitized;
}

// Pre-bundled language + theme set. Add languages here as they come up
// in chat transcripts; each entry pulls a small grammar JSON into the
// webview bundle.
const SHIKI_LANGS: BundledLanguage[] = [
  "bash",
  "c",
  "cpp",
  "css",
  "diff",
  "dockerfile",
  "go",
  "html",
  "java",
  "javascript",
  "json",
  "make",
  "markdown",
  "python",
  "rust",
  "shellscript",
  "sql",
  "system-verilog",
  "toml",
  "tsx",
  "typescript",
  "verilog",
  "yaml",
];
const SHIKI_THEMES: BundledTheme[] = ["github-dark", "github-light"];

let highlighter: Highlighter | null = null;
let highlighterPromise: Promise<Highlighter> | null = null;

/**
 * Kick off the Shiki highlighter once. Returns the same promise on
 * subsequent calls. The webview awaits this at boot and triggers a
 * re-render when it resolves so any code blocks rendered as plain
 * `<pre><code>` get repainted with token colors.
 */
export function initShiki(): Promise<Highlighter> {
  if (highlighterPromise) {
    return highlighterPromise;
  }
  highlighterPromise = createHighlighter({
    themes: SHIKI_THEMES,
    langs: SHIKI_LANGS,
  }).then((hl) => {
    highlighter = hl;
    return hl;
  });
  return highlighterPromise;
}

function applyShikiHighlight(root: ParentNode): void {
  if (!highlighter) {
    return;
  }
  const isDark =
    document.body.classList.contains("vscode-dark") ||
    document.body.classList.contains("vscode-high-contrast");
  const theme: BundledTheme = isDark ? "github-dark" : "github-light";
  const blocks = root.querySelectorAll("pre > code");
  for (const code of Array.from(blocks)) {
    // Prefer an explicit `language-X` class on the <code> element
    // (set by markdown-it for ```rust fences); fall back to a
    // content-based guess for fences that arrived without a tag,
    // or with the explicit but uninformative "text" tag (which
    // upstream callers use when they couldn't decide a language).
    const explicit = inferLang(code as HTMLElement);
    const lang =
      explicit && explicit !== "text"
        ? explicit
        : inferLangFromContent(code.textContent ?? "");
    if (!lang || !SHIKI_LANGS.includes(lang as BundledLanguage)) {
      continue;
    }
    const src = code.textContent ?? "";
    let html: string;
    try {
      html = highlighter.codeToHtml(src, {
        lang: lang as BundledLanguage,
        theme,
      });
    } catch {
      continue;
    }
    const pre = code.parentElement;
    if (!pre) {
      continue;
    }
    // Run Shiki's output through DOMPurify before grafting into the
    // live DOM. Shiki escapes its inputs and emits a tight set of
    // `<pre><code><span class="...">` tags, so this is defense in
    // depth -- but the prior code path (innerHTML + replaceWith)
    // skipped the sanitizer entirely, so any future Shiki regression
    // or a bundled-language plugin with HTML injection would land
    // straight in the panel DOM. Keep `style` allowed on Shiki's
    // per-token spans (carry the syntax color), even though the
    // outer sanitizer drops it for assistant text. See chat-panel
    // audit #5 (2026-05-16).
    const sanitized = DOMPurify.sanitize(html, {
      ALLOWED_TAGS: ["pre", "code", "span"],
      ALLOWED_ATTR: ["class", "style"],
    });
    const tpl = document.createElement("template");
    tpl.innerHTML = sanitized;
    const replacement = tpl.content.firstElementChild;
    if (!replacement) {
      continue;
    }
    // Shiki emits inline `background-color` + `color` on the <pre>
    // that bakes in its own theme background. Drop those so the
    // bubble's existing dark-on-light styling shows through; keep the
    // per-token span colors, which carry the actual syntax info.
    replacement.removeAttribute("style");
    pre.replaceWith(replacement);
  }
}

function inferLang(code: HTMLElement): string | null {
  for (const cls of Array.from(code.classList)) {
    if (cls.startsWith("language-")) {
      return cls.slice("language-".length);
    }
  }
  return null;
}

/**
 * Best-effort language guess for unlabeled code blocks. Looks at the
 * first ~4KB of content for syntactic markers. Returns null when no
 * pattern hits cleanly; callers should treat that as "leave plain".
 * Order matters -- check the more specific markers (Verilog modules,
 * Rust `fn`/`use::`) before the looser JS/TS patterns that would
 * otherwise swallow them. Exported so the chat panel can label
 * tool-result bodies that arrive as plain text.
 */
export function inferLangFromContent(text: string): string | null {
  const sample = text.length > 4000 ? text.slice(0, 4000) : text;
  if (
    /\bfn\s+[\w_]+\s*[<(]/.test(sample) ||
    /\buse\s+[\w:]+::/.test(sample) ||
    /\bpub\s+(fn|struct|enum|mod|trait)\b/.test(sample) ||
    /\bimpl\s+(<.+>\s+)?[\w_]+(\s+for\s+[\w_]+)?\s*\{/.test(sample)
  ) {
    return "rust";
  }
  if (
    /^\s*module\s+[\w_]+\s*[#(]/m.test(sample) ||
    /\bendmodule\b/.test(sample) ||
    /^\s*always(_(ff|comb|latch))?\s*@/m.test(sample)
  ) {
    return "system-verilog";
  }
  if (
    /^\s*def\s+[\w_]+\s*\(/m.test(sample) ||
    /^\s*from\s+[\w.]+\s+import\b/m.test(sample) ||
    /^\s*class\s+[\w_]+\s*[(:]/m.test(sample)
  ) {
    return "python";
  }
  if (
    /\binterface\s+[\w_]+/.test(sample) ||
    /\btype\s+[\w_]+\s*=\s*[\{<]/.test(sample) ||
    /:\s*[\w_]+(\[\])?\s*[,;)]/.test(sample)
  ) {
    return "typescript";
  }
  if (
    /\bfunction\s+[\w_]+\s*\(/.test(sample) ||
    /\bconst\s+[\w_]+\s*=/.test(sample) ||
    /=>\s*[\{\(]/.test(sample)
  ) {
    return "javascript";
  }
  if (/^#!\/.*\b(ba)?sh\b/.test(sample) || /\$\([^)]+\)/.test(sample)) {
    return "bash";
  }
  if (/^\s*\{[\s\S]*"[\w-]+"\s*:/.test(sample)) {
    return "json";
  }
  if (/^\[[\w.]+\]/m.test(sample) && /^[\w-]+\s*=\s*/m.test(sample)) {
    return "toml";
  }
  return null;
}

export function isSafeMarkdownHref(href: string): boolean {
  if (href.startsWith("#")) {
    return true;
  }
  try {
    const url = new URL(href, "https://sim-flow.invalid");
    if (url.origin === "https://sim-flow.invalid" && !href.includes(":")) {
      return false;
    }
    return url.protocol === "http:" || url.protocol === "https:" || url.protocol === "mailto:";
  } catch {
    return false;
  }
}

function normalizeMarkdownInput(text: string): string {
  return text.replace(/\r\n?/g, "\n");
}
