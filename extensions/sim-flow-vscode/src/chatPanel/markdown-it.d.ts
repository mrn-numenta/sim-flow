declare module "markdown-it" {
  export interface MarkdownToken {
    attrGet(name: string): string | null;
    attrSet(name: string, value: string): void;
  }

  export interface MarkdownRenderer {
    rules: Record<
      string,
      | ((
          tokens: MarkdownToken[],
          index: number,
          options: unknown,
          env: unknown,
          self: MarkdownRenderer,
        ) => string)
      | undefined
    >;
    renderToken(tokens: MarkdownToken[], index: number, options: unknown): string;
  }

  export interface MarkdownItOptions {
    html?: boolean;
    breaks?: boolean;
    linkify?: boolean;
    typographer?: boolean;
    /**
     * Custom code-block highlighter. Returns highlighted HTML or an
     * empty string to let markdown-it emit its default `<pre><code>`
     * wrapper. Returning a string that starts with `<pre` tells
     * markdown-it to skip its own wrapper.
     */
    highlight?: (str: string, lang: string, attrs: string) => string;
  }

  export interface MarkdownItInstance {
    render(src: string): string;
    renderer: MarkdownRenderer;
  }

  export default function MarkdownIt(options?: MarkdownItOptions): MarkdownItInstance;
}
