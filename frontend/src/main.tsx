// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import App from "./App";
import "./index.css";
import { clearToken, getEnv, setEnv, setToken } from "./api/client";
import { AuthProvider } from "./state/useAuth";
import { ThemeProvider } from "./state/theme";
import { TimezoneProvider } from "./state/timezone";

// SAML SSO landing: consume the `/sso#token=…` (or `#error=…`) fragment from
// the ACS redirect before mount, then normalize the URL to `/` for the router.
if (window.location.pathname === "/sso" && window.location.hash) {
    const frag = new URLSearchParams(window.location.hash.slice(1));
    const token = frag.get("token");
    const ssoError = frag.get("error");
    if (token) setToken(token);
    if (ssoError) sessionStorage.setItem("helios.sso_error", ssoError);
    window.history.replaceState(null, "", "/");
}

// Break-glass: `/login?local=1` drops any active session before mount so the
// password form shows (and LoginPage skips the SSO auto-redirect) for SSO users.
if (
    window.location.pathname === "/login" &&
    new URLSearchParams(window.location.search).get("local") === "1"
) {
    clearToken();
}

// The URL is authoritative for the active env: adopt a `?env=` (e.g. a shared
// link) before anything renders or fetches so the whole app agrees.
const urlEnv = new URLSearchParams(window.location.search).get("env")?.trim();
if (urlEnv && urlEnv !== getEnv()) setEnv(urlEnv);

createRoot(document.getElementById("root")!).render(
    <StrictMode>
        <BrowserRouter>
            <AuthProvider>
                <ThemeProvider>
                    <TimezoneProvider>
                        <App />
                    </TimezoneProvider>
                </ThemeProvider>
            </AuthProvider>
        </BrowserRouter>
    </StrictMode>,
);
