import { describe, expect, it } from "vitest";

import { isSafeMarkdownHref, renderMarkdownHtml } from "./renderMarkdown";

describe("chatPanel/renderMarkdown", () => {
  it("renders bold text with preserved hard line breaks", () => {
    expect(renderMarkdownHtml("Hello **world**\nthere")).toBe(
      "<p>Hello <strong>world</strong>\nthere</p>\n",
    );
  });

  it("renders fenced code blocks via markdown-it", () => {
    expect(renderMarkdownHtml("```rs\nfn main() {}\n```")).toBe(
      '<pre><code class="language-rs">fn main() {}\n</code></pre>\n',
    );
  });

  it("rejects unsafe protocols", () => {
    expect(isSafeMarkdownHref("javascript:alert(1)")).toBe(false);
    expect(isSafeMarkdownHref("file:///tmp/nope")).toBe(false);
  });

  it("allows safe http, https, and mailto links", () => {
    expect(isSafeMarkdownHref("https://example.com")).toBe(true);
    expect(isSafeMarkdownHref("http://example.com")).toBe(true);
    expect(isSafeMarkdownHref("mailto:test@example.com")).toBe(true);
  });

  it("renders markdown tables", () => {
    expect(renderMarkdownHtml("| a | b |\n| - | - |\n| 1 | 2 |")).toContain("<table>");
    expect(renderMarkdownHtml("| a | b |\n| - | - |\n| 1 | 2 |")).toContain("<td>1</td>");
  });
});
