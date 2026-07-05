import { For, type JSX } from "solid-js";
import type {
  Code,
  FootnoteDefinition,
  FootnoteReference,
  Heading,
  Image,
  ImageReference,
  Link,
  LinkReference,
  List,
  ListItem,
  Paragraph,
  PhrasingContent,
  Table,
  TableCell,
} from "mdast";
import { CodeBlock } from "../../CodeBlock";
import { MermaidBlock } from "../../MermaidBlock";
import { errorMessage } from "../../../lib/errors";
import { openExternalUrl } from "../../../lib/external-links";
import { toastError } from "../../../stores/toast";
import { LocalImage, RemoteImage, isLocalPath } from "../ImagePreview";
import { renderKatex } from "./katex";
import { isSafeUrl, wrapHighlight } from "./utils";
import { footnoteDomId, headingTagName, normalizeIdentifier } from "./parser";
import type {
  InlineMathNode,
  MarkdownBlockNode,
  MarkdownInlineNode,
  MathNode,
  RenderContext,
} from "./types";

export function renderBlockNodes(
  nodes: MarkdownBlockNode[],
  context: RenderContext,
): JSX.Element {
  return (
    <For each={nodes}>
      {(node, index) => renderBlockNode(node, context, `block-${index()}`)}
    </For>
  );
}

function renderBlockNode(
  node: MarkdownBlockNode,
  context: RenderContext,
  key: string,
): JSX.Element | null {
  switch (node.type) {
    case "paragraph":
      return renderParagraph(node, context, key);
    case "heading":
      return renderHeading(node, context, key);
    case "blockquote":
      return (
        <blockquote class="msg-blockquote">
          {renderBlockNodes(node.children, context)}
        </blockquote>
      );
    case "list":
      return renderList(node, context, key);
    case "listItem":
      return renderListItem(node, context, key);
    case "table":
      return renderTable(node, context, key);
    case "code":
      return renderCodeBlock(node, key, context);
    case "math":
      return renderMathBlock(node, key);
    case "thematicBreak":
      return <hr class="msg-hr" />;
    case "html":
      return (
        <p class="msg-text-line">
          {wrapHighlight(node.value, context.highlightTerm)}
        </p>
      );
    case "definition":
    case "footnoteDefinition":
      return null;
    default:
      return null;
  }
}

function renderParagraph(
  node: Paragraph,
  context: RenderContext,
  _key: string,
): JSX.Element {
  const segments = splitParagraphChildren(node.children);

  if (segments.length === 1 && segments[0].type === "phrasing") {
    return (
      <p class="msg-text-line">
        {renderInlineNodes(segments[0].children, context)}
      </p>
    );
  }

  return (
    <For each={segments}>
      {(segment) =>
        segment.type === "phrasing" ? (
          <p class="msg-text-line">
            {renderInlineNodes(segment.children, context)}
          </p>
        ) : (
          <div>{renderImageNode(segment.node, context)}</div>
        )
      }
    </For>
  );
}

function splitParagraphChildren(children: PhrasingContent[]) {
  const segments: Array<
    | { type: "phrasing"; children: PhrasingContent[] }
    | { type: "image"; node: Image }
  > = [];
  let current: PhrasingContent[] = [];

  for (const child of children) {
    if (child.type === "image") {
      if (current.length > 0) {
        segments.push({ type: "phrasing", children: current });
        current = [];
      }
      segments.push({ type: "image", node: child });
    } else {
      current.push(child);
    }
  }

  if (current.length > 0 || segments.length === 0) {
    segments.push({ type: "phrasing", children: current });
  }

  return segments;
}

function renderHeading(
  node: Heading,
  context: RenderContext,
  _key: string,
): JSX.Element {
  const content = renderInlineNodes(node.children, context);

  switch (headingTagName(node.depth)) {
    case "h1":
      return <h1>{content}</h1>;
    case "h2":
      return <h2>{content}</h2>;
    case "h3":
      return <h3>{content}</h3>;
    case "h4":
      return <h4>{content}</h4>;
    case "h5":
      return <h5>{content}</h5>;
    case "h6":
      return <h6>{content}</h6>;
  }
}

function renderList(
  node: List,
  context: RenderContext,
  key: string,
): JSX.Element {
  if (node.ordered) {
    return (
      <ol start={node.start ?? 1}>
        <For each={node.children}>
          {(child, index) =>
            renderListItem(child, context, `${key}-${index()}`)
          }
        </For>
      </ol>
    );
  }

  return (
    <ul>
      <For each={node.children}>
        {(child, index) => renderListItem(child, context, `${key}-${index()}`)}
      </For>
    </ul>
  );
}

