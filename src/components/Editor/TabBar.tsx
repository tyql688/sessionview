import {
  createSignal,
  createEffect,
  on,
  onMount,
  onCleanup,
  For,
  Show,
} from "solid-js";
import type { SessionRef, Provider } from "../../lib/types";
import { useI18n } from "../../i18n/index";
import { ContextMenu, type MenuItemDef } from "../ContextMenu";
import { isMac } from "../../lib/platform";
import { moveTabToGroup } from "../../stores/editorGroups";

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
  const [menuState, setMenuState] = createSignal<{
    pos: { x: number; y: number };
    tabId: string;
  } | null>(null);
  const [overflowing, setOverflowing] = createSignal(false);
  const [showOverflowMenu, setShowOverflowMenu] = createSignal(false);

  let scrollRef: HTMLDivElement | undefined;
  let overflowBtnRef: HTMLButtonElement | undefined;
  let overflowMenuRef: HTMLDivElement | undefined;

  // --- Overflow detection ---
  function checkOverflow() {
    if (!scrollRef) return;
    setOverflowing(scrollRef.scrollWidth > scrollRef.clientWidth + 1);
  }

  onMount(() => {
    if (!scrollRef) return;
    const ro = new ResizeObserver(checkOverflow);
    ro.observe(scrollRef);
    onCleanup(() => ro.disconnect());
  });

  // Re-check overflow when tabs change (count, titles, or preview state)
  createEffect(on(() => props.tabs, checkOverflow, { defer: true }));

  // Scroll active tab into view
  createEffect(
    on(
      () => props.activeTabId,
      (id) => {
        if (!id || !scrollRef) return;
        const el = scrollRef.querySelector(
          `[data-tab-id="${CSS.escape(id)}"]`,
        ) as HTMLElement | null;
        el?.scrollIntoView({ block: "nearest", inline: "nearest" });
      },
      { defer: true },
    ),
  );

  function handleWheel(e: WheelEvent) {
    if (!scrollRef) return;
    if (Math.abs(e.deltaX) > Math.abs(e.deltaY)) return; // natural horizontal scroll
    e.preventDefault();
    scrollRef.scrollLeft += e.deltaY;
  }

  function handleContextMenu(e: MouseEvent, tabId: string) {
    e.preventDefault();
    e.stopPropagation();
    setMenuState({ pos: { x: e.clientX, y: e.clientY }, tabId });
  }

  function menuItems(): MenuItemDef[] {
    const m = menuState();
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
    if (overflowBtnRef?.contains(target)) return;
    if (overflowMenuRef?.contains(target)) return;
    setShowOverflowMenu(false);
  }
  onMount(() => {
    document.addEventListener("mousedown", handleDocClick);
    onCleanup(() => document.removeEventListener("mousedown", handleDocClick));
  });

  return (
    <div class="tab-bar">
      <div
        ref={scrollRef}
        class="tab-bar-scroll"
        onWheel={handleWheel}
        onDragOver={(e) => {
          e.preventDefault();
          if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
        }}
        onDrop={(e) => {
          e.preventDefault();
          try {
            const data: unknown = JSON.parse(
              e.dataTransfer?.getData("text/plain") ?? "{}",
            );
            const payload = data as {
              sessionId?: unknown;
              sourceGroupId?: unknown;
            };
            if (
              typeof payload.sessionId === "string" &&
              payload.sourceGroupId !== props.groupId
            ) {
              moveTabToGroup(payload.sessionId, props.groupId);
            }
          } catch (error) {
            console.warn("Failed to parse dragged tab payload:", error);
          }
        }}
      >
        <For each={props.tabs}>
          {(tab) => {
            const isActive = () => tab.id === props.activeTabId;
            const isPreview = () => tab.id === props.previewTabId;
            return (
              <div
                class={`tab${isActive() ? " active" : ""}${isPreview() ? " preview" : ""}`}
                data-tab-id={tab.id}
                draggable={true}
                onDragStart={(e) => {
                  e.dataTransfer!.setData(
                    "text/plain",
                    JSON.stringify({
                      sessionId: tab.id,
                      sourceGroupId: props.groupId,
                    }),
                  );
                  e.dataTransfer!.effectAllowed = "move";
                  (e.currentTarget as HTMLElement).style.opacity = "0.4";
                }}
                onDragEnd={(e) => {
                  (e.currentTarget as HTMLElement).style.opacity = "";
                }}
                onClick={(e) => {
                  if (e.button === 0) props.onTabSelect(tab.id);
                }}
                onDblClick={() => {
                  if (isPreview()) props.onPinTab(tab.id);
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
                  class="tab-dot"
                  style={{ background: providerColor(tab.provider) }}
                />
                <span class="tab-title">{tab.title}</span>
                <button
                  class={`tab-close${isActive() ? " visible" : ""}`}
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
          }}
        </For>
      </div>

      {/* Overflow chevron */}
      <Show when={overflowing()}>
        <button
          ref={overflowBtnRef}
          class="tab-overflow-btn"
          title={t("tabs.showOpenTabs")}
          onClick={() => setShowOverflowMenu((v) => !v)}
        >
          &#xBB;
        </button>
        <Show when={showOverflowMenu()}>
          <div ref={overflowMenuRef} class="tab-overflow-menu">
            <For each={props.tabs}>
              {(tab) => (
                <button
                  class={`tab-overflow-item${tab.id === props.activeTabId ? " active" : ""}${tab.id === props.previewTabId ? " preview" : ""}`}
                  onClick={() => {
                    props.onTabSelect(tab.id);
                    setShowOverflowMenu(false);
                  }}
                >
                  <span
                    class="tab-dot"
                    style={{ background: providerColor(tab.provider) }}
                  />
                  <span class="tab-overflow-title">{tab.title}</span>
                </button>
              )}
            </For>
          </div>
        </Show>
      </Show>

      <ContextMenu
        items={menuItems()}
        position={menuState()?.pos ?? null}
        onClose={() => setMenuState(null)}
      />
    </div>
  );
}
