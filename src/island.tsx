import ReactDOM from "react-dom/client";
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { IslandWindow } from "./components/Island/IslandWindow";
import { detectSystemLanguage, I18nProvider, resolveUiLanguage, type UiLanguagePreference } from "./lib/i18n";
import "./islandWindow.css";
import "./island.css";

function IslandRoot() {
  const [language, setLanguage] = useState(detectSystemLanguage);

  useEffect(() => {
    let active = true;
    void invoke<{ ui_language?: string }>("get_settings")
      .then((settings) => {
        if (active && (settings.ui_language === "system" || settings.ui_language === "zh" || settings.ui_language === "en")) {
          setLanguage(resolveUiLanguage(settings.ui_language as UiLanguagePreference));
        }
      })
      .catch(() => undefined);
    const subscription = listen<string>("settings://ui-language", (event) => {
      if (active && (event.payload === "system" || event.payload === "zh" || event.payload === "en")) {
        setLanguage(resolveUiLanguage(event.payload as UiLanguagePreference));
      }
    });
    return () => {
      active = false;
      void subscription.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, []);

  return (
    <I18nProvider key={language} initialLanguage={language}>
      <IslandWindow />
    </I18nProvider>
  );
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <IslandRoot />,
);
