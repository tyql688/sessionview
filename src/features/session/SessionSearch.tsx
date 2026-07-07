import type { Dispatch, SetStateAction } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
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
      <Input
        className="session-search-input h-auto"
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
          if (props.matchTotal > 0) return `${props.searchMatchIdx + 1}/${props.matchTotal}`;
          return t("session.searchNoMatch");
        })()}
      </span>
      <Button
        variant="outline"
        size="icon-xs"
        className="session-search-nav active:translate-y-0"
        onClick={() => props.navigateMatch(-1)}
        aria-label={t("common.previousMatch")}
      >
        &uarr;
      </Button>
      <Button
        variant="outline"
        size="icon-xs"
        className="session-search-nav active:translate-y-0"
        onClick={() => props.navigateMatch(1)}
        aria-label={t("common.nextMatch")}
      >
        &darr;
      </Button>
      <Button
        variant="outline"
        size="icon-xs"
        className="session-search-nav active:translate-y-0"
        onClick={() => {
          props.setSearchBarOpen(false);
          props.setSessionSearch("");
        }}
        aria-label={t("common.closeSearch")}
      >
        &times;
      </Button>
    </div>
  );
}
