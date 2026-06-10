import { createEffect, Show, For, onCleanup } from "solid-js";
import type { SessionRef } from "../lib/types";
import {
  query,
  results,
  isSearching,
  search,
  clearSearch,
  parseSearchQuery,
  setPendingSessionSearch,
} from "../stores/search";
import { useI18n } from "../i18n/index";
import { ProviderIcon } from "./icons";
import { createSignal } from "solid-js";

function sanitizeSnippet(html: string): string {
  const escaped = html
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
  return escaped
    .replace(/&lt;mark&gt;/gi, "<mark>")
    .replace(/&lt;\/mark&gt;/gi, "</mark>");
}

export function SearchOverlay(props: {
  show: boolean;
  onClose: () => void;
  onOpenSession: (session: SessionRef) => void;
}) {
  const { t } = useI18n();
  const [selectedIndex, setSelectedIndex] = createSignal(0);
  let inputRef: HTMLInputElement | undefined;

  createEffect(() => {
    if (props.show) {
      queueMicrotask(() => inputRef?.focus());
      setSelectedIndex(0);
    }
  });

  createEffect(() => {
    // keep selection within range as results change
    const len = results().length;
    if (selectedIndex() >= len) {
      setSelectedIndex(len > 0 ? len - 1 : 0);
    }
  });

  function handleInput(e: InputEvent) {
    const target = e.currentTarget as HTMLInputElement;
    search(target.value);
    setSelectedIndex(0);
  }

  function openAt(idx: number) {
    const r = results();
    if (idx < 0 || idx >= r.length) return;
    const session = r[idx].session;
    // Strip filter prefixes (provider:foo project:bar ...) so the in-session
    // search receives the actual content query the user typed.
    const contentQuery = parseSearchQuery(query()).query;
    if (contentQuery) {
      setPendingSessionSearch({ sessionId: session.id, query: contentQuery });
    }
    props.onOpenSession(session);
    handleClose();
  }

  function handleClose() {
    clearSearch();
    setSelectedIndex(0);
    props.onClose();
  }

  function handleKeyDown(e: KeyboardEvent) {
    const r = results();
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSelectedIndex((i) => (r.length === 0 ? 0 : (i + 1) % r.length));
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelectedIndex((i) =>
        r.length === 0 ? 0 : (i - 1 + r.length) % r.length,
      );
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      // Block Enter while a search is still in flight, so a lingering result
      // list from the previous keystroke can't be navigated to.
      if (isSearching() || r.length === 0) return;
      openAt(selectedIndex());
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      handleClose();
    }
  }

  onCleanup(() => {
    clearSearch();
  });

  return (
    <Show when={props.show}>
      <div
        class="search-overlay-backdrop"
        onMouseDown={(e) => {
          if (e.target === e.currentTarget) handleClose();
        }}
      >
        <div
          class="search-overlay"
          role="dialog"
          aria-modal="true"
          aria-label={t("search.ariaLabel")}
        >
          <div class="search-overlay-input-row">
            <svg
              class="search-icon"
              width="16"
              height="16"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
            >
              <circle cx="11" cy="11" r="8" />
              <path d="M21 21l-4.35-4.35" />
            </svg>
            <input
              ref={inputRef}
              class="search-overlay-input"
              type="text"
              aria-label={t("search.ariaLabel")}
              placeholder={t("search.placeholder")}
              value={query()}
              onInput={handleInput}
              onKeyDown={handleKeyDown}
            />
            <kbd class="search-shortcut">Esc</kbd>
          </div>
          <div class="search-overlay-results">
            <Show when={isSearching()}>
              <div class="search-loading">
                <div class="spinner spinner-sm" />
              </div>
            </Show>
            <Show
              when={
                !isSearching() &&
                results().length === 0 &&
                query().trim().length > 0
              }
            >
              <div class="search-no-results">{t("search.noResults")}</div>
            </Show>
            <Show when={query().trim().length === 0}>
              <div class="search-no-results">{t("search.placeholder")}</div>
            </Show>
            <For each={results()}>
              {(result, i) => (
                <button
                  class="search-result-item"
                  classList={{ selected: selectedIndex() === i() }}
                  onMouseDown={(e) => {
                    e.preventDefault();
                    openAt(i());
                  }}
                  onMouseEnter={() => setSelectedIndex(i())}
                >
                  <span
                    class="provider-dot provider-logo"
                    style={{ color: `var(--${result.session.provider})` }}
                  >
                    <ProviderIcon provider={result.session.provider} />
                  </span>
                  <div class="search-result-text">
                    <span class="search-result-title">
                      {result.session.title}
                    </span>
                    <span
                      class="search-result-snippet"
                      // eslint-disable-next-line solid/no-innerhtml -- sanitizeSnippet escapes then restores <mark> only
                      innerHTML={sanitizeSnippet(result.snippet)}
                    />
                  </div>
                </button>
              )}
            </For>
          </div>
        </div>
      </div>
    </Show>
  );
}
