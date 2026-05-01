import MarkdownIt from "markdown-it";

const markdown = MarkdownIt({
  html: false,
  breaks: false,
  linkify: false,
  typographer: false,
});

markdown.renderer.rules.softbreak = () => "\n";
markdown.renderer.rules.hardbreak = () => "<br>";

markdown.renderer.rules.link_open = (tokens, index, options, _env, self) => {
  const href = tokens[index]?.attrGet("href");
  if (!href || !isSafeMarkdownHref(href)) {
    tokens[index]?.attrSet("href", "#");
  }
  tokens[index]?.attrSet("target", "_blank");
  tokens[index]?.attrSet("rel", "noreferrer noopener");
  return self.renderToken(tokens, index, options);
};

const ALLOWED_TAGS = new Set([
  "A",
  "BLOCKQUOTE",
  "BR",
  "CODE",
  "DEL",
  "EM",
  "H1",
  "H2",
  "H3",
  "H4",
  "H5",
  "H6",
  "HR",
  "LI",
  "OL",
  "P",
  "PRE",
  "STRONG",
  "TABLE",
  "TBODY",
  "TD",
  "TH",
  "THEAD",
  "TR",
  "UL",
]);

const ALLOWED_ATTRS = new Map<string, Set<string>>([
  ["A", new Set(["href", "target", "rel"])],
  ["CODE", new Set(["class"])],
]);

export function renderMarkdownHtml(text: string): string {
  return markdown.render(normalizeMarkdownInput(text));
}

export function renderMarkdownFragment(text: string): DocumentFragment {
  const template = document.createElement("template");
  template.innerHTML = renderMarkdownHtml(text);
  sanitizeFragment(template.content);
  return template.content;
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

function sanitizeFragment(fragment: DocumentFragment): void {
  const walker = document.createTreeWalker(fragment, NodeFilter.SHOW_ELEMENT);
  const elements: Element[] = [];
  let current = walker.nextNode();
  while (current) {
    elements.push(current as Element);
    current = walker.nextNode();
  }

  for (const element of elements) {
    if (!ALLOWED_TAGS.has(element.tagName)) {
      const replacement = document.createTextNode(element.textContent ?? "");
      element.replaceWith(replacement);
      continue;
    }
    sanitizeAttributes(element);
  }
}

function sanitizeAttributes(element: Element): void {
  const allowedAttrs = ALLOWED_ATTRS.get(element.tagName) ?? new Set<string>();
  for (const attr of Array.from(element.attributes)) {
    if (!allowedAttrs.has(attr.name)) {
      element.removeAttribute(attr.name);
    }
  }
  if (element.tagName === "A") {
    const href = element.getAttribute("href");
    if (!href || !isSafeMarkdownHref(href)) {
      element.removeAttribute("href");
      element.removeAttribute("target");
      element.removeAttribute("rel");
    }
  }
}
