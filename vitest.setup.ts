import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";
import "@testing-library/jest-dom/vitest";

// KaTeX warns when `document.compatMode` is not standards mode. happy-dom does
// not currently expose this browser property, so provide the standards-mode
// value our app runs with in real documents.
Object.defineProperty(document, "compatMode", {
  configurable: true,
  value: "CSS1Compat",
});

// Unmount components rendered by @testing-library/react between tests so DOM
// does not leak across cases.
afterEach(() => {
  cleanup();
});
