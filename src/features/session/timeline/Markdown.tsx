import { Check, Copy, ExternalLink } from "lucide-react";
import { memo, useEffect, useMemo, useState } from "react";
// @streamdown/math renders KaTeX markup but does NOT inject its stylesheet —
// without this import every formula renders twice (KaTeX HTML plus the
// unhidden MathML fallback copy).
import "katex/dist/katex.min.css";
// Streamdown 2.x made syntax highlighting a plugin: `shikiTheme` alone is
// inert without @streamdown/code supplying the actual shiki highlighter.
// CommonMark's emphasis flanking rules break on CJK punctuation (`**加粗**：`
// refuses to bold) — @streamdown/cjk bundles the remark-cjk-friendly fixes.
import { cjk } from "@streamdown/cjk";
import { createCodePlugin } from "@streamdown/code";
import { createMathPlugin } from "@streamdown/math";
import { mermaid } from "@streamdown/mermaid";
import {
  defaultTranslations,
  type LinkSafetyConfig,
  type LinkSafetyModalProps,
  Streamdown,
  type StreamdownTranslations,
} from "streamdown";
import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { useI18n } from "@/i18n/index";
import { useResolvedTheme } from "@/stores/theme";
import { openExternalUrl } from "@/lib/external-links";
import { toastError } from "@/stores/toast";
import { COPY_FEEDBACK_MS } from "@/features/session/MessageBubble/TokenUsage";

// Models write inline math as $E=mc^2$; the plugin default only accepts $$…$$.
const math = createMathPlugin({ singleDollarTextMath: true });
const code = createCodePlugin({ themes: ["github-light", "github-dark"] });

/** Streamdown's built-in control labels default to English — feed them from
 * the app locale. The `markdown.*` keys mirror StreamdownTranslations field
 * names one-to-one. */
const STREAMDOWN_TRANSLATION_KEYS = Object.keys(defaultTranslations) as (keyof StreamdownTranslations)[];

function useStreamdownTranslations(): StreamdownTranslations {
  const { t } = useI18n();
  return useMemo(
    () =>
      Object.fromEntries(
        STREAMDOWN_TRANSLATION_KEYS.map((key) => [key, t(`markdown.${key}`)]),
      ) as unknown as StreamdownTranslations,
    [t],
  );
}

function ExternalLinkModal({ isOpen, onClose, url }: LinkSafetyModalProps) {
  const { t } = useI18n();
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!copied) return;
    const timeout = window.setTimeout(() => setCopied(false), COPY_FEEDBACK_MS);
    return () => window.clearTimeout(timeout);
  }, [copied]);

  const copyLink = async () => {
    await navigator.clipboard.writeText(url);
    setCopied(true);
  };
  const openLink = () => {
    openExternalUrl(url).catch((error: unknown) => {
      toastError(String(error));
    });
  };

  return (
    <Dialog open={isOpen} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="max-w-sm gap-3" showCloseButton={false}>
        <DialogTitle>{t("markdown.openExternalLink")}</DialogTitle>
        <DialogDescription className="-mt-1">{t("markdown.externalLinkWarning")}</DialogDescription>
        <div className="max-h-24 overflow-auto rounded-md border border-border-subtle bg-surface-code px-2.5 py-2 font-mono text-xs break-all text-text-secondary">
          {url}
        </div>
        <div className="flex items-center justify-end gap-2">
          <Button type="button" variant="ghost" size="sm" onClick={() => void copyLink()}>
            {copied ? (
              <Check className="size-3.5" aria-hidden="true" />
            ) : (
              <Copy className="size-3.5" aria-hidden="true" />
            )}
            {copied ? t("markdown.copied") : t("markdown.copyLink")}
          </Button>
          <DialogClose render={<Button type="button" size="sm" />} onClick={openLink}>
            <ExternalLink className="size-3.5" aria-hidden="true" />
            {t("markdown.openLink")}
          </DialogClose>
        </div>
      </DialogContent>
    </Dialog>
  );
}

const linkSafety: LinkSafetyConfig = {
  enabled: true,
  renderModal: (props) => <ExternalLinkModal {...props} />,
};

/* A viewer needs far fewer chrome buttons than a chat product: copy is the
 * core affordance; downloading code/tables out of a session transcript is
 * noise. Mermaid keeps fullscreen + pan-zoom for reading large diagrams. */
const controls = {
  code: { copy: true, download: false },
  table: { copy: true, download: false, fullscreen: false },
  mermaid: { copy: false, download: false, fullscreen: true, panZoom: true },
};

/**
 * Timeline markdown renders through Streamdown: shiki-highlighted code blocks
 * with a copy control, mermaid diagrams, GFM, KaTeX math, CJK emphasis fixes.
 * This is a viewer; the optional streaming mode is reserved for transient
 * trailing assistant content.
 */
export const Markdown = memo(function Markdown({
  text,
  streaming = false,
}: {
  text: string;
  /** True while rendering transient trailing assistant content. */
  streaming?: boolean;
}) {
  const translations = useStreamdownTranslations();
  const resolvedTheme = useResolvedTheme();
  // Mermaid rasterizes its own colors — it must be told the concrete theme.
  // useMaxWidth (its default) shrinks diagrams to fit the column, leaving
  // them squat and unreadable; natural size + a scrollable container reads
  // far better inside a bubble.
  const mermaidOptions = useMemo(() => {
    const natural = { useMaxWidth: false };
    return {
      config: {
        theme: resolvedTheme === "dark" ? ("dark" as const) : ("default" as const),
        flowchart: natural,
        sequence: natural,
        gantt: natural,
        er: natural,
        journey: natural,
        state: natural,
        class: natural,
        pie: natural,
      },
    };
  }, [resolvedTheme]);
  return (
    <div className="timeline-markdown min-w-0 text-base leading-relaxed text-text-primary">
      <Streamdown
        className="space-y-3"
        parseIncompleteMarkdown={streaming}
        shikiTheme={["github-light", "github-dark"]}
        plugins={{ cjk, math, code, mermaid }}
        mermaid={mermaidOptions}
        translations={translations}
        controls={controls}
        linkSafety={linkSafety}
        {...(streaming ? { caret: "block" as const, isAnimating: true } : {})}
      >
        {text}
      </Streamdown>
    </div>
  );
});
