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
  }

  export interface MarkdownItInstance {
    render(src: string): string;
    renderer: MarkdownRenderer;
  }

  export default function MarkdownIt(options?: MarkdownItOptions): MarkdownItInstance;
}
