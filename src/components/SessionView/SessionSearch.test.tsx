import { fireEvent, render, waitFor } from "@testing-library/react";
import { useState } from "react";
import { beforeAll, describe, expect, it } from "vitest";
import { collectSearchRanges } from "@/components/SessionView/search-utils";
import { SessionSearch } from "@/components/SessionView/SessionSearch";

// happy-dom doesn't implement scrollIntoView; navigateSearchMatch calls it.
beforeAll(() => {
  Element.prototype.scrollIntoView = () => {};
});

function setup(messagesRef: () => HTMLDivElement | undefined) {
  // The search-match index lives inside the React harness; mirror the latest
  // committed value into a closure var so tests can read it after fireEvent.
  let latestMatchIdx = 0;

  function Harness() {
    const [sessionSearch, setSessionSearch] = useState("foo");
    const [activeSessionSearch] = useState("foo");
    const [searchMatchIdx, setSearchMatchIdx] = useState(0);
    const [, setSearchBarOpen] = useState(true);
    latestMatchIdx = searchMatchIdx;
    return (
      <SessionSearch
        sessionSearch={sessionSearch}
        activeSessionSearch={activeSessionSearch}
        setSessionSearch={setSessionSearch}
        searchMatchIdx={searchMatchIdx}
        setSearchMatchIdx={setSearchMatchIdx}
        setSearchBarOpen={setSearchBarOpen}
        messagesRef={messagesRef}
      />
    );
  }

  const result = render(<Harness />);
  return { ...result, searchMatchIdx: () => latestMatchIdx };
}

/** A messages container whose searchable text holds `count` "foo" hits. */
function searchableContainer(count: number): HTMLDivElement {
  const div = document.createElement("div");
  div.innerHTML = `<div data-searchable="">${Array.from(
    { length: count },
    (_, i) => `<p>foo number ${i}</p>`,
  ).join("")}</div>`;
  document.body.appendChild(div);
  return div;
}

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

describe("SessionSearch", () => {
  it("navigates matches via the live accessor even when the ref was undefined at mount", () => {
    // Regression for the by-value capture bug: opening the search bar during
    // load (Cmd+F) mounted SessionSearch while the messages div did not exist
    // yet, permanently capturing `undefined`. With an accessor, navigation must
    // see the container once it later mounts.
    let container: HTMLDivElement | undefined;
    const { getByLabelText, searchMatchIdx } = setup(() => container);

    // Messages div mounts *after* the search bar (post-load).
    container = searchableContainer(2);

    fireEvent.click(getByLabelText("Next match"));

    // 2 occurrences, started at idx 0, +1 -> idx 1 becomes active.
    expect(searchMatchIdx()).toBe(1);

    container.remove();
  });

  it("is a no-op (no throw) when the accessor still returns undefined", () => {
    const { getByLabelText, searchMatchIdx } = setup(() => undefined);
    fireEvent.click(getByLabelText("Next match"));
    expect(searchMatchIdx()).toBe(0);
  });

  it("displays a total equal to the navigable occurrence count", async () => {
    // The counter total must match how many times Next advances before
    // looping: both read the same range list. A single entry can hold many
    // occurrences, so an entry-count total would disagree.
    const div = searchableContainer(3);
    const { getByText } = setup(() => div);

    // Count is derived from the DOM after the committed query renders.
    await waitFor(() => expect(getByText("1/3")).toBeInTheDocument());

    div.remove();
  });

  it("keeps the counter total in sync with navigation across the full cycle", async () => {
    const div = searchableContainer(3);
    const { getByLabelText, getByText } = setup(() => div);

    await waitFor(() => expect(getByText("1/3")).toBeInTheDocument());

    // Next three times must cycle 1/3 -> 2/3 -> 3/3 -> 1/3 (loop), proving the
    // displayed total equals the number of navigable occurrences.
    fireEvent.click(getByLabelText("Next match"));
    expect(getByText("2/3")).toBeInTheDocument();
    fireEvent.click(getByLabelText("Next match"));
    expect(getByText("3/3")).toBeInTheDocument();
    fireEvent.click(getByLabelText("Next match"));
    expect(getByText("1/3")).toBeInTheDocument();

    div.remove();
  });
});
