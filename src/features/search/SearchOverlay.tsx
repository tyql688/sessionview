import type React from "react";
import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import type { SessionRef } from "@/lib/types";
import {
  useSearchQuery,
  useSearchResults,
  useIsSearching,
  search,
  clearSearch,
  parseSearchQuery,
  setPendingSessionSearch,
} from "@/features/search/search";
import { useI18n } from "@/i18n/index";
import { ProviderIcon } from "@/components/icons";
import { cn } from "@/lib/utils";

function sanitizeSnippet(html: string): string {
  const escaped = html.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  return escaped.replace(/&lt;mark&gt;/gi, "<mark>").replace(/&lt;\/mark&gt;/gi, "</mark>");
}

export function SearchOverlay(props: {
  show: boolean;
  onClose: () => void;
  onOpenSession: (session: SessionRef) => void;
}) {
  const { t } = useI18n();
  const query = useSearchQuery();
  const results = useSearchResults();
  const isSearching = useIsSearching();
  const [selectedIndex, setSelectedIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (props.show) {
      setSelectedIndex(0);
    } else {
      clearSearch();
    }
  }, [props.show]);

  useEffect(() => {
    // keep selection within range as results change
    const len = results.length;
    if (selectedIndex >= len) {
      setSelectedIndex(len > 0 ? len - 1 : 0);
    }
  }, [results, selectedIndex]);

  function handleInput(e: React.ChangeEvent<HTMLInputElement>) {
    const target = e.currentTarget;
    search(target.value);
    setSelectedIndex(0);
  }

  function openAt(idx: number) {
    const r = results;
    if (idx < 0 || idx >= r.length) return;
    const session = r[idx].session;
    // Strip filter prefixes (provider:foo project:bar ...) so the in-session
    // search receives the actual content query the user typed.
    const contentQuery = parseSearchQuery(query).query;
    if (contentQuery) {
      setPendingSessionSearch({ sessionId: session.id, query: contentQuery });
    }
    props.onOpenSession(session);
    handleClose();
  }

  function handleClose() {
    if (!props.show) return;
    clearSearch();
    setSelectedIndex(0);
    props.onClose();
  }

  function handleKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    const r = results;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSelectedIndex((i) => (r.length === 0 ? 0 : (i + 1) % r.length));
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelectedIndex((i) => (r.length === 0 ? 0 : (i - 1 + r.length) % r.length));
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      // Block Enter while a search is still in flight, so a lingering result
      // list from the previous keystroke can't be navigated to.
      if (isSearching || r.length === 0) return;
      openAt(selectedIndex);
      return;
    }
  }

  useEffect(() => {
    return () => {
      clearSearch();
    };
  }, []);

  return (
    <Dialog
      open={props.show}
      onOpenChange={(open) => {
        if (!open) handleClose();
      }}
    >
      <DialogContent
        showCloseButton={false}
        initialFocus={() => inputRef.current}
        className="search-overlay top-[12vh] left-1/2 flex max-h-[70vh] w-[min(640px,90vw)] max-w-none -translate-x-1/2 translate-y-0 flex-col gap-0 rounded-[10px] border border-border bg-[var(--bg-editor)] p-0 shadow-[0_12px_36px_rgba(0,0,0,0.25)] ring-0 sm:max-w-none data-open:zoom-in-100 data-closed:zoom-out-100"
      >
        <DialogTitle className="sr-only">{t("search.ariaLabel")}</DialogTitle>
        <div className="search-overlay-input-row">
          <svg
            className="search-icon"
            width="16"
            height="16"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
          >
            <circle cx="11" cy="11" r="8" />
            <path d="M21 21l-4.35-4.35" />
          </svg>
          <Input
            ref={inputRef}
            className="search-overlay-input h-auto border-none px-0 py-0 focus-visible:border-transparent focus-visible:ring-0"
            type="text"
            aria-label={t("search.ariaLabel")}
            placeholder={t("search.placeholder")}
            value={query}
            onChange={handleInput}
            onKeyDown={handleKeyDown}
          />
          <kbd className="search-shortcut">Esc</kbd>
        </div>
        <div className="search-overlay-results">
          {isSearching && (
            <div className="search-loading">
              <div className="spinner spinner-sm" />
            </div>
          )}
          {!isSearching && results.length === 0 && query.trim().length > 0 && (
            <div className="search-no-results">{t("search.noResults")}</div>
          )}
          {query.trim().length === 0 && <div className="search-no-results">{t("search.placeholder")}</div>}
          {results.map((result, i) => (
            <Button
              key={result.session.id}
              variant="ghost"
              className={cn(
                "search-result-item h-auto justify-start rounded-none whitespace-normal active:translate-y-0",
                selectedIndex === i && "selected",
              )}
              onMouseDown={(e) => {
                e.preventDefault();
                openAt(i);
              }}
              onMouseEnter={() => setSelectedIndex(i)}
            >
              <span className="provider-dot provider-logo" style={{ color: `var(--${result.session.provider})` }}>
                <ProviderIcon provider={result.session.provider} />
              </span>
              <div className="search-result-text">
                <span className="search-result-title">{result.session.title}</span>
                <span
                  className="search-result-snippet"
                  // biome-ignore lint/security/noDangerouslySetInnerHtml: sanitizeSnippet escapes then restores <mark> only
                  dangerouslySetInnerHTML={{
                    __html: sanitizeSnippet(result.snippet),
                  }}
                />
              </div>
            </Button>
          ))}
        </div>
      </DialogContent>
    </Dialog>
  );
}
