import { createSignal, createEffect, onCleanup } from "solid-js";
import { useI18n } from "../i18n/index";
import { toastError } from "../stores/toast";
import hljs from "highlight.js/lib/core";

// Register languages on demand
import javascript from "highlight.js/lib/languages/javascript";
import typescript from "highlight.js/lib/languages/typescript";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import go from "highlight.js/lib/languages/go";
import java from "highlight.js/lib/languages/java";
import cpp from "highlight.js/lib/languages/cpp";
import csharp from "highlight.js/lib/languages/csharp";
import ruby from "highlight.js/lib/languages/ruby";
import php from "highlight.js/lib/languages/php";
import swift from "highlight.js/lib/languages/swift";
import kotlin from "highlight.js/lib/languages/kotlin";
import bash from "highlight.js/lib/languages/bash";
import shell from "highlight.js/lib/languages/shell";
import sql from "highlight.js/lib/languages/sql";
import xml from "highlight.js/lib/languages/xml";
import css from "highlight.js/lib/languages/css";
import json from "highlight.js/lib/languages/json";
import yaml from "highlight.js/lib/languages/yaml";
import markdown from "highlight.js/lib/languages/markdown";
import diff from "highlight.js/lib/languages/diff";
import dockerfile from "highlight.js/lib/languages/dockerfile";
import lua from "highlight.js/lib/languages/lua";
import r from "highlight.js/lib/languages/r";
import perl from "highlight.js/lib/languages/perl";
import haskell from "highlight.js/lib/languages/haskell";
import elixir from "highlight.js/lib/languages/elixir";
import makefile from "highlight.js/lib/languages/makefile";
import graphql from "highlight.js/lib/languages/graphql";

hljs.registerLanguage("javascript", javascript);
hljs.registerLanguage("js", javascript);
hljs.registerLanguage("typescript", typescript);
hljs.registerLanguage("ts", typescript);
hljs.registerLanguage("tsx", typescript);
hljs.registerLanguage("jsx", javascript);
hljs.registerLanguage("python", python);
hljs.registerLanguage("rust", rust);
hljs.registerLanguage("go", go);
hljs.registerLanguage("java", java);
hljs.registerLanguage("cpp", cpp);
hljs.registerLanguage("c", cpp);
hljs.registerLanguage("csharp", csharp);
hljs.registerLanguage("ruby", ruby);
hljs.registerLanguage("php", php);
hljs.registerLanguage("swift", swift);
hljs.registerLanguage("kotlin", kotlin);
hljs.registerLanguage("bash", bash);
hljs.registerLanguage("shell", shell);
hljs.registerLanguage("zsh", bash);
hljs.registerLanguage("sh", bash);
hljs.registerLanguage("sql", sql);
hljs.registerLanguage("html", xml);
hljs.registerLanguage("xml", xml);
hljs.registerLanguage("css", css);
hljs.registerLanguage("json", json);
hljs.registerLanguage("jsonc", json);
hljs.registerLanguage("yaml", yaml);
hljs.registerLanguage("toml", yaml);
hljs.registerLanguage("markdown", markdown);
hljs.registerLanguage("md", markdown);
hljs.registerLanguage("diff", diff);
hljs.registerLanguage("dockerfile", dockerfile);
hljs.registerLanguage("lua", lua);
hljs.registerLanguage("r", r);
hljs.registerLanguage("perl", perl);
hljs.registerLanguage("haskell", haskell);
hljs.registerLanguage("elixir", elixir);
hljs.registerLanguage("makefile", makefile);
hljs.registerLanguage("graphql", graphql);

const HIGHLIGHT_CACHE_LIMIT = 128;
const HIGHLIGHT_CACHE_MAX_CODE_CHARS = 100_000;
const LAZY_HIGHLIGHT_MIN_CODE_CHARS = 20_000;
const highlightCache = new Map<string, string>();

function highlightCacheKey(code: string, language: string): string {
  return `${language}\0${code}`;
}

function getCachedHighlight(key: string): string | undefined {
  const cached = highlightCache.get(key);
  if (cached === undefined) return undefined;
  // Refresh insertion order so the Map acts as a tiny LRU cache.
  highlightCache.delete(key);
  highlightCache.set(key, cached);
  return cached;
}

function setCachedHighlight(key: string, value: string): void {
  highlightCache.set(key, value);
  while (highlightCache.size > HIGHLIGHT_CACHE_LIMIT) {
    const oldest = highlightCache.keys().next().value;
    if (oldest === undefined) break;
    highlightCache.delete(oldest);
  }
}

