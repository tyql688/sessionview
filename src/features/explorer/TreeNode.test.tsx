import { fireEvent, render } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { TreeNodeComponent } from "@/features/explorer/TreeNode";
import type { TreeNode } from "@/lib/types";

const child: TreeNode = {
  id: "session",
  label: "Session",
  node_type: "session",
  children: [],
  count: 0,
  provider: "claude",
};

const parent: TreeNode = {
  id: "provider",
  label: "Claude",
  node_type: "provider",
  children: [child],
  count: 1,
  provider: "claude",
};

function renderTree() {
  function Harness() {
    const [expanded, setExpanded] = useState(false);
    const [focusedNodeId, setFocusedNodeId] = useState<string | null>(parent.id);
    return (
      <div role="tree" aria-label="Sessions">
        <TreeNodeComponent
          node={parent}
          depth={0}
          activeSessionId={null}
          focusedNodeId={focusedNodeId}
          onNodeFocus={setFocusedNodeId}
          isNodeExpanded={() => expanded}
          toggleExpanded={() => setExpanded((value) => !value)}
          onSessionContextMenu={vi.fn()}
          onNodeContextMenu={vi.fn()}
          onSessionClick={vi.fn()}
        />
      </div>
    );
  }
  return render(<Harness />);
}

describe("TreeNodeComponent", () => {
  it("uses roving focus and arrow keys to expand and enter children", () => {
    const { getAllByRole } = renderTree();
    const provider = getAllByRole("treeitem")[0];

    expect(provider).toHaveAttribute("tabindex", "0");
    expect(provider).toHaveAttribute("aria-expanded", "false");

    fireEvent.keyDown(provider, { key: "ArrowRight" });
    const expandedItems = getAllByRole("treeitem");
    expect(expandedItems).toHaveLength(2);
    expect(expandedItems[0]).toHaveAttribute("aria-expanded", "true");
    expect(expandedItems[1]).toHaveAttribute("tabindex", "-1");

    expandedItems[0].focus();
    fireEvent.keyDown(expandedItems[0], { key: "ArrowRight" });
    expect(expandedItems[1]).toHaveFocus();
    expect(expandedItems[1]).toHaveAttribute("tabindex", "0");
    expect(expandedItems[0]).toHaveAttribute("tabindex", "-1");
  });
});
