import { createSignal } from "solid-js";
import type { SessionRef } from "../lib/types";

export interface EditorGroup {
  id: string;
  tabs: SessionRef[];
  activeTabId: string | null;
  previewTabId: string | null; // at most one preview (unpinned) tab per group
  flexBasis: number; // percentage, e.g. 100 = full width
}

const MAX_GROUPS = 4;
let nextGroupId = 1;

function makeGroup(tabs: SessionRef[] = [], flexBasis = 100): EditorGroup {
  return {
    id: String(nextGroupId++),
    tabs,
    activeTabId: tabs.length > 0 ? tabs[0].id : null,
    previewTabId: null,
    flexBasis,
  };
}

const [groups, setGroups] = createSignal<EditorGroup[]>([makeGroup()]);
const [activeGroupId, setActiveGroupId] = createSignal<string>(groups()[0].id);

// ---------- helpers ----------

function findGroupBySession(sessionId: string): EditorGroup | undefined {
  return groups().find((g) => g.tabs.some((t) => t.id === sessionId));
}

function activeGroup(): EditorGroup | undefined {
  return groups().find((g) => g.id === activeGroupId());
}

function updateGroup(groupId: string, fn: (g: EditorGroup) => EditorGroup) {
  setGroups((prev) => prev.map((g) => (g.id === groupId ? fn(g) : g)));
}

/**
 * Remove a tab from its source group, returning a new group with the tab
 * gone, `activeTabId` recomputed (falls back to the last remaining tab, or
 * null when none remain), and `previewTabId` cleared when the detached tab
 * was the preview. Pure: does not mutate `group`. Callers layer any
 * flexBasis change on top of the returned group.
 */
function detachTab(group: EditorGroup, sessionId: string): EditorGroup {
  const tabs = group.tabs.filter((t) => t.id !== sessionId);
  const activeTabId =
    group.activeTabId === sessionId
      ? tabs.length > 0
        ? tabs[tabs.length - 1].id
        : null
      : group.activeTabId;
  const previewTabId =
    group.previewTabId === sessionId ? null : group.previewTabId;
  return { ...group, tabs, activeTabId, previewTabId };
}

function removeGroupIfEmpty(groupId: string) {
  setGroups((prev) => {
    if (prev.length <= 1) return prev; // keep last group
    const g = prev.find((x) => x.id === groupId);
    if (g && g.tabs.length === 0) {
      const filtered = prev.filter((x) => x.id !== groupId);
      if (activeGroupId() === groupId) {
        setActiveGroupId(filtered[filtered.length - 1].id);
      }
      // Redistribute removed group's width to survivors
      if (filtered.length === 1) {
        // sole survivor gets 100%
        filtered[0] = { ...filtered[0], flexBasis: 100 };
      } else {
        // transfer removed width proportionally
        const removedBasis = g.flexBasis;
        const share = removedBasis / filtered.length;
        return filtered.map((x) => ({
          ...x,
          flexBasis: x.flexBasis + share,
        }));
      }
      return filtered;
    }
    return prev;
  });
}

// ---------- actions ----------

function openSession(session: SessionRef) {
  const existing = findGroupBySession(session.id);
  if (existing) {
    setActiveGroupId(existing.id);
    updateGroup(existing.id, (g) => ({
      ...g,
      activeTabId: session.id,
      // opening explicitly pins a preview tab
      previewTabId: g.previewTabId === session.id ? null : g.previewTabId,
    }));
    return;
  }
  const gId = activeGroupId();
  updateGroup(gId, (g) => ({
    ...g,
    tabs: [...g.tabs, session],
    activeTabId: session.id,
  }));
}

/** Open a session as a preview (unpinned) tab. Replaces the existing preview in the active group. */
function openPreview(session: SessionRef) {
  // If already open anywhere, just focus it (don't create a duplicate)
  const existing = findGroupBySession(session.id);
  if (existing) {
    setActiveGroupId(existing.id);
    updateGroup(existing.id, (g) => ({ ...g, activeTabId: session.id }));
    return;
  }
  const gId = activeGroupId();
  updateGroup(gId, (prev) => ({
    ...prev,
    tabs: [
      ...(prev.previewTabId
        ? prev.tabs.filter((t) => t.id !== prev.previewTabId)
        : prev.tabs),
      session,
    ],
    activeTabId: session.id,
    previewTabId: session.id,
  }));
}

