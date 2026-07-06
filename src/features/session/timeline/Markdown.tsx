import { Check, Copy, ExternalLink } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
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
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
import { useI18n } from "@/i18n/index";
import { openExternalUrl } from "@/lib/external-links";
import { toastError } from "@/stores/toast";

// Models write inline math as $E=mc^2$; the plugin default only accepts $$…$$.
const math = createMathPlugin({ singleDollarTextMath: true });
const code = createCodePlugin({ themes: ["github-light", "github-dark"] });

/** Streamdown's built-in control labels default to English — feed them from
 * the app locale. The `markdown.*` keys mirror StreamdownTranslations field
 * names one-to-one. */
const STREAMDOWN_TRANSLATION_KEYS = Object.keys(
  defaultTranslations,
) as (keyof StreamdownTranslations)[];

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
    const timeout = window.setTimeout(() => setCopied(false), 1500);
    return () => window.clearTimeout(timeout);
  }, [copied]);

  if (!isOpen) return null;

  const copyLink = async () => {
    await navigator.clipboard.writeText(url);
    setCopied(true);
  };
  const openLink = () => {
    onClose();
    openExternalUrl(url).catch((error: unknown) => {
      toastError(String(error));
    });
  };

  return (
    <Dialog open={isOpen} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="flex max-w-md flex-col gap-4">
        <div className="space-y-2">
          <DialogTitle className="flex items-center gap-2">
            <ExternalLink className="size-4" aria-hidden="true" />
            {t("markdown.openExternalLink")}
          </DialogTitle>
          <DialogDescription>
            {t("markdown.externalLinkWarning")}
          </DialogDescription>
        </div>
        <pre className="max-h-32 overflow-auto rounded-lg bg-surface-code p-3 font-mono text-sm whitespace-pre-wrap break-all">
          {url}
        </pre>
        <div className="grid grid-cols-2 gap-2">
          <Button
            type="button"
            variant="outline"
            onClick={() => void copyLink()}
          >
            {copied ? (
              <Check className="size-4" aria-hidden="true" />
            ) : (
              <Copy className="size-4" aria-hidden="true" />
            )}
            {copied ? t("markdown.copied") : t("markdown.copyLink")}
          </Button>
          <Button type="button" onClick={openLink}>
            <ExternalLink className="size-4" aria-hidden="true" />
            {t("markdown.openLink")}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

const linkSafety: LinkSafetyConfig = {
  enabled: true,
  renderModal: (props) => <ExternalLinkModal {...props} />,
};

/**
 * Timeline markdown renders through Streamdown: shiki-highlighted code blocks
 * with a copy control, mermaid diagrams, GFM, KaTeX math, CJK emphasis fixes.
 * This is a viewer — incomplete-markdown parsing and the typing caret are only
 * enabled for the message currently streaming in under live watch.
 */
export function Markdown({
  text,
  streaming = false,
}: {
  text: string;
  /** True only under live watch for the trailing assistant message. */
  streaming?: boolean;
}) {
  const translations = useStreamdownTranslations();
  return (
    <div className="timeline-markdown min-w-0 text-[13px] leading-relaxed text-text-primary">
      <Streamdown
        className="space-y-3"
        parseIncompleteMarkdown={streaming}
        shikiTheme={["github-light", "github-dark"]}
        plugins={{ cjk, math, code, mermaid }}
        translations={translations}
        linkSafety={linkSafety}
        {...(streaming ? { caret: "block" as const, isAnimating: true } : {})}
      >
        {text}
      </Streamdown>
    </div>
  );
}
