import { Copy, Minus, Square, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useI18n } from "@/i18n/index";

export interface TitleBarProps {
  showWindowControls: boolean;
  isMaximized: boolean;
  onMinimize: () => void;
  onToggleMaximize: () => void;
  onClose: () => void;
  onStartDragging: () => void;
}

function isInteractiveTitlebarTarget(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && target.closest("input, button") !== null;
}

export function TitleBar(props: TitleBarProps) {
  const { t } = useI18n();

  return (
    <header
      className="titlebar"
      onMouseDown={(e) => {
        if (e.buttons !== 1) return;
        if (isInteractiveTitlebarTarget(e.target)) return;
        e.preventDefault();
        if (e.detail === 2) {
          props.onToggleMaximize();
        } else {
          props.onStartDragging();
        }
      }}
    >
      <div className="titlebar-center" aria-hidden="true">
        <span className="app-name">SessionView</span>
      </div>
      <div className="titlebar-right" />

      {props.showWindowControls && (
        <div className="win-controls">
          <Button
            variant="ghost"
            size="icon-sm"
            type="button"
            className="win-ctrl-btn active:translate-y-0"
            aria-label={t("window.minimize")}
            onClick={props.onMinimize}
          >
            <Minus className="size-3" aria-hidden="true" />
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            type="button"
            className="win-ctrl-btn active:translate-y-0"
            aria-label={props.isMaximized ? t("window.restore") : t("window.maximize")}
            onClick={props.onToggleMaximize}
          >
            {props.isMaximized ? (
              <Copy className="size-3" aria-hidden="true" />
            ) : (
              <Square className="size-3" aria-hidden="true" />
            )}
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            type="button"
            className="win-ctrl-btn close active:translate-y-0"
            aria-label={t("window.close")}
            onClick={props.onClose}
          >
            <X className="size-3" aria-hidden="true" />
          </Button>
        </div>
      )}
    </header>
  );
}
