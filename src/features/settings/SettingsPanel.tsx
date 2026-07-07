import { useEffect, useState } from "react";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { useI18n } from "@/i18n/index";
import { GeneralSettings } from "@/features/settings/GeneralSettings";
import { DataSourceSettings } from "@/features/settings/DataSourceSettings";
import { IndexSettings } from "@/features/settings/IndexSettings";
import { KeyboardSettings } from "@/features/settings/KeyboardSettings";
import { AboutSettings } from "@/features/settings/AboutSettings";
import { listProviderSnapshots, refreshProviderSnapshots } from "@/stores/providerSnapshots";
import { cn } from "@/lib/utils";

type SettingsCategory = "general" | "dataSources" | "index" | "keyboard" | "about";

export function SettingsPanel() {
  const { t } = useI18n();
  const [activeCategory, setActiveCategory] = useState<SettingsCategory>("general");

  useEffect(() => {
    if (activeCategory === "dataSources") {
      void refreshProviderSnapshots();
    }
  }, [activeCategory]);

  const categories = [
    {
      id: "general" as SettingsCategory,
      labelKey: "settings.general" as const,
    },
    {
      id: "dataSources" as SettingsCategory,
      labelKey: "settings.dataSources" as const,
    },
    { id: "index" as SettingsCategory, labelKey: "settings.index" as const },
    { id: "keyboard" as SettingsCategory, labelKey: "keyboard.title" as const },
    { id: "about" as SettingsCategory, labelKey: "settings.about" as const },
  ];

  function handleIndexChanged() {
    void refreshProviderSnapshots();
  }

  return (
    <div className="settings-panel">
      <ToggleGroup
        className="settings-sidebar"
        orientation="vertical"
        spacing={0}
        value={[activeCategory]}
        onValueChange={(next) => {
          const value = next[0];
          if (
            value === "general" ||
            value === "dataSources" ||
            value === "index" ||
            value === "keyboard" ||
            value === "about"
          ) {
            setActiveCategory(value);
          }
        }}
      >
        {categories.map((cat) => (
          <ToggleGroupItem
            key={cat.id}
            value={cat.id}
            className={cn(
              "settings-nav-item h-auto min-w-0 justify-start rounded-none",
              activeCategory === cat.id && "active",
            )}
          >
            {t(cat.labelKey)}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>

      <div className="settings-content">
        {activeCategory === "general" && <GeneralSettings />}

        {activeCategory === "dataSources" && <DataSourceSettings providerSnapshots={listProviderSnapshots} />}

        {activeCategory === "index" && <IndexSettings onIndexChanged={handleIndexChanged} />}

        {activeCategory === "keyboard" && <KeyboardSettings />}

        {activeCategory === "about" && <AboutSettings />}
      </div>
    </div>
  );
}