function renderListItem(
  node: ListItem,
  context: RenderContext,
  key: string,
): JSX.Element {
  const isTask = typeof node.checked === "boolean";
  const onlyParagraph =
    node.children.length === 1 && node.children[0]?.type === "paragraph";

  const content = onlyParagraph
    ? renderListItemParagraph(node.children[0] as Paragraph, context, key)
    : renderBlockNodes(node.children, context);

  return (
    <li class={isTask ? "msg-task-item" : undefined}>
      {isTask && (
        <input
          class="msg-task-checkbox"
          type="checkbox"
          checked={node.checked === true}
          disabled
        />
      )}
      <div class={isTask ? "msg-task-content" : undefined}>{content}</div>
    </li>
  );
}

function renderListItemParagraph(
  node: Paragraph,
  context: RenderContext,
  _key: string,
): JSX.Element {
  const segments = splitParagraphChildren(node.children);

  if (segments.length === 1 && segments[0].type === "phrasing") {
    return <>{renderInlineNodes(segments[0].children, context)}</>;
  }

  return (
    <For each={segments}>
      {(segment) =>
        segment.type === "phrasing" ? (
          <p class="msg-text-line">
            {renderInlineNodes(segment.children, context)}
          </p>
        ) : (
          <div>{renderImageNode(segment.node, context)}</div>
        )
      }
    </For>
  );
}

function renderTable(
  node: Table,
  context: RenderContext,
  _key: string,
): JSX.Element {
  const headerRow = node.children[0];
  const bodyRows = node.children.slice(1);

  return (
    <table class="msg-table">
      <thead>
        <tr>
          <For each={headerRow.children}>
            {(cell, index) =>
              renderTableCell(
                cell,
                node.align?.[index()] ?? null,
                context,
                "th",
              )
            }
          </For>
        </tr>
      </thead>
      <tbody>
        <For each={bodyRows}>
          {(row) => (
            <tr>
              <For each={row.children}>
                {(cell, index) =>
                  renderTableCell(
                    cell,
                    node.align?.[index()] ?? null,
                    context,
                    "td",
                  )
                }
              </For>
            </tr>
          )}
        </For>
      </tbody>
    </table>
  );
}

function renderTableCell(
  node: TableCell,
  align: "left" | "right" | "center" | null | undefined,
  context: RenderContext,
  tag: "th" | "td",
): JSX.Element {
  const content = renderInlineNodes(node.children, context);
  const style = align ? { "text-align": align } : undefined;

  return tag === "th" ? (
    <th style={style}>{content}</th>
  ) : (
    <td style={style}>{content}</td>
  );
}

function renderCodeBlock(
  node: Code,
  _key: string,
  context: RenderContext,
): JSX.Element {
  if (node.lang?.toLowerCase() === "mermaid") {
    return <MermaidBlock code={node.value} />;
  }

  return (
    <CodeBlock
      code={node.value}
      language={node.lang ?? undefined}
      highlightTerm={context.highlightTerm}
    />
  );
}

function renderMathBlock(node: MathNode, _key: string): JSX.Element {
  const html = renderKatex(node.value, true);

  if (html) {
    // KaTeX output is sanitized HTML produced from controlled LaTeX input;
    // innerHTML is required because KaTeX's renderer emits a DOM string.
    // eslint-disable-next-line solid/no-innerhtml
    return <div class="katex-display-block" innerHTML={html} />;
  }

  return (
    <pre class="code-block-pre">
      <code>{`$$\n${node.value}\n$$`}</code>
    </pre>
  );
}

function renderInlineNodes(
  nodes: MarkdownInlineNode[],
  context: RenderContext,
): JSX.Element {
  return (
    <For each={nodes}>
      {(node, index) => renderInlineNode(node, context, `inline-${index()}`)}
    </For>
  );
}

function renderInlineNode(
  node: MarkdownInlineNode,
  context: RenderContext,
  key: string,
): JSX.Element | null {
  switch (node.type) {
    case "text":
      return <>{wrapHighlight(node.value, context.highlightTerm)}</>;
    case "strong":
      return <strong>{renderInlineNodes(node.children, context)}</strong>;
    case "emphasis":
      return <em>{renderInlineNodes(node.children, context)}</em>;
    case "delete":
      return <del>{renderInlineNodes(node.children, context)}</del>;
    case "inlineCode":
      return <code>{wrapHighlight(node.value, context.highlightTerm)}</code>;
    case "link":
      return renderLinkNode(node, context, key);
    case "linkReference":
      return renderLinkReferenceNode(node, context, key);
    case "image":
      return renderImageNode(node, context);
    case "imageReference":
      return renderImageReferenceNode(node, context, key);
    case "footnoteReference":
      return renderFootnoteReferenceNode(node, context);
    case "inlineMath":
      return renderInlineMathNode(node, key);
    case "break":
      return <br />;
    case "html":
      return <span>{wrapHighlight(node.value, context.highlightTerm)}</span>;
    default:
      return null;
  }
}

