import { useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { cn } from "@/lib/utils";
import type { SessionMeta } from "../lib/types";
import { exportSession } from "../lib/tauri";
import { useI18n } from "../i18n/index";
import { toast, toastError } from "../stores/toast";
import { errorMessage } from "../lib/errors";

type ExportFormat = "json" | "markdown" | "html";

const FORMAT_OPTIONS: { value: ExportFormat; labelKey: string; ext: string }[] =
  [
    { value: "json", labelKey: "export.json", ext: "json" },
    { value: "markdown", labelKey: "export.markdown", ext: "md" },
    { value: "html", labelKey: "export.html", ext: "html" },
  ];

export function ExportDialog(props: {
  open: boolean;
  session: SessionMeta;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const [format, setFormat] = useState<ExportFormat>("json");
  const [exporting, setExporting] = useState(false);

  async function handleExport() {
    const selected = FORMAT_OPTIONS.find((f) => f.value === format);
    if (!selected) return;

    try {
      const outputPath = await save({
        defaultPath: `${props.session.title || "session"}.${selected.ext}`,
        filters: [
          { name: selected.value.toUpperCase(), extensions: [selected.ext] },
        ],
      });

      if (!outputPath) return;

      setExporting(true);
      await exportSession(props.session.id, selected.value, outputPath);
      props.onClose();
      toast(t("toast.exportOk"));
    } catch (e) {
      toastError(errorMessage(e));
    } finally {
      setExporting(false);
    }
  }

  return (
    <Dialog
      open={props.open}
      onOpenChange={(open) => {
        if (!open) props.onClose();
      }}
    >
      <DialogContent showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>{t("export.title")}</DialogTitle>
        </DialogHeader>
        <div className="grid grid-cols-3 gap-2">
          {FORMAT_OPTIONS.map((opt) => (
            <button
              key={opt.value}
              type="button"
              className={cn(
                "flex flex-col items-center gap-0.5 rounded-lg border px-3 py-2.5 text-sm transition-colors",
                format === opt.value
                  ? "border-primary bg-primary/10 text-foreground"
                  : "border-border text-muted-foreground hover:bg-muted",
              )}
              onClick={() => setFormat(opt.value)}
            >
              <span className="font-medium">{t(opt.labelKey)}</span>
              <span className="text-xs text-muted-foreground">.{opt.ext}</span>
            </button>
          ))}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={props.onClose}>
            {t("confirm.cancel")}
          </Button>
          <Button onClick={() => void handleExport()} disabled={exporting}>
            {exporting ? "..." : t("session.export")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
