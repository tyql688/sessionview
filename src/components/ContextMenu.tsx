import { Menu } from "@base-ui/react/menu";
import { useMemo } from "react";
import { cn } from "@/lib/utils";

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

/**
 * Imperative context menu: callers open it at pointer coordinates (the
 * right-click position becomes a zero-size virtual anchor), Base UI handles
 * positioning, collision flipping, focus and dismissal.
 */
export function ContextMenu(props: Props) {
  const { position } = props;
  const anchor = useMemo(() => {
    if (!position) return null;
    return {
      getBoundingClientRect: () =>
        DOMRect.fromRect({ x: position.x, y: position.y, width: 0, height: 0 }),
    };
  }, [position]);

  return (
    <Menu.Root
      open={position !== null}
      onOpenChange={(open) => {
        if (!open) props.onClose();
      }}
      modal={false}
    >
      <Menu.Portal>
        <Menu.Positioner
          anchor={anchor}
          side="bottom"
          align="start"
          sideOffset={2}
          className="z-[300] outline-none"
        >
          <Menu.Popup className="min-w-44 rounded-lg bg-popover p-1 text-popover-foreground shadow-md ring-1 ring-foreground/10 outline-none duration-100 data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95">
            {props.items.map((item, i) =>
              item.separator ? (
                <Menu.Separator key={i} className="-mx-1 my-1 h-px bg-border" />
              ) : (
                <Menu.Item
                  key={i}
                  className={cn(
                    "flex cursor-default items-center justify-between gap-6 rounded-md px-2 py-1.5 text-sm outline-none select-none data-highlighted:bg-accent data-highlighted:text-accent-foreground",
                    item.danger &&
                      "text-destructive data-highlighted:bg-destructive/10 data-highlighted:text-destructive",
                  )}
                  onClick={() => {
                    item.onClick();
                    props.onClose();
                  }}
                >
                  <span>
                    {typeof item.label === "function"
                      ? item.label()
                      : item.label}
                  </span>
                  {item.shortcut && (
                    <span className="text-xs text-muted-foreground">
                      {item.shortcut}
                    </span>
                  )}
                </Menu.Item>
              ),
            )}
          </Menu.Popup>
        </Menu.Positioner>
      </Menu.Portal>
    </Menu.Root>
  );
}