function renderLinkNode(
  node: Link,
  context: RenderContext,
  _key: string,
): JSX.Element {
  if (!isSafeUrl(node.url)) {
    return <span>{renderInlineNodes(node.children, context)}</span>;
  }

  return (
    <a
      href={node.url}
      rel="noopener noreferrer"
      target="_blank"
      title={node.title ?? undefined}
      onClick={(event) => {
        event.preventDefault();
        openMarkdownLink(node.url);
      }}
    >
      {renderInlineNodes(node.children, context)}
    </a>
  );
}

function openMarkdownLink(url: string): void {
  void openExternalUrl(url).catch((error: unknown) => {
    console.error("Failed to open external link:", error);
    toastError(errorMessage(error));
  });
}

function renderLinkReferenceNode(
  node: LinkReference,
  context: RenderContext,
  _key: string,
): JSX.Element {
  const definition = context.definitions.get(
    normalizeIdentifier(node.identifier),
  );
  if (!definition || !isSafeUrl(definition.url)) {
    return <span>{renderInlineNodes(node.children, context)}</span>;
  }

  return (
    <a
      href={definition.url}
      rel="noopener noreferrer"
      target="_blank"
      title={definition.title ?? undefined}
      onClick={(event) => {
        event.preventDefault();
        openMarkdownLink(definition.url);
      }}
    >
      {renderInlineNodes(node.children, context)}
    </a>
  );
}

function renderImageReferenceNode(
  node: ImageReference,
  context: RenderContext,
  _key: string,
): JSX.Element | null {
  const definition = context.definitions.get(
    normalizeIdentifier(node.identifier),
  );
  if (!definition) {
    return node.alt ? <span>{node.alt}</span> : null;
  }

  return renderImageNode(
    {
      type: "image",
      alt: node.alt,
      title: definition.title,
      url: definition.url,
    },
    context,
  );
}

function renderImageNode(node: Image, context: RenderContext): JSX.Element {
  if (isLocalPath(node.url)) {
    return (
      <LocalImage
        path={node.url}
        onPreview={(src, source) => context.onPreview(src, source)}
      />
    );
  }

  return (
    <RemoteImage
      src={node.url}
      onPreview={(src, source) => context.onPreview(src, source)}
    />
  );
}

function renderFootnoteReferenceNode(
  node: FootnoteReference,
  context: RenderContext,
): JSX.Element {
  const identifier = normalizeIdentifier(node.identifier);
  const label = String(
    context.footnoteNumbers.get(identifier) ?? node.label ?? node.identifier,
  );
  const target = context.footnoteDefinitions.has(identifier)
    ? `#${footnoteDomId(context.footnotePrefix, identifier)}`
    : undefined;

  return (
    <sup class="msg-footnote-ref">
      {target ? <a href={target}>{label}</a> : <span>{label}</span>}
    </sup>
  );
}

export function renderFootnotesSection(
  context: RenderContext,
): JSX.Element | null {
  const footnotes = context.footnoteOrder
    .map((identifier) => ({
      identifier,
      node: context.footnoteDefinitions.get(identifier),
    }))
    .filter(
      (
        entry,
      ): entry is {
        identifier: string;
        node: FootnoteDefinition;
      } => !!entry.node,
    );

  if (footnotes.length === 0) {
    return null;
  }

  return (
    <section class="msg-footnotes">
      <ol>
        <For each={footnotes}>
          {(entry) => (
            <li
              id={footnoteDomId(context.footnotePrefix, entry.identifier)}
              class="msg-footnote-item"
            >
              {renderBlockNodes(entry.node.children, context)}
            </li>
          )}
        </For>
      </ol>
    </section>
  );
}

function renderInlineMathNode(node: InlineMathNode, _key: string): JSX.Element {
  const html = renderKatex(node.value, false);

  if (html) {
    // KaTeX output is sanitized HTML produced from controlled LaTeX input;
    // innerHTML is required because KaTeX's renderer emits a DOM string.
    // eslint-disable-next-line solid/no-innerhtml
    return <span class="katex-inline" innerHTML={html} />;
  }

  return <code>{`$${node.value}$`}</code>;
}