function highlightCode(code: string, language: string): string | undefined {
  const key = highlightCacheKey(code, language);
  const cached = getCachedHighlight(key);
  if (cached !== undefined) return cached;

  let highlighted: string;
  if (hljs.getLanguage(language)) {
    highlighted = hljs.highlight(code, { language }).value;
  } else {
    try {
      highlighted = hljs.highlightAuto(code).value;
    } catch (error) {
      console.warn(`Failed to auto-highlight code block (${language}):`, error);
      return undefined;
    }
  }

  if (code.length <= HIGHLIGHT_CACHE_MAX_CODE_CHARS) {
    setCachedHighlight(key, highlighted);
  }
  return highlighted;
}

export function CodeBlock(props: {
  code: string;
  language?: string;
  highlightTerm?: string;
}) {
  const { t } = useI18n();
  const [copied, setCopied] = createSignal(false);
  const [highlightReady, setHighlightReady] = createSignal(false);
  let copyTimer: ReturnType<typeof setTimeout> | undefined;
  let codeRef: HTMLElement | undefined;
  let highlightObserver: IntersectionObserver | undefined;

  onCleanup(() => {
    clearTimeout(copyTimer);
    highlightObserver?.disconnect();
  });

  createEffect(() => {
    highlightObserver?.disconnect();
    highlightObserver = undefined;

    const lang = props.language?.toLowerCase();
    if (
      !codeRef ||
      !lang ||
      props.code.length < LAZY_HIGHLIGHT_MIN_CODE_CHARS
    ) {
      setHighlightReady(true);
      return;
    }

    if (typeof IntersectionObserver === "undefined") {
      setHighlightReady(true);
      return;
    }

    setHighlightReady(false);
    highlightObserver = new IntersectionObserver((entries) => {
      if (!entries.some((entry) => entry.isIntersecting)) return;
      setHighlightReady(true);
      highlightObserver?.disconnect();
      highlightObserver = undefined;
    });
    highlightObserver.observe(codeRef);
  });

  createEffect(() => {
    if (!codeRef) return;

    codeRef.textContent = props.code;
    const lang = props.language?.toLowerCase();
    if (lang && highlightReady()) {
      const highlighted = highlightCode(props.code, lang);
      if (highlighted !== undefined) {
        codeRef.innerHTML = highlighted;
      }
    }

    // Wrap matches of `highlightTerm` in <mark class="search-highlight">,
    // preserving existing hljs markup. Matches that cross span boundaries
    // (e.g. a keyword that's partially colored) are not highlighted — this
    // covers the common "search for plain substring" case.
    const term = props.highlightTerm?.trim();
    if (term && term.length > 0) {
      highlightMatchesInElement(codeRef, term);
    }
  });

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(props.code);
      setCopied(true);
      clearTimeout(copyTimer);
      copyTimer = setTimeout(() => setCopied(false), 1500);
    } catch (error) {
      console.error("Failed to copy code block:", error);
      toastError(t("toast.copyFailed"));
    }
  }

  return (
    <div class="code-block">
      <div class="code-block-header">
        {props.language && (
          <span class="code-block-lang">{props.language}</span>
        )}
        <button
          type="button"
          class="code-block-copy"
          onClick={handleCopy}
          title={t("common.copyCode")}
          aria-label={t("common.copyCode")}
        >
          {copied() ? (
            <svg
              width="14"
              height="14"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
            >
              <polyline points="20 6 9 17 4 12" />
            </svg>
          ) : (
            <svg
              width="14"
              height="14"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
            >
              <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
              <path d="M5 15H4a2 2 0 01-2-2V4a2 2 0 012-2h9a2 2 0 012 2v1" />
            </svg>
          )}
        </button>
      </div>
      <pre class="code-block-pre">
        <code ref={codeRef}>{props.code}</code>
      </pre>
    </div>
  );
}

function highlightMatchesInElement(root: HTMLElement, term: string): void {
  const lower = term.toLowerCase();
  const termLen = term.length;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const nodes: Text[] = [];
  let current: Node | null = walker.nextNode();
  while (current) {
    nodes.push(current as Text);
    current = walker.nextNode();
  }
  for (const textNode of nodes) {
    const text = textNode.nodeValue ?? "";
    const lowerText = text.toLowerCase();
    let idx = lowerText.indexOf(lower);
    if (idx < 0) continue;
    const frag = document.createDocumentFragment();
    let cursor = 0;
    while (idx >= 0) {
      if (idx > cursor) {
        frag.appendChild(document.createTextNode(text.slice(cursor, idx)));
      }
      const mark = document.createElement("mark");
      mark.className = "search-highlight";
      mark.textContent = text.slice(idx, idx + termLen);
      frag.appendChild(mark);
      cursor = idx + termLen;
      idx = lowerText.indexOf(lower, cursor);
    }
    if (cursor < text.length) {
      frag.appendChild(document.createTextNode(text.slice(cursor)));
    }
    textNode.parentNode?.replaceChild(frag, textNode);
  }
}