/** Pin a preview tab (make it permanent). */
function pinTab(sessionId: string) {
  const g = findGroupBySession(sessionId);
  if (!g || g.previewTabId !== sessionId) return;
  updateGroup(g.id, (prev) => ({ ...prev, previewTabId: null }));
}

function closeTab(sessionId: string) {
  const g = findGroupBySession(sessionId);
  if (!g) return;
  const newTabs = g.tabs.filter((t) => t.id !== sessionId);
  const newActive =
    g.activeTabId === sessionId
      ? newTabs.length > 0
        ? newTabs[newTabs.length - 1].id
        : null
      : g.activeTabId;
  const gId = g.id;
  updateGroup(gId, (prev) => ({
    ...prev,
    tabs: newTabs,
    activeTabId: newActive,
    previewTabId: prev.previewTabId === sessionId ? null : prev.previewTabId,
  }));
  removeGroupIfEmpty(gId);
}

function closeAllTabs() {
  const g = makeGroup();
  setGroups([g]);
  setActiveGroupId(g.id);
}

function closeOtherTabs(keepId: string) {
  const g = findGroupBySession(keepId);
  if (!g) return;
  const kept = g.tabs.filter((t) => t.id === keepId);
  updateGroup(g.id, (prev) => ({
    ...prev,
    tabs: kept,
    activeTabId: keepId,
    previewTabId: prev.previewTabId === keepId ? prev.previewTabId : null,
    flexBasis: 100,
  }));
  // remove all other groups
  setGroups((prev) => prev.filter((x) => x.id === g.id));
  setActiveGroupId(g.id);
}

function closeTabsToRight(fromId: string) {
  const g = findGroupBySession(fromId);
  if (!g) return;
  const idx = g.tabs.findIndex((t) => t.id === fromId);
  if (idx === -1) return;
  const kept = g.tabs.slice(0, idx + 1);
  const newActive =
    g.activeTabId && kept.some((t) => t.id === g.activeTabId)
      ? g.activeTabId
      : fromId;
  updateGroup(g.id, (prev) => ({
    ...prev,
    tabs: kept,
    activeTabId: newActive,
    previewTabId:
      prev.previewTabId && kept.some((t) => t.id === prev.previewTabId)
        ? prev.previewTabId
        : null,
  }));
}

function splitToRight(sessionId: string) {
  const sourceGroup = findGroupBySession(sessionId);
  if (!sourceGroup) return;
  // guard: sole tab in last group → no-op
  if (sourceGroup.tabs.length <= 1 && groups().length <= 1) return;

  const session = sourceGroup.tabs.find((t) => t.id === sessionId)!;

  const sourceIdx = groups().findIndex((g) => g.id === sourceGroup.id);
  const rightNeighbor = groups()[sourceIdx + 1];

  if (rightNeighbor) {
    // move to existing right group
    updateGroup(sourceGroup.id, (g) => detachTab(g, sessionId));
    updateGroup(rightNeighbor.id, (g) => ({
      ...g,
      tabs: [...g.tabs, session],
      activeTabId: session.id,
    }));
    setActiveGroupId(rightNeighbor.id);
  } else if (groups().length < MAX_GROUPS) {
    // create new group, split source width 50/50
    const halfBasis = sourceGroup.flexBasis / 2;
    updateGroup(sourceGroup.id, (g) => ({
      ...detachTab(g, sessionId),
      flexBasis: halfBasis,
    }));
    const newGroup = makeGroup([session], halfBasis);
    setGroups((prev) => [
      ...prev.slice(0, sourceIdx + 1),
      newGroup,
      ...prev.slice(sourceIdx + 1),
    ]);
    setActiveGroupId(newGroup.id);
  } else {
    // at max groups, move to rightmost
    const rightmost = groups()[groups().length - 1];
    if (rightmost.id === sourceGroup.id) return; // already rightmost, no split target
    updateGroup(sourceGroup.id, (g) => detachTab(g, sessionId));
    updateGroup(rightmost.id, (g) => ({
      ...g,
      tabs: [...g.tabs, session],
      activeTabId: session.id,
    }));
    setActiveGroupId(rightmost.id);
  }

  removeGroupIfEmpty(sourceGroup.id);
}

