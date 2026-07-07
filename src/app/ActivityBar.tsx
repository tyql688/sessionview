import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import type { JSX } from "react";
import { useI18n } from "@/i18n/index";

interface ActivityItem {
  id: string;
  label: string;
  icon: () => JSX.Element;
  position?: "bottom";
}

function HomeIcon() {
  return (
    <svg width="20" height="20" fill="none" stroke="currentColor" strokeWidth="1.5" viewBox="0 0 24 24">
      <path d="M3 9l9-7 9 7v11a2 2 0 01-2 2H5a2 2 0 01-2-2z" />
      <polyline points="9 22 9 12 15 12 15 22" />
    </svg>
  );
}

function StarIcon() {
  return (
    <svg width="20" height="20" fill="none" stroke="currentColor" strokeWidth="1.5" viewBox="0 0 24 24">
      <polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2" />
    </svg>
  );
}

function BlockedIcon() {
  return (
    <svg width="20" height="20" fill="none" stroke="currentColor" strokeWidth="1.5" viewBox="0 0 24 24">
      <circle cx="12" cy="12" r="10" />
      <line x1="4.93" y1="4.93" x2="19.07" y2="19.07" />
    </svg>
  );
}

function TrashIcon() {
  return (
    <svg width="20" height="20" fill="none" stroke="currentColor" strokeWidth="1.5" viewBox="0 0 24 24">
      <polyline points="3 6 5 6 21 6" />
      <path d="M19 6v14a2 2 0 01-2 2H7a2 2 0 01-2-2V6m3 0V4a2 2 0 012-2h4a2 2 0 012 2v2" />
    </svg>
  );
}

function UsageIcon() {
  return (
    <svg width="20" height="20" fill="none" stroke="currentColor" strokeWidth="1.5" viewBox="0 0 24 24">
      <rect x="3" y="12" width="4" height="9" rx="1" />
      <rect x="10" y="7" width="4" height="14" rx="1" />
      <rect x="17" y="3" width="4" height="18" rx="1" />
    </svg>
  );
}

function SettingsIcon() {
  return (
    <svg width="20" height="20" fill="none" stroke="currentColor" strokeWidth="1.5" viewBox="0 0 24 24">
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 010 2.83 2 2 0 01-2.83 0l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-4 0v-.09A1.65 1.65 0 009 19.4a1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 01-2.83 0 2 2 0 010-2.83l.06-.06A1.65 1.65 0 004.68 15a1.65 1.65 0 00-1.51-1H3a2 2 0 010-4h.09A1.65 1.65 0 004.6 9a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 112.83-2.83l.06.06A1.65 1.65 0 009 4.68a1.65 1.65 0 001-1.51V3a2 2 0 014 0v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 012.83 2.83l-.06.06A1.65 1.65 0 0019.32 9a1.65 1.65 0 001.51 1H21a2 2 0 010 4h-.09a1.65 1.65 0 00-1.51 1z" />
    </svg>
  );
}

export function ActivityBar(props: { activeView: string; onViewChange: (v: string) => void }) {
  const { t } = useI18n();
  const items: ActivityItem[] = [
    { id: "explorer", label: t("explorer.title"), icon: HomeIcon },
    { id: "favorites", label: t("favorites.title"), icon: StarIcon },
    { id: "usage", label: t("usage.title"), icon: UsageIcon },
    { id: "blocked", label: t("settings.blockedFolders"), icon: BlockedIcon },
    { id: "trash", label: t("trash.title"), icon: TrashIcon },
    {
      id: "settings",
      label: t("settings.title"),
      icon: SettingsIcon,
      position: "bottom",
    },
  ];
  const topItems = items.filter((i) => i.position !== "bottom");
  const bottomItems = items.filter((i) => i.position === "bottom");

  return (
    <TooltipProvider>
      <div className="activity-bar">
        <div className="activity-bar-top">
          {topItems.map((item) => {
            const Icon = item.icon;
            return (
              <Tooltip key={item.id}>
                <TooltipTrigger
                  render={
                    <Button
                      variant="ghost"
                      className={`activity-btn active:translate-y-0${props.activeView === item.id ? " active" : ""}`}
                      onClick={() => props.onViewChange(item.id)}
                      aria-label={item.label}
                    />
                  }
                >
                  <Icon />
                </TooltipTrigger>
                <TooltipContent side="right">{item.label}</TooltipContent>
              </Tooltip>
            );
          })}
        </div>
        <div className="activity-bar-spacer" />
        <div className="activity-bar-bottom">
          {bottomItems.map((item) => {
            const Icon = item.icon;
            return (
              <Tooltip key={item.id}>
                <TooltipTrigger
                  render={
                    <Button
                      variant="ghost"
                      className={`activity-btn active:translate-y-0${props.activeView === item.id ? " active" : ""}`}
                      onClick={() => props.onViewChange(item.id)}
                      aria-label={item.label}
                    />
                  }
                >
                  <Icon />
                </TooltipTrigger>
                <TooltipContent side="right">{item.label}</TooltipContent>
              </Tooltip>
            );
          })}
        </div>
      </div>
    </TooltipProvider>
  );
}
