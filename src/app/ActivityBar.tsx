import { Ban, BarChart3, FolderKanban, Home, Settings, Star, type LucideIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { useI18n } from "@/i18n/index";

interface ActivityItem {
  id: string;
  label: string;
  icon: LucideIcon;
  position?: "bottom";
}

interface ActivityBarProps {
  activeView: string;
  onViewChange: (view: string) => void;
  /** "horizontal" renders the compact bottom-navigation variant. */
  orientation?: "vertical" | "horizontal";
}

export function ActivityBar(props: ActivityBarProps) {
  const { t } = useI18n();
  const horizontal = props.orientation === "horizontal";
  const tooltipSide = horizontal ? ("top" as const) : ("right" as const);
  const items: ActivityItem[] = [
    { id: "explorer", label: t("explorer.title"), icon: Home },
    { id: "favorites", label: t("favorites.title"), icon: Star },
    { id: "usage", label: t("usage.title"), icon: BarChart3 },
    { id: "folderAnalytics", label: t("usage.folderAnalyticsTitle"), icon: FolderKanban },
    { id: "blocked", label: t("settings.blockedFolders"), icon: Ban },
    { id: "settings", label: t("settings.title"), icon: Settings, position: "bottom" },
  ];
  const renderItem = (item: ActivityItem) => {
    const Icon = item.icon;
    const active = props.activeView === item.id;
    return (
      <Tooltip key={item.id}>
        <TooltipTrigger
          render={
            <Button
              variant="ghost"
              className={`activity-btn active:translate-y-0${active ? " active" : ""}`}
              onClick={() => props.onViewChange(item.id)}
              aria-label={item.label}
              aria-current={active ? "page" : undefined}
            />
          }
        >
          <Icon size={20} strokeWidth={1.7} aria-hidden="true" />
        </TooltipTrigger>
        <TooltipContent side={tooltipSide}>{item.label}</TooltipContent>
      </Tooltip>
    );
  };

  return (
    <TooltipProvider>
      <nav className={`activity-bar${horizontal ? " horizontal" : ""}`} aria-label={t("keyboard.navigation")}>
        <div className="activity-bar-top">{items.filter((item) => item.position !== "bottom").map(renderItem)}</div>
        <div className="activity-bar-spacer" />
        <div className="activity-bar-bottom">{items.filter((item) => item.position === "bottom").map(renderItem)}</div>
      </nav>
    </TooltipProvider>
  );
}
