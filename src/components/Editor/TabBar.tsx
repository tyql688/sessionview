import {
  type MouseEvent as ReactMouseEvent,
  useEffect,
  useRef,
  useState,
} from "react";
import type { SessionRef, Provider } from "../../lib/types";
import { useI18n } from "../../i18n/index";
import { ContextMenu, type MenuItemDef } from "../ContextMenu";
import { isMac } from "../../lib/platform";
import { moveTabToGroup } from "../../stores/editorGroups";
import {
  parseTabDragPayload,
  serializeTabDragPayload,
  TAB_DRAG_FALLBACK_MIME,
  TAB_DRAG_MIME,
} from "./tabDragPayload";

function providerColor(provider: Provider): string {
  return `var(--${provider})`;
}

export function TabBar(props: {
  groupId: string;
  tabs: SessionRef[];
  activeTabId: string | null;
  previewTabId: string | null;
  onTabSelect: (id: string) => void;
  onTabClose: (id: string) => void;
  onCloseAllTabs: () => void;
  onCloseOtherTabs: (keepId: string) => void;
  onCloseTabsToRight: (fromId: string) => void;
  onSplitToRight: (sessionId: string) => void;
  onPinTab: (sessionId: string) => void;
}) {
  const { t } = useI18n();
  const [menuState, setMenuState] = useState<{
    pos: { x: number; y: number };
    tabId: string;
  } | null>(null);
  const [overflowing, setOverflowing] = useState(false);
  const [showOverflowMenu, setShowOverflowMenu] = useState(false);

  const scrollRef = useRef<HTMLDivElement>(null);
  const overflowBtnRef = useRef<HTMLButtonElement>(null);
  const overflowMenuRef = useRef<HTMLDivElement>(null);

  // --- Overflow detection ---
  function checkOverflow() {
    const el = scrollRef.current;
    if (!el) return;
    setOverflowing(el.scrollWidth > el.clientWidth + 1);
  }

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver(checkOverflow);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Re-check overflow when tabs change (count, titles, or preview state)
  useEffect(() => {
    checkOverflow();
  }, [props.tabs]);

  // Scroll active tab into view
  useEffect(() => {
    const id = props.activeTabId;
    if (!id || !scrollRef.current) return;
    const el = scrollRef.current.querySelector(
      `[data-tab-id="${CSS.escape(id)}"]`,
    ) as HTMLElement | null;
    el?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [props.activeTabId]);

  // Natural horizontal wheel scroll. React attaches `wheel` as a passive
  // listener at the root, so `preventDefault()` there is a no-op — attach a
  // non-passive native listener to keep the Solid behavior.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (Math.abs(e.deltaX) > Math.abs(e.deltaY)) return; // natural horizontal scroll
      e.preventDefault();
      el.scrollLeft += e.deltaY;
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  function handleContextMenu(e: ReactMouseEvent, tabId: string) {
    e.preventDefault();
    e.stopPropagation();
    setMenuState({ pos: { x: e.clientX, y: e.clientY }, tabId });
  }

  function menuItems(): MenuItemDef[] {
    const m = menuState;
    if (!m) return [];
    const isPreview = m.tabId === props.previewTabId;
    const items: MenuItemDef[] = [
      {
        label: t("contextMenu.close"),
        shortcut: isMac ? "\u2318W" : "Ctrl+W",
        onClick: () => props.onTabClose(m.tabId),
      },
      {
        label: t("contextMenu.closeOthers"),
        onClick: () => props.onCloseOtherTabs(m.tabId),
      },
      {
        label: t("contextMenu.closeToRight"),
        onClick: () => props.onCloseTabsToRight(m.tabId),
      },
      {
        label: t("contextMenu.openToSide"),
        onClick: () => props.onSplitToRight(m.tabId),
      },
    ];
    if (isPreview) {
      items.push({
        label: t("contextMenu.keepOpen"),
        onClick: () => props.onPinTab(m.tabId),
      });
    }
    items.push(
      { label: "", separator: true, onClick: () => {} },
      {
        label: t("contextMenu.closeAll"),
        shortcut: isMac ? "\u21E7\u2318W" : "Ctrl+Shift+W",
        onClick: () => props.onCloseAllTabs(),
      },
    );
    return items;
  }

  // Close overflow menu when clicking outside
  function handleDocClick(e: MouseEvent) {
    const target = e.target as Node;
    if (overflowBtnRef.current?.contains(target)) return;
    if (overflowMenuRef.current?.contains(target)) return;
    setShowOverflowMenu(false);
  }
  useEffect(() => {
    document.addEventListener("mousedown", handleDocClick);
    return () => document.removeEventListener("mousedown", handleDocClick);
  }, []);

  return (
    <div className="tab-bar">
      <div
        ref={scrollRef}
        className="tab-bar-scroll"
        onDragOver={(e) => {
          e.preventDefault();
          if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
        }}
        onDrop={(e) => {
          e.preventDefault();
          const rawPayload =
            e.dataTransfer?.getData(TAB_DRAG_MIME) ||
            e.dataTransfer?.getData(TAB_DRAG_FALLBACK_MIME) ||
            "";
          if (rawPayload.length === 0) return;

          try {
            const payload = parseTabDragPayload(rawPayload);
            if (payload.sourceGroupId !== props.groupId) {
              moveTabToGroup(payload.sessionId, props.groupId);
            }
          } catch (error) {
            console.warn("Failed to parse dragged tab payload:", error);
          }
        }}
      >
        {props.tabs.map((tab) => {
          const isActive = tab.id === props.activeTabId;
          const isPreview = tab.id === props.previewTabId;
          return (
            <div
              key={tab.id}
              className={`tab${isActive ? " active" : ""}${isPreview ? " preview" : ""}`}
              data-tab-id={tab.id}
              draggable={true}
              onDragStart={(e) => {
                const transfer = e.dataTransfer;
                if (!transfer) {
                  console.warn("Tab drag started without dataTransfer");
                  return;
                }
                const payload = serializeTabDragPayload({
                  sessionId: tab.id,
                  sourceGroupId: props.groupId,
                });
                transfer.setData(TAB_DRAG_MIME, payload);
                transfer.setData(TAB_DRAG_FALLBACK_MIME, payload);
                transfer.effectAllowed = "move";
                (e.currentTarget as HTMLElement).style.opacity = "0.4";
              }}
              onDragEnd={(e) => {
                (e.currentTarget as HTMLElement).style.opacity = "";
              }}
              onClick={(e) => {
                if (e.button === 0) props.onTabSelect(tab.id);
              }}
              onDoubleClick={() => {
                if (isPreview) props.onPinTab(tab.id);
              }}
              onMouseDown={(e) => {
                if (e.button === 1) {
                  e.preventDefault();
                  props.onTabClose(tab.id);
                }
              }}
              onContextMenu={(e) => handleContextMenu(e, tab.id)}
            >
              <span
                className="tab-dot"
                style={{ background: providerColor(tab.provider) }}
              />
              <span className="tab-title">{tab.title}</span>
              <button
                className={`tab-close${isActive ? " visible" : ""}`}
                aria-label={t("common.closeTab")}
                onClick={(e) => {
                  e.stopPropagation();
                  props.onTabClose(tab.id);
                }}
              >
                &times;
              </button>
            </div>
          );
        })}
      </div>

      {/* Overflow chevron */}
      {overflowing && (
        <>
          <button
            ref={overflowBtnRef}
            className="tab-overflow-btn"
            title={t("tabs.showOpenTabs")}
            onClick={() => setShowOverflowMenu((v) => !v)}
          >
            &#xBB;
          </button>
          {showOverflowMenu && (
            <div ref={overflowMenuRef} className="tab-overflow-menu">
              {props.tabs.map((tab) => (
                <button
                  key={tab.id}
                  className={`tab-overflow-item${tab.id === props.activeTabId ? " active" : ""}${tab.id === props.previewTabId ? " preview" : ""}`}
                  onClick={() => {
                    props.onTabSelect(tab.id);
                    setShowOverflowMenu(false);
                  }}
                >
                  <span
                    className="tab-dot"
                    style={{ background: providerColor(tab.provider) }}
                  />
                  <span className="tab-overflow-title">{tab.title}</span>
                </button>
              ))}
            </div>
          )}
        </>
      )}

      <ContextMenu
        items={menuItems()}
        position={menuState?.pos ?? null}
        onClose={() => setMenuState(null)}
      />
    </div>
  );
}
