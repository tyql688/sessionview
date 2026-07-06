import type { Dispatch, SetStateAction } from "react";
import { useEffect, useRef, useState } from "react";
import { useI18n } from "@/i18n/index";
import {
  applySearchHighlight,
  collectSearchRanges,
  scrollRangeIntoView,
} from "@/components/SessionView/search-utils";

export interface SessionSearchProps {
  sessionSearch: string;
  activeSessionSearch: string;
  setSessionSearch: Dispatch<SetStateAction<string>>;
  searchMatchIdx: number;
  setSearchMatchIdx: Dispatch<SetStateAction<number>>;
  setSearchBarOpen: Dispatch<SetStateAction<boolean>>;
  // Accessor (not a bare ref) so it reflects the live messages container even
  // when the search bar is opened before the messages div mounts (Cmd+F during
  // load). Passing the ref by value would capture `undefined` permanently.
  messagesRef: () => HTMLDivElement | undefined;
}

export function SessionSearch(props: SessionSearchProps) {
  const { t } = useI18n();

  // Single source of truth for the displayed total: the number of highlight
  // ranges — the SAME list navigation cycles over, so the counter can never
  // disagree with how many times Next advances before looping.
  const [rangeCount, setRangeCount] = useState(0);

  function currentRanges(): Range[] {
    return collectSearchRanges(props.messagesRef(), props.activeSessionSearch);
  }

  // Recompute the count whenever the committed query changes. The rows render
  // during the re-render the new `activeSessionSearch` triggers, so wait two
  // animation frames (mirroring the focus-first-match timing in
  // createSessionSearch) before reading the DOM.
  const pendingRafRef = useRef<number | undefined>(undefined);
  const clearPendingRaf = () => {
    if (pendingRafRef.current !== undefined)
      cancelAnimationFrame(pendingRafRef.current);
    pendingRafRef.current = undefined;
  };
  useEffect(() => clearPendingRaf, []);

  useEffect(() => {
    const active = props.activeSessionSearch.trim();
    clearPendingRaf();
    if (!active) {
      setRangeCount(0);
      applySearchHighlight([], null);
      return;
    }
    pendingRafRef.current = requestAnimationFrame(() => {
      pendingRafRef.current = requestAnimationFrame(() => {
        pendingRafRef.current = undefined;
        setRangeCount(currentRanges().length);
      });
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.activeSessionSearch]);

  function navigateSearchMatch(delta: number) {
    const ranges = currentRanges();
    // Keep the displayed total in sync with the list we are about to cycle.
    setRangeCount(ranges.length);
    if (ranges.length === 0) return;
    const newIdx =
      (props.searchMatchIdx + delta + ranges.length) % ranges.length;
    props.setSearchMatchIdx(newIdx);
    applySearchHighlight(ranges, newIdx);
    scrollRangeIntoView(ranges[newIdx]);
  }

  return (
    <div className="session-search-bar">
      <input
        className="session-search-input"
        type="text"
        placeholder={t("session.searchPlaceholder")}
        value={props.sessionSearch}
        onChange={(e) => {
          props.setSessionSearch(e.currentTarget.value);
          props.setSearchMatchIdx(0);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            if (e.shiftKey) {
              navigateSearchMatch(-1);
            } else {
              navigateSearchMatch(1);
            }
          }
          if (e.key === "Escape") {
            props.setSearchBarOpen(false);
            props.setSessionSearch("");
          }
        }}
      />
      <span className="session-search-count">
        {(() => {
          const query = props.sessionSearch.trim();
          const activeQuery = props.activeSessionSearch.trim();
          if (!query) return "";
          if (query !== activeQuery) return "";
          const total = rangeCount;
          if (total > 0) return `${props.searchMatchIdx + 1}/${total}`;
          return t("session.searchNoMatch");
        })()}
      </span>
      <button
        className="session-search-nav"
        onClick={() => navigateSearchMatch(-1)}
        aria-label={t("common.previousMatch")}
      >
        &uarr;
      </button>
      <button
        className="session-search-nav"
        onClick={() => navigateSearchMatch(1)}
        aria-label={t("common.nextMatch")}
      >
        &darr;
      </button>
      <button
        className="session-search-nav"
        onClick={() => {
          props.setSearchBarOpen(false);
          props.setSessionSearch("");
        }}
        aria-label={t("common.closeSearch")}
      >
        &times;
      </button>
    </div>
  );
}
