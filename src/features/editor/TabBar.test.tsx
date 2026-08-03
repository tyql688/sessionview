import { fireEvent, render } from "@testing-library/react";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { TabBar } from "@/features/editor/TabBar";
import type { SessionRef } from "@/lib/types";

const tabs: SessionRef[] = [
  { id: "first", provider: "claude", title: "First", project_name: "Project", is_sidechain: false },
  { id: "second", provider: "codex", title: "Second", project_name: "Project", is_sidechain: false },
];

beforeEach(() => {
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
});

function renderTabBar(onTabClose: (tabId: string) => void = () => {}) {
  function Harness() {
    const [activeTabId, setActiveTabId] = useState(tabs[0].id);
    return (
      <TabBar
        groupId="group"
        tabs={tabs}
        activeTabId={activeTabId}
        previewTabId={null}
        onTabSelect={setActiveTabId}
        onTabClose={onTabClose}
        onCloseAllTabs={() => {}}
        onCloseOtherTabs={() => {}}
        onCloseTabsToRight={() => {}}
        onSplitToRight={() => {}}
        onPinTab={() => {}}
      />
    );
  }
  return render(<Harness />);
}

describe("TabBar", () => {
  it("exposes tabs and moves selection with arrow keys", () => {
    const { getAllByRole } = renderTabBar();
    const tabButtons = getAllByRole("tab");

    expect(tabButtons).toHaveLength(2);
    expect(tabButtons[0]).toHaveAttribute("aria-selected", "true");
    expect(tabButtons[0]).toHaveAttribute("tabindex", "0");
    expect(tabButtons[1]).toHaveAttribute("tabindex", "-1");

    tabButtons[0].focus();
    fireEvent.keyDown(tabButtons[0], { key: "ArrowRight" });

    expect(tabButtons[1]).toHaveFocus();
    expect(tabButtons[1]).toHaveAttribute("aria-selected", "true");
    expect(tabButtons[1]).toHaveAttribute("tabindex", "0");
  });

  it("closes the focused tab with Delete", () => {
    const onTabClose = vi.fn();
    const { getByRole } = renderTabBar(onTabClose);

    fireEvent.keyDown(getByRole("tab", { name: "First" }), { key: "Delete" });

    expect(onTabClose).toHaveBeenCalledWith("first");
  });

  it("keeps close controls separate from tab controls", () => {
    const { getByRole } = renderTabBar();
    expect(getByRole("button", { name: "Close tab: First" })).toHaveAttribute("tabindex", "-1");
    expect(getByRole("tab", { name: "First" })).toBeInTheDocument();
  });
});
