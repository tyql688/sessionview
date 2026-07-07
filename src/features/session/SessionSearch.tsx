import type { Dispatch, SetStateAction } from "react";
import { useI18n } from "@/i18n/index";

export interface SessionSearchProps {
  sessionSearch: string;
  activeSessionSearch: string;
  setSessionSearch: Dispatch<SetStateAction<string>>;
  searchMatchIdx: number;
  /** Data-level match count — covers the whole loaded session, independent
   * of which rows the virtualizer currently mounts. */
  matchTotal: number;
  navigateMatch: (delta: number) => void;
  setSearchBarOpen: Dispatch<SetStateAction<boolean>>;
}

export function SessionSearch(props: SessionSearchProps) {
  const { t } = useI18n();

  return (
    <div className="session-search-bar">
      <input
        className="session-search-input"
        type="text"
        placeholder={t("session.searchPlaceholder")}
        value={props.sessionSearch}
        onChange={(e) => {
          props.setSessionSearch(e.currentTarget.value);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            props.navigateMatch(e.shiftKey ? -1 : 1);
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
          if (props.matchTotal > 0)
            return `${props.searchMatchIdx + 1}/${props.matchTotal}`;
          return t("session.searchNoMatch");
        })()}
      </span>
      <button
        className="session-search-nav"
        onClick={() => props.navigateMatch(-1)}
        aria-label={t("common.previousMatch")}
      >
        &uarr;
      </button>
      <button
        className="session-search-nav"
        onClick={() => props.navigateMatch(1)}
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
