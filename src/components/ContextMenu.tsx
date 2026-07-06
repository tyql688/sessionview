import { useEffect } from "react";

export interface MenuItemDef {
  label: string | (() => string);
  shortcut?: string;
  danger?: boolean;
  separator?: boolean;
  onClick: () => void;
}

interface Props {
  items: MenuItemDef[];
  position: { x: number; y: number } | null;
  onClose: () => void;
}

export function ContextMenu(props: Props) {
  useEffect(() => {
    const handleDocClick = () => props.onClose();
    document.addEventListener("click", handleDocClick);
    return () => document.removeEventListener("click", handleDocClick);
    // onClose is the only reactive read; the "destructure props" hint is noise here.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.onClose]);

  const pos = props.position;
  if (!pos) return null;

  return (
    <div
      className="context-menu"
      ref={(el) => {
        if (!el) return;
        requestAnimationFrame(() => {
          const rect = el.getBoundingClientRect();
          const vw = window.innerWidth;
          const vh = window.innerHeight;
          if (rect.right > vw)
            el.style.left = `${Math.max(4, pos.x - rect.width)}px`;
          if (rect.bottom > vh)
            el.style.top = `${Math.max(4, pos.y - rect.height)}px`;
        });
      }}
      style={{ left: `${pos.x}px`, top: `${pos.y}px` }}
      onClick={(e) => e.stopPropagation()}
    >
      {props.items.map((item, i) =>
        !item.separator ? (
          <button
            key={i}
            className={`context-menu-item${item.danger ? " danger" : ""}`}
            onClick={() => {
              item.onClick();
              props.onClose();
            }}
          >
            <span>
              {typeof item.label === "function" ? item.label() : item.label}
            </span>
            {item.shortcut && <span className="shortcut">{item.shortcut}</span>}
          </button>
        ) : (
          <div key={i} className="context-menu-separator" />
        ),
      )}
    </div>
  );
}
