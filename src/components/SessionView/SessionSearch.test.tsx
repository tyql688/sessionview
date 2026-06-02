import { fireEvent, render } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { beforeAll, describe, expect, it } from "vitest";
import { SessionSearch } from "./SessionSearch";

// happy-dom doesn't implement scrollIntoView; navigateSearchMatch calls it.
beforeAll(() => {
  Element.prototype.scrollIntoView = () => {};
});

function setup(messagesRef: () => HTMLDivElement | undefined) {
  const [sessionSearch, setSessionSearch] = createSignal("foo");
  const [activeSessionSearch] = createSignal("foo");
  const [searchMatchIdx, setSearchMatchIdx] = createSignal(0);
  const [searchMatchCount] = createSignal(2);
  const [, setSearchBarOpen] = createSignal(true);
  const result = render(() => (
    <SessionSearch
      sessionSearch={sessionSearch}
      activeSessionSearch={activeSessionSearch}
      setSessionSearch={setSessionSearch}
      searchMatchIdx={searchMatchIdx}
      searchMatchCount={searchMatchCount}
      setSearchMatchIdx={setSearchMatchIdx}
      setSearchBarOpen={setSearchBarOpen}
      messagesRef={messagesRef}
    />
  ));
  return { ...result, searchMatchIdx };
}

describe("SessionSearch", () => {
  it("navigates matches via the live accessor even when the ref was undefined at mount", () => {
    // Regression for the by-value capture bug: opening the search bar during
    // load (Cmd+F) mounted SessionSearch while the messages div did not exist
    // yet, permanently capturing `undefined`. With an accessor, navigation must
    // see the container once it later mounts.
    const [container, setContainer] = createSignal<HTMLDivElement | undefined>(
      undefined,
    );
    const { getByLabelText, searchMatchIdx } = setup(container);

    // Messages div mounts *after* the search bar (post-load).
    const div = document.createElement("div");
    div.innerHTML =
      '<mark class="search-highlight">a</mark>' +
      '<mark class="search-highlight">b</mark>';
    document.body.appendChild(div);
    setContainer(div);

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
});
