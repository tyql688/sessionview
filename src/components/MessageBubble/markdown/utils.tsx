import { For, type JSX } from "solid-js";
import { isExternalUrl } from "../../../lib/external-links";

export function wrapHighlight(text: string, term?: string): JSX.Element {
  if (!term) return <>{text}</>;
  const escaped = term.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const parts = text.split(new RegExp(`(${escaped})`, "gi"));
  const lowerTerm = term.toLowerCase();

  return (
    <For each={parts}>
      {(part) =>
        part.toLowerCase() === lowerTerm ? (
          <mark class="search-highlight">{part}</mark>
        ) : (
          part
        )
      }
    </For>
  );
}

export function isSafeUrl(url: string): boolean {
  return isExternalUrl(url);
}
