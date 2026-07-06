import { useEffect, useState } from "react";
import { useI18n } from "@/i18n/index";
import { GeneralSettings } from "@/components/Settings/GeneralSettings";
import { DataSourceSettings } from "@/components/Settings/DataSourceSettings";
import { IndexSettings } from "@/components/Settings/IndexSettings";
import { KeyboardSettings } from "@/components/Settings/KeyboardSettings";
import { AboutSettings } from "@/components/Settings/AboutSettings";
import {
  listProviderSnapshots,
  refreshProviderSnapshots,
} from "@/stores/providerSnapshots";

type SettingsCategory =
  | "general"
  | "dataSources"
  | "index"
  | "keyboard"
  | "about";

export function SettingsPanel() {
  const { t } = useI18n();
  const [activeCategory, setActiveCategory] =
    useState<SettingsCategory>("general");

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
      <div className="settings-sidebar">
        {categories.map((cat) => (
          <button
            key={cat.id}
            className={`settings-nav-item${activeCategory === cat.id ? " active" : ""}`}
            onClick={() => setActiveCategory(cat.id)}
          >
            {t(cat.labelKey)}
          </button>
        ))}
      </div>

      <div className="settings-content">
        {activeCategory === "general" && <GeneralSettings />}

        {activeCategory === "dataSources" && (
          <DataSourceSettings providerSnapshots={listProviderSnapshots} />
        )}

        {activeCategory === "index" && (
          <IndexSettings onIndexChanged={handleIndexChanged} />
        )}

        {activeCategory === "keyboard" && <KeyboardSettings />}

        {activeCategory === "about" && <AboutSettings />}
      </div>
    </div>
  );
}