function moveTabToGroup(
  sessionId: string,
  targetGroupId: string,
  insertIndex?: number,
) {
  const sourceGroup = findGroupBySession(sessionId);
  if (!sourceGroup) return;
  if (sourceGroup.id === targetGroupId) {
    // reorder within group
    if (insertIndex === undefined) return;
    const tab = sourceGroup.tabs.find((t) => t.id === sessionId)!;
    const without = sourceGroup.tabs.filter((t) => t.id !== sessionId);
    const reordered = [
      ...without.slice(0, insertIndex),
      tab,
      ...without.slice(insertIndex),
    ];
    updateGroup(sourceGroup.id, (g) => ({ ...g, tabs: reordered }));
    return;
  }
  const session = sourceGroup.tabs.find((t) => t.id === sessionId)!;
  // remove from source
  updateGroup(sourceGroup.id, (g) => detachTab(g, sessionId));
  // add to target (drag = pin, so don't set previewTabId)
  updateGroup(targetGroupId, (g) => {
    const tabs =
      insertIndex !== undefined
        ? [
            ...g.tabs.slice(0, insertIndex),
            session,
            ...g.tabs.slice(insertIndex),
          ]
        : [...g.tabs, session];
    return { ...g, tabs, activeTabId: session.id };
  });
  setActiveGroupId(targetGroupId);
  removeGroupIfEmpty(sourceGroup.id);
}

function createGroupFromDrop(sessionId: string): void {
  if (groups().length >= MAX_GROUPS) return;
  const sourceGroup = findGroupBySession(sessionId);
  if (!sourceGroup) return;
  // guard: sole tab in sole group → no-op (same as splitToRight)
  if (sourceGroup.tabs.length <= 1 && groups().length <= 1) return;
  const session = sourceGroup.tabs.find((t) => t.id === sessionId)!;

  const halfBasis = sourceGroup.flexBasis / 2;
  updateGroup(sourceGroup.id, (g) => ({
    ...detachTab(g, sessionId),
    flexBasis: halfBasis,
  }));

  const newGroup = makeGroup([session], halfBasis);
  setGroups((prev) => [...prev, newGroup]);
  setActiveGroupId(newGroup.id);
  removeGroupIfEmpty(sourceGroup.id);
}

function focusGroup(groupId: string) {
  if (groups().some((g) => g.id === groupId)) {
    setActiveGroupId(groupId);
  }
}

function focusAdjacentGroup(direction: "left" | "right") {
  const idx = groups().findIndex((g) => g.id === activeGroupId());
  if (idx === -1) return;
  const nextIdx = direction === "right" ? idx + 1 : idx - 1;
  const target = groups()[nextIdx];
  if (target) setActiveGroupId(target.id);
}

function setActiveTabInGroup(groupId: string, tabId: string) {
  updateGroup(groupId, (g) => ({ ...g, activeTabId: tabId }));
}

function setGroupFlexBasis(groupId: string, basis: number) {
  updateGroup(groupId, (g) => ({ ...g, flexBasis: basis }));
}

function syncAllTabTitles(titleMap: Map<string, string>) {
  setGroups((prev) => {
    let anyGroupChanged = false;
    const next = prev.map((g) => {
      let anyTabChanged = false;
      const newTabs = g.tabs.map((tab) => {
        const newTitle = titleMap.get(tab.id);
        if (newTitle && newTitle !== tab.title) {
          anyTabChanged = true;
          return { ...tab, title: newTitle };
        }
        return tab;
      });
      if (anyTabChanged) {
        anyGroupChanged = true;
        return { ...g, tabs: newTabs };
      }
      return g;
    });
    return anyGroupChanged ? next : prev;
  });
}

/** Reset store state — useful for testing */
function _reset() {
  nextGroupId = 1;
  const g = makeGroup();
  setGroups([g]);
  setActiveGroupId(g.id);
}

export {
  MAX_GROUPS,
  groups,
  activeGroupId,
  activeGroup,
  findGroupBySession,
  openSession,
  openPreview,
  pinTab,
  closeTab,
  closeAllTabs,
  closeOtherTabs,
  closeTabsToRight,
  splitToRight,
  moveTabToGroup,
  createGroupFromDrop,
  focusGroup,
  focusAdjacentGroup,
  setActiveGroupId,
  setActiveTabInGroup,
  setGroupFlexBasis,
  syncAllTabTitles,
  _reset,
};
