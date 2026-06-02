import type { Accessor, Setter } from "solid-js";
import { useI18n } from "../../i18n/index";

export function SessionSearch(props: {
  sessionSearch: Accessor<string>;
  activeSessionSearch: Accessor<string>;
  setSessionSearch: Setter<string>;
  searchMatchIdx: Accessor<number>;
  searchMatchCount: Accessor<number>;
  setSearchMatchIdx: Setter<number>;
  setSearchBarOpen: Setter<boolean>;
  // Accessor (not a bare ref) so it reflects the live messages container even
  // when the search bar is opened before the messages div mounts (Cmd+F during
  // load). Passing the ref by value would capture `undefined` permanently.
  messagesRef: Accessor<HTMLDivElement | undefined>;
}) {
  const { t } = useI18n();

  /** Get marks in visual order (top->bottom). Sort by position since column-reverse
   *  flips message order but not text order within each message. */
  function getMarksInVisualOrder(): Element[] {
    const ref = props.messagesRef();
    if (!ref) return [];
    const marks = Array.from(ref.querySelectorAll("mark.search-highlight"));
    marks.sort((a, b) => {
      const ra = a.getBoundingClientRect();
      const rb = b.getBoundingClientRect();

      return ra.top - rb.top || ra.left - rb.left;
    });

    return marks;
  }

  function navigateSearchMatch(delta: number) {
    const marks = getMarksInVisualOrder();
    if (marks.length === 0) return;
    // Remove previous active highlight
    props
      .messagesRef()
      ?.querySelector("mark.search-active")
      ?.classList.remove("search-active");
    const newIdx =
      (props.searchMatchIdx() + delta + marks.length) % marks.length;
    props.setSearchMatchIdx(newIdx);
    const target = marks[newIdx];
    target.classList.add("search-active");
    target.scrollIntoView({ behavior: "smooth", block: "center" });
  }

  return (
    <div class="session-search-bar">
      <input
        class="session-search-input"
        type="text"
        placeholder={t("session.searchPlaceholder")}
        value={props.sessionSearch()}
        onInput={(e) => {
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
      <span class="session-search-count">
        {(() => {
          const query = props.sessionSearch().trim();
          const activeQuery = props.activeSessionSearch().trim();
          if (!query) return "";
          if (query !== activeQuery) return "";
          const total = props.searchMatchCount();
          if (total > 0) return `${props.searchMatchIdx() + 1}/${total}`;
          return t("session.searchNoMatch");
        })()}
      </span>
      <button
        class="session-search-nav"
        onClick={() => navigateSearchMatch(-1)}
        aria-label={t("common.previousMatch")}
      >
        &uarr;
      </button>
      <button
        class="session-search-nav"
        onClick={() => navigateSearchMatch(1)}
        aria-label={t("common.nextMatch")}
      >
        &darr;
      </button>
      <button
        class="session-search-nav"
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
