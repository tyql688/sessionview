/// <reference types="vite-plugin-svgr/client" />

/** Injected by Vite's `define` from package.json (see vite.config.ts). */
declare const __APP_VERSION__: string;

declare module "*?raw" {
  const content: string;
  export default content;
}

// TS 6 flags side-effect imports without declarations (TS2882).
declare module "*.css";

declare module "markdown-it-task-lists" {
  import type MarkdownIt from "markdown-it";

  interface Options {
    enabled?: boolean;
    label?: boolean;
    labelAfter?: boolean;
  }

  export default function taskLists(md: MarkdownIt, options?: Options): void;
}
