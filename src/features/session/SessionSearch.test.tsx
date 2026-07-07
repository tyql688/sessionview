import { fireEvent, render } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import {
  activeMatchTarget,
  buildMatchLocations,
  collectSearchRanges,
} from "@/features/session/search-utils";
import type { ProcessedEntry } from "@/features/session/hooks";
import { SessionSearch } from "@/features/session/SessionSearch";

describe("collectSearchRanges (DOM)", () => {
  it("collects occurrence ranges only inside searchable subtrees", () => {
    const container = document.createElement("div");
    container.innerHTML = [
      '<div data-searchable=""><p>alpha beta alpha</p></div>',
      "<div><p>alpha in tool output stays out</p></div>",
    ].join("");
    document.body.appendChild(container);

    const ranges = collectSearchRanges(container, "alpha");
    expect(ranges).toHaveLength(2);
    expect(ranges.map((range) => range.toString())).toEqual(["alpha", "alpha"]);
    container.remove();
  });

  it("matches case-insensitively", () => {
    const container = document.createElement("div");
    container.innerHTML = '<div data-searchable="">Foo BAR foo</div>';
    document.body.appendChild(container);
    expect(collectSearchRanges(container, "foo")).toHaveLength(2);
    expect(collectSearchRanges(container, "bar")).toHaveLength(1);
    container.remove();
  });

  it("returns an empty list without a container or term", () => {
    expect(collectSearchRanges(undefined, "x")).toEqual([]);
    const container = document.createElement("div");
    expect(collectSearchRanges(container, "  ")).toEqual([]);
  });
});

function messageEntry(index: number, content: string): ProcessedEntry {
  return {
    key: `msg-${index}`,
    type: "message",
    msg: {
      role: "user",
      content,
      timestamp: null,
      tool_name: null,
      tool_input: null,
      token_usage: null,
    },
    messageIndex: index,
    searchHaystack: content.toLocaleLowerCase(),
  };
}

describe("buildMatchLocations", () => {
  it("emits one location per occurrence, in entry order", () => {
    const entries = [
      messageEntry(0, "foo bar foo"),
      messageEntry(1, "nothing here"),
      messageEntry(2, "FOO again"),
    ];
    // Data-level counting: entry 0 holds two occurrences, entry 2 one —
    // independent of which rows the virtualizer has mounted.
    expect(buildMatchLocations(entries, "foo")).toEqual([0, 0, 2]);
  });

  it("returns nothing for a blank term", () => {
    expect(buildMatchLocations([messageEntry(0, "foo")], "  ")).toEqual([]);
  });
});

describe("activeMatchTarget", () => {
  it("addresses a match as entry + nth occurrence within that entry", () => {
    const locations = [0, 0, 2];
    expect(activeMatchTarget(locations, 0)).toEqual({
      entryIndex: 0,
      occurrence: 0,
    });
    expect(activeMatchTarget(locations, 1)).toEqual({
      entryIndex: 0,
      occurrence: 1,
    });
    expect(activeMatchTarget(locations, 2)).toEqual({
      entryIndex: 2,
      occurrence: 0,
    });
    expect(activeMatchTarget(locations, 3)).toBeNull();
  });
});

function setup(matchTotal: number, navigateMatch = vi.fn()) {
  function Harness() {
    const [sessionSearch, setSessionSearch] = useState("foo");
    const [, setSearchBarOpen] = useState(true);
    return (
      <SessionSearch
        sessionSearch={sessionSearch}
        activeSessionSearch="foo"
        setSessionSearch={setSessionSearch}
        searchMatchIdx={0}
        matchTotal={matchTotal}
        navigateMatch={navigateMatch}
        setSearchBarOpen={setSearchBarOpen}
      />
    );
  }

  return { ...render(<Harness />), navigateMatch };
}

describe("SessionSearch", () => {
  it("displays the data-level total with a 1-based active index", () => {
    const { getByText } = setup(3);
    expect(getByText("1/3")).toBeInTheDocument();
  });

  it("shows the no-match label when the committed query has zero hits", () => {
    const { getByText } = setup(0);
    expect(getByText("No matches")).toBeInTheDocument();
  });

  it("routes Enter / Shift+Enter and the arrow buttons to navigation", () => {
    const { getByLabelText, container, navigateMatch } = setup(3);
    const input = container.querySelector<HTMLInputElement>(
      ".session-search-input",
    );
    expect(input).not.toBeNull();

    fireEvent.keyDown(input!, { key: "Enter" });
    expect(navigateMatch).toHaveBeenLastCalledWith(1);
    fireEvent.keyDown(input!, { key: "Enter", shiftKey: true });
    expect(navigateMatch).toHaveBeenLastCalledWith(-1);
    fireEvent.click(getByLabelText("Next match"));
    expect(navigateMatch).toHaveBeenLastCalledWith(1);
    fireEvent.click(getByLabelText("Previous match"));
    expect(navigateMatch).toHaveBeenLastCalledWith(-1);
  });
});
