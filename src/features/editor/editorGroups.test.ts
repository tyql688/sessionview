import { describe, it, expect, beforeEach } from "vitest";
import type { SessionRef } from "@/lib/types";
import {
  getGroups as groups,
  getActiveGroupId as activeGroupId,
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
  focusAdjacentGroup,
  mergeAllGroups,
  _reset,
} from "@/features/editor/editorGroups";

function makeSession(id: string): SessionRef {
  return {
    id,
    provider: "claude",
    title: `Session ${id}`,
    project_name: "test",
    is_sidechain: false,
  };
}

describe("editorGroups store", () => {
  beforeEach(() => _reset());

  describe("openSession", () => {
    it("adds session to active group", () => {
      openSession(makeSession("s1"));
      expect(groups()[0].tabs).toHaveLength(1);
      expect(groups()[0].activeTabId).toBe("s1");
    });

    it("does not duplicate — activates existing", () => {
      openSession(makeSession("s1"));
      openSession(makeSession("s2"));
      openSession(makeSession("s1"));
      expect(groups()[0].tabs).toHaveLength(2);
      expect(groups()[0].activeTabId).toBe("s1");
    });

    it("focuses group containing existing session", () => {
      openSession(makeSession("s1"));
      openSession(makeSession("s2"));
      splitToRight("s1"); // s1 moved to new group 2
      const g2Id = groups()[1].id;
      openSession(makeSession("s1"));
      expect(activeGroupId()).toBe(g2Id);
    });
  });

  describe("closeTab", () => {
    it("removes tab and activates previous", () => {
      openSession(makeSession("s1"));
      openSession(makeSession("s2"));
      closeTab("s2");
      expect(groups()[0].tabs).toHaveLength(1);
      expect(groups()[0].activeTabId).toBe("s1");
    });

    it("auto-destroys empty non-last group", () => {
      openSession(makeSession("s1"));
      openSession(makeSession("s2"));
      splitToRight("s2");
      expect(groups()).toHaveLength(2);
      closeTab("s2");
      expect(groups()).toHaveLength(1);
    });

    it("keeps last group even when empty", () => {
      openSession(makeSession("s1"));
      closeTab("s1");
      expect(groups()).toHaveLength(1);
      expect(groups()[0].tabs).toHaveLength(0);
    });
  });

  describe("closeAllTabs", () => {
    it("resets to single empty group", () => {
      openSession(makeSession("s1"));
      openSession(makeSession("s2"));
      splitToRight("s2");
      closeAllTabs();
      expect(groups()).toHaveLength(1);
      expect(groups()[0].tabs).toHaveLength(0);
    });
  });

  describe("closeOtherTabs", () => {
    it("keeps only specified tab, collapses to one group", () => {
      openSession(makeSession("s1"));
      openSession(makeSession("s2"));
      openSession(makeSession("s3"));
      splitToRight("s3");
      closeOtherTabs("s1");
      expect(groups()).toHaveLength(1);
      expect(groups()[0].tabs).toHaveLength(1);
      expect(groups()[0].tabs[0].id).toBe("s1");
    });
  });

  describe("closeTabsToRight", () => {
    it("removes tabs after the specified one in same group", () => {
      openSession(makeSession("s1"));
      openSession(makeSession("s2"));
      openSession(makeSession("s3"));
      closeTabsToRight("s1");
      expect(groups()[0].tabs).toHaveLength(1);
    });
  });

  describe("splitToRight", () => {
    it("creates new group with the tab", () => {
      openSession(makeSession("s1"));
      openSession(makeSession("s2"));
      splitToRight("s2");
      expect(groups()).toHaveLength(2);
      expect(groups()[0].tabs.map((t) => t.id)).toEqual(["s1"]);
      expect(groups()[1].tabs.map((t) => t.id)).toEqual(["s2"]);
    });

    it("splits flexBasis 50/50 from source", () => {
      openSession(makeSession("s1"));
      openSession(makeSession("s2"));
      splitToRight("s2");
      expect(groups()[0].flexBasis).toBe(50);
      expect(groups()[1].flexBasis).toBe(50);
    });

    it("no-op when sole tab in only group", () => {
      openSession(makeSession("s1"));
      splitToRight("s1");
      expect(groups()).toHaveLength(1);
      expect(groups()[0].tabs).toHaveLength(1);
    });

    it("moves to existing right neighbor instead of creating", () => {
      openSession(makeSession("s1"));
      openSession(makeSession("s2"));
      openSession(makeSession("s3"));
      splitToRight("s2"); // creates group 2 with s2
      splitToRight("s1"); // right neighbor exists (group with s2)
      expect(groups()).toHaveLength(2);
      expect(groups()[1].tabs.map((t) => t.id)).toContain("s1");
    });
  });

  describe("moveTabToGroup", () => {
    it("moves tab between groups", () => {
      openSession(makeSession("s1"));
      openSession(makeSession("s2"));
      splitToRight("s2");
      const g2Id = groups()[1].id;
      moveTabToGroup("s1", g2Id);
      expect(groups().find((g) => g.id === g2Id)!.tabs).toHaveLength(2);
    });

    it("reorders within same group", () => {
      openSession(makeSession("s1"));
      openSession(makeSession("s2"));
      openSession(makeSession("s3"));
      const gId = groups()[0].id;
      moveTabToGroup("s3", gId, 0);
      expect(groups()[0].tabs.map((t) => t.id)).toEqual(["s3", "s1", "s2"]);
    });
  });

  describe("createGroupFromDrop", () => {
    it("creates new group at end", () => {
      openSession(makeSession("s1"));
      openSession(makeSession("s2"));
      createGroupFromDrop("s2");
      expect(groups()).toHaveLength(2);
      expect(groups()[1].tabs[0].id).toBe("s2");
    });

    it("respects max group limit", () => {
      openSession(makeSession("s1"));
      openSession(makeSession("s2"));
      openSession(makeSession("s3"));
      openSession(makeSession("s4"));
      openSession(makeSession("s5"));
      // build up to 4 groups via createGroupFromDrop
      createGroupFromDrop("s2"); // 2 groups
      createGroupFromDrop("s3"); // 3 groups
      createGroupFromDrop("s4"); // 4 groups
      createGroupFromDrop("s5"); // should be no-op (at MAX_GROUPS)
      expect(groups()).toHaveLength(4);
    });
  });

  describe("focusAdjacentGroup", () => {
    it("moves focus right", () => {
      openSession(makeSession("s1"));
      openSession(makeSession("s2"));
      splitToRight("s2");
      const g1Id = groups()[0].id;
      focusAdjacentGroup("left");
      expect(activeGroupId()).toBe(g1Id);
      focusAdjacentGroup("right");
      expect(activeGroupId()).toBe(groups()[1].id);
    });

    it("no-op at boundary", () => {
      openSession(makeSession("s1"));
      const gId = groups()[0].id;
      focusAdjacentGroup("left");
      expect(activeGroupId()).toBe(gId);
    });
  });

  describe("session uniqueness", () => {
    it("session exists in only one group at a time", () => {
      openSession(makeSession("s1"));
      openSession(makeSession("s2"));
      splitToRight("s2");
      const matches = groups().filter((g) => g.tabs.some((t) => t.id === "s2"));
      expect(matches).toHaveLength(1);
    });
  });

  describe("openPreview", () => {
    it("opens session as preview tab", () => {
      openPreview(makeSession("s1"));
      expect(groups()[0].tabs).toHaveLength(1);
      expect(groups()[0].activeTabId).toBe("s1");
      expect(groups()[0].previewTabId).toBe("s1");
    });

    it("replaces existing preview tab", () => {
      openPreview(makeSession("s1"));
      openPreview(makeSession("s2"));
      expect(groups()[0].tabs).toHaveLength(1);
      expect(groups()[0].tabs[0].id).toBe("s2");
      expect(groups()[0].previewTabId).toBe("s2");
    });

    it("does not replace pinned tabs", () => {
      openSession(makeSession("s1")); // pinned
      openPreview(makeSession("s2"));
      expect(groups()[0].tabs).toHaveLength(2);
      expect(groups()[0].tabs.map((t) => t.id)).toEqual(["s1", "s2"]);
      expect(groups()[0].previewTabId).toBe("s2");
    });

    it("focuses existing tab without creating duplicate", () => {
      openSession(makeSession("s1"));
      openSession(makeSession("s2"));
      openPreview(makeSession("s1")); // already open as pinned
      expect(groups()[0].tabs).toHaveLength(2);
      expect(groups()[0].activeTabId).toBe("s1");
    });

    it("replaces preview but keeps pinned when re-previewing", () => {
      openSession(makeSession("s1")); // pinned
      openPreview(makeSession("s2")); // preview
      openPreview(makeSession("s3")); // replaces s2
      expect(groups()[0].tabs.map((t) => t.id)).toEqual(["s1", "s3"]);
      expect(groups()[0].previewTabId).toBe("s3");
    });
  });

  describe("pinTab", () => {
    it("pins a preview tab", () => {
      openPreview(makeSession("s1"));
      expect(groups()[0].previewTabId).toBe("s1");
      pinTab("s1");
      expect(groups()[0].previewTabId).toBeNull();
      expect(groups()[0].tabs).toHaveLength(1); // tab still there
    });

    it("no-op for already pinned tab", () => {
      openSession(makeSession("s1"));
      pinTab("s1"); // not a preview, should be no-op
      expect(groups()[0].previewTabId).toBeNull();
    });

    it("pinned tab is not replaced by next preview", () => {
      openPreview(makeSession("s1"));
      pinTab("s1");
      openPreview(makeSession("s2"));
      expect(groups()[0].tabs).toHaveLength(2);
      expect(groups()[0].tabs.map((t) => t.id)).toEqual(["s1", "s2"]);
    });
  });

  describe("preview interactions with other actions", () => {
    it("closeTab clears previewTabId", () => {
      openPreview(makeSession("s1"));
      closeTab("s1");
      expect(groups()[0].previewTabId).toBeNull();
    });

    it("openSession on preview tab pins it", () => {
      openPreview(makeSession("s1"));
      expect(groups()[0].previewTabId).toBe("s1");
      openSession(makeSession("s1")); // explicit open = pin
      expect(groups()[0].previewTabId).toBeNull();
    });

    it("closeOtherTabs preserves preview if it is the kept tab", () => {
      openSession(makeSession("s1"));
      openPreview(makeSession("s2"));
      closeOtherTabs("s2");
      expect(groups()[0].tabs).toHaveLength(1);
      expect(groups()[0].previewTabId).toBe("s2");
    });

    it("closeOtherTabs clears preview if it is not the kept tab", () => {
      openSession(makeSession("s1"));
      openPreview(makeSession("s2"));
      closeOtherTabs("s1");
      expect(groups()[0].tabs).toHaveLength(1);
      expect(groups()[0].previewTabId).toBeNull();
    });

    it("splitToRight clears preview from source group", () => {
      openSession(makeSession("s1"));
      openPreview(makeSession("s2"));
      splitToRight("s2");
      expect(groups()[0].previewTabId).toBeNull();
      // split is an explicit action, so target group should not have preview
      expect(groups()[1].previewTabId).toBeNull();
    });

    it("moveTabToGroup clears preview from source", () => {
      openSession(makeSession("s1"));
      openPreview(makeSession("s2"));
      openSession(makeSession("s3"));
      splitToRight("s3");
      const g2Id = groups()[1].id;
      moveTabToGroup("s2", g2Id);
      expect(groups()[0].previewTabId).toBeNull();
    });
  });

  describe("mergeAllGroups", () => {
    it("collapses split groups into one, keeping tabs and focus", () => {
      openSession(makeSession("s1"));
      openSession(makeSession("s2"));
      openSession(makeSession("s3"));
      splitToRight("s3");
      expect(groups()).toHaveLength(2);
      expect(activeGroupId()).toBe(groups()[1].id);

      mergeAllGroups();

      expect(groups()).toHaveLength(1);
      expect(groups()[0].tabs.map((t) => t.id)).toEqual(["s1", "s2", "s3"]);
      expect(groups()[0].activeTabId).toBe("s3");
      expect(groups()[0].flexBasis).toBe(100);
      expect(activeGroupId()).toBe(groups()[0].id);
    });

    it("is a no-op with a single group", () => {
      openSession(makeSession("s1"));
      const before = groups();
      mergeAllGroups();
      expect(groups()).toBe(before);
    });
  });
});
