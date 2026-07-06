import { fireEvent, render, waitFor } from "@testing-library/react";
import { useState } from "react";
import { beforeAll, describe, expect, it } from "vitest";
import { SessionSearch } from "./SessionSearch";

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

function markedContainer(count: number): HTMLDivElement {
  const div = document.createElement("div");
  div.innerHTML = Array.from(
    { length: count },
    () => '<mark class="search-highlight">x</mark>',
  ).join("");
  document.body.appendChild(div);
  return div;
}

describe("SessionSearch", () => {
  it("navigates matches via the live accessor even when the ref was undefined at mount", () => {
    // Regression for the by-value capture bug: opening the search bar during
    // load (Cmd+F) mounted SessionSearch while the messages div did not exist
    // yet, permanently capturing `undefined`. With an accessor, navigation must
    // see the container once it later mounts.
    let container: HTMLDivElement | undefined;
    const { getByLabelText, searchMatchIdx } = setup(() => container);

    // Messages div mounts *after* the search bar (post-load).
    const div = document.createElement("div");
    div.innerHTML =
      '<mark class="search-highlight">a</mark>' +
      '<mark class="search-highlight">b</mark>';
    document.body.appendChild(div);
    container = div;

    fireEvent.click(getByLabelText("Next match"));

    // 2 marks, started at idx 0, +1 -> idx 1 ("b") becomes active.
    expect(searchMatchIdx()).toBe(1);
    expect(div.querySelector("mark.search-active")?.textContent).toBe("b");

    div.remove();
  });

  it("is a no-op (no throw) when the accessor still returns undefined", () => {
    const { getByLabelText, searchMatchIdx } = setup(() => undefined);
    fireEvent.click(getByLabelText("Next match"));
    expect(searchMatchIdx()).toBe(0);
  });

  it("displays a total equal to the navigable mark count", async () => {
    // The counter total must match how many times Next advances before
    // looping: both read the same `<mark>` list. A single entry can hold many
    // marks (merged tool groups), so an entry-count total would disagree.
    const div = markedContainer(3);
    const { getByText } = setup(() => div);

    // Count is derived from the DOM after the committed query renders marks.
    await waitFor(() => expect(getByText("1/3")).toBeInTheDocument());

    div.remove();
  });

  it("keeps the counter total in sync with navigation across the full cycle", async () => {
    const div = markedContainer(3);
    const { getByLabelText, getByText } = setup(() => div);

    await waitFor(() => expect(getByText("1/3")).toBeInTheDocument());

    // Next three times must cycle 1/3 -> 2/3 -> 3/3 -> 1/3 (loop), proving the
    // displayed total equals the number of navigable marks.
    fireEvent.click(getByLabelText("Next match"));
    expect(getByText("2/3")).toBeInTheDocument();
    fireEvent.click(getByLabelText("Next match"));
    expect(getByText("3/3")).toBeInTheDocument();
    fireEvent.click(getByLabelText("Next match"));
    expect(getByText("1/3")).toBeInTheDocument();

    div.remove();
  });
});
