import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ChevronsRight } from "lucide-react";
import {
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  useEffect,
  useRef,
  useState,
} from "react";
import type { SessionRef, Provider } from "@/lib/types";
import { useI18n } from "@/i18n/index";
import { ContextMenu, type MenuItemDef } from "@/components/ContextMenu";
import { isMac } from "@/lib/platform";
import { useLongPress } from "@/lib/useLongPress";
import { useIsCoarse, useIsCompact } from "@/stores/viewport";
import { moveTabToGroup } from "@/features/editor/editorGroups";
import {
  parseTabDragPayload,
  serializeTabDragPayload,
  TAB_DRAG_FALLBACK_MIME,
  TAB_DRAG_MIME,
} from "@/features/editor/tabDragPayload";

function providerColor(provider: Provider): string {
  return `var(--${provider})`;
}

interface TabBarProps {
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
}

export function TabBar(props: TabBarProps) {
  const { t } = useI18n();
  const isCoarse = useIsCoarse();
  const isCompact = useIsCompact();
  const [menuState, setMenuState] = useState<{
    pos: { x: number; y: number };
    tabId: string;
  } | null>(null);
  const [overflowing, setOverflowing] = useState(false);

  const scrollRef = useRef<HTMLDivElement>(null);

  // Touch stand-in for the right-click tab menu: the pressed tab is recorded
  // at pointerdown so the shared long-press handlers know which tab to target.
  const pressedTabRef = useRef<string | null>(null);
  const longPress = useLongPress((pos) => {
    const tabId = pressedTabRef.current;
    if (tabId) setMenuState({ pos, tabId });
  });

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
    const el = scrollRef.current.querySelector(`[data-tab-id="${CSS.escape(id)}"]`) as HTMLElement | null;
    el?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [props.activeTabId]);

  // Natural horizontal wheel scroll. React attaches `wheel` as a passive
  // listener at the root, so `preventDefault()` there is a no-op — attach a
  // non-passive native listener for consistent horizontal tab scrolling.
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

  function handleTabKeyDown(event: ReactKeyboardEvent<HTMLButtonElement>, index: number) {
    let nextIndex: number;
    switch (event.key) {
      case "ArrowLeft":
        nextIndex = (index - 1 + props.tabs.length) % props.tabs.length;
        break;
      case "ArrowRight":
        nextIndex = (index + 1) % props.tabs.length;
        break;
      case "Home":
        nextIndex = 0;
        break;
      case "End":
        nextIndex = props.tabs.length - 1;
        break;
      case "Delete":
        event.preventDefault();
        props.onTabClose(props.tabs[index].id);
        requestAnimationFrame(() => {
          scrollRef.current?.querySelector<HTMLButtonElement>('[role="tab"][tabindex="0"]')?.focus();
        });
        return;
      default:
        return;
    }

    event.preventDefault();
    const nextTab = props.tabs[nextIndex];
    props.onTabSelect(nextTab.id);
    scrollRef.current?.querySelector<HTMLButtonElement>(`[data-tab-target="${CSS.escape(nextTab.id)}"]`)?.focus();
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
    ];
    // Split view doesn't exist in the compact single-group layout.
    if (!isCompact) {
      items.push({
        label: t("contextMenu.openToSide"),
        onClick: () => props.onSplitToRight(m.tabId),
      });
    }
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

  return (
    <div className="tab-bar">
      <div
        ref={scrollRef}
        className="tab-bar-scroll"
        role="tablist"
        aria-label={t("tabs.openTabs")}
        onDragOver={(e) => {
          e.preventDefault();
          if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
        }}
        onDrop={(e) => {
          e.preventDefault();
          const rawPayload =
            e.dataTransfer?.getData(TAB_DRAG_MIME) || e.dataTransfer?.getData(TAB_DRAG_FALLBACK_MIME) || "";
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
        {props.tabs.map((tab, index) => {
          const isActive = tab.id === props.activeTabId;
          const isPreview = tab.id === props.previewTabId;
          return (
            <div
              key={tab.id}
              className={`tab${isActive ? " active" : ""}${isPreview ? " preview" : ""}`}
              data-tab-id={tab.id}
              draggable={!isCoarse}
              onPointerDown={(e) => {
                pressedTabRef.current = tab.id;
                longPress.onPointerDown(e);
              }}
              onPointerMove={longPress.onPointerMove}
              onPointerUp={longPress.onPointerUp}
              onPointerCancel={longPress.onPointerCancel}
              onClickCapture={longPress.onClickCapture}
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
              onMouseDown={(e) => {
                if (e.button === 1) {
                  e.preventDefault();
                  props.onTabClose(tab.id);
                }
              }}
              onContextMenu={(e) => handleContextMenu(e, tab.id)}
            >
              <button
                type="button"
                role="tab"
                id={`tab-${props.groupId}-${tab.id}`}
                className="tab-select"
                data-tab-target={tab.id}
                aria-selected={isActive}
                aria-controls={`tabpanel-${props.groupId}-${tab.id}`}
                tabIndex={isActive ? 0 : -1}
                onClick={() => props.onTabSelect(tab.id)}
                onDoubleClick={() => {
                  if (isPreview) props.onPinTab(tab.id);
                }}
                onKeyDown={(event) => handleTabKeyDown(event, index)}
              >
                <span className="tab-dot" style={{ background: providerColor(tab.provider) }} />
                <span className="tab-title">{tab.title}</span>
              </button>
              <Button
                variant="ghost"
                size="icon-xs"
                className={`tab-close active:translate-y-0${isActive ? " visible" : ""}`}
                aria-label={`${t("common.closeTab")}: ${tab.title}`}
                tabIndex={-1}
                onClick={(e) => {
                  e.stopPropagation();
                  props.onTabClose(tab.id);
                }}
              >
                &times;
              </Button>
            </div>
          );
        })}
      </div>

      {/* Overflow chevron */}
      {overflowing && (
        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <Button
                variant="ghost"
                size="icon-xs"
                className="tab-overflow-btn shrink-0"
                title={t("tabs.showOpenTabs")}
                aria-label={t("tabs.showOpenTabs")}
              />
            }
          >
            <ChevronsRight className="size-3.5" aria-hidden="true" />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" side="bottom" className="w-64 max-w-80">
            {props.tabs.map((tab) => (
              <DropdownMenuItem
                key={tab.id}
                className={`tab-overflow-item${tab.id === props.activeTabId ? " active" : ""}${tab.id === props.previewTabId ? " preview" : ""}`}
                onClick={() => props.onTabSelect(tab.id)}
              >
                <span className="tab-dot" style={{ background: providerColor(tab.provider) }} />
                <span className="tab-overflow-title">{tab.title}</span>
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      )}

      <ContextMenu items={menuItems()} position={menuState?.pos ?? null} onClose={() => setMenuState(null)} />
    </div>
  );
}
