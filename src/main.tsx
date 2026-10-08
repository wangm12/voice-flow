import ReactDOM from "react-dom/client";
import App from "./App";
import { I18nProvider } from "./lib/i18n";

// macOS uses an overlay titlebar; other platforms retain their native frame.
document.documentElement.dataset.platform = /Mac/i.test(navigator.userAgent) ? "macos" : "other";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <>
    <div data-tauri-drag-region aria-hidden="true" className="vf-window-drag-region" />
    <I18nProvider>
      <App />
    </I18nProvider>
  </>,
);
