// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import {
    Bell,
    LayoutDashboard,
    Layers,
    LogOut,
    Moon,
    Search as SearchIcon,
    Settings,
    Star,
    Sun,
} from "lucide-react";
import { useEffect, useState } from "react";
import { Link, Navigate, NavLink, Route, Routes, useLocation, useNavigate } from "react-router-dom";
import { getSamlStatus, getSetupStatus } from "./api/client";
import { AgentDrawer } from "./components/AgentDrawer";
import { AlertToasts } from "./components/AlertToasts";
import { EnvPicker } from "./components/EnvPicker";
import { AccountPage } from "./pages/AccountPage";
import { AdminLayout } from "./pages/admin/AdminLayout";
import { ApiKeysPanel } from "./pages/admin/ApiKeysPanel";
import { GeneralPanel } from "./pages/admin/GeneralPanel";
import { AgentPanel } from "./pages/admin/AgentPanel";
import { EnvironmentsPanel } from "./pages/admin/EnvironmentsPanel";
import { IndexesPanel } from "./pages/admin/IndexesPanel";
import { IngestionLayout } from "./pages/admin/IngestionLayout";
import { IntegrationsPanel } from "./pages/admin/IntegrationsPanel";
import { McpPanel } from "./pages/admin/McpPanel";
import { PushTokensPanel } from "./pages/admin/PushTokensPanel";
import { SamlPanel } from "./pages/admin/SamlPanel";
import { SourcesPanel } from "./pages/admin/SourcesPanel";
import { SyslogPanel } from "./pages/admin/SyslogPanel";
import { UsersLayout } from "./pages/admin/UsersLayout";
import { UsersPanel } from "./pages/admin/UsersPanel";
import { AlertsHistoryPanel, AlertsInboxPanel, AlertsPage } from "./pages/AlertsPage";
import { DashboardsPage } from "./pages/DashboardsPage";
import { DashboardViewPage } from "./pages/DashboardViewPage";
import { LoginPage } from "./pages/LoginPage";
import { SetupPage } from "./pages/SetupPage";
import { SavedSearchesPanel } from "./pages/SavedSearchesPage";
import { MonitorsPanel } from "./components/MonitorsPanel";
import { SearchPage } from "./pages/SearchPage";
import { useAgentPanel } from "./state/useAgentPanel";
import { useUnackedAlertCount } from "./state/useAlerts";
import { useAuth } from "./state/useAuth";
import { useTheme } from "./state/theme";
import { primeAlertSound } from "./lib/alertSound";
import {
    loadLastSearch,
    readUrl,
    saveLastSearch,
    searchHref,
    SEARCH_PATH,
    type SearchInput,
} from "./state/url";

export default function App() {
    const { user } = useAuth();

    // When logged out, find out whether this is a fresh instance (no users yet) so
    // we show the first-run setup screen instead of an un-claimable login form.
    // `undefined` = probe in flight; only matters while `user === null`.
    const [needsSetup, setNeedsSetup] = useState<boolean | undefined>(undefined);
    useEffect(() => {
        if (user !== null) return;
        let cancelled = false;
        getSetupStatus()
            .then((s) => {
                if (!cancelled) setNeedsSetup(s.needs_setup);
            })
            .catch(() => {
                if (!cancelled) setNeedsSetup(false);
            });
        return () => {
            cancelled = true;
        };
    }, [user]);

    // Three auth states: `undefined` = boot probe in flight; `null` = no
    // session, show login (or setup); otherwise the main app.
    if (user === undefined) {
        return (
            <div className="h-screen flex items-center justify-center bg-stone-50 dark:bg-stone-950 text-stone-500 dark:text-stone-400">
                Loading…
            </div>
        );
    }
    if (user === null) {
        if (needsSetup === undefined) {
            return (
                <div className="h-screen flex items-center justify-center bg-stone-50 dark:bg-stone-950 text-stone-500 dark:text-stone-400">
                    Loading…
                </div>
            );
        }
        return needsSetup ? <SetupPage /> : <LoginPage />;
    }
    return <MainApp />;
}

function MainApp() {
    const { theme, toggleTheme } = useTheme();
    const panel = useAgentPanel();
    const navigate = useNavigate();
    const { user, logout } = useAuth();

    // In SSO-only mode, hide logout for non-admins (it just bounces them back to
    // the IdP); admins keep it since their password break-glass makes it meaningful.
    const [ssoOnly, setSsoOnly] = useState(false);
    useEffect(() => {
        getSamlStatus()
            .then((s) => setSsoOnly(s.enabled && s.local_login_disabled))
            .catch(() => {});
    }, []);

    // Unlock the alert chime on the user's first interaction (autoplay policy).
    useEffect(() => primeAlertSound(), []);

    // Remember the active search so the top-nav "Search" item restores it
    // instead of resetting to defaults when navigating back from another tab.
    const location = useLocation();
    useEffect(() => {
        if (location.pathname === SEARCH_PATH) saveLastSearch(readUrl());
    }, [location.pathname, location.search]);

    // Load a saved search: navigate to /search with params baked into the URL,
    // which SearchPage seeds its input from on mount.
    const handleLoadFromSaved = (s: SearchInput) => {
        navigate(searchHref({ ...s, page: 1 }));
    };

    return (
        <div className="h-screen flex flex-col overflow-hidden">
            <TopNav
                theme={theme}
                onToggleTheme={toggleTheme}
                userName={user?.display_name ?? user?.userid ?? ""}
                isAdmin={!!user?.is_admin}
                showLogout={!!user?.is_admin || !ssoOnly}
                onLogout={() => void logout()}
            />

            {/* Content shifts left to make room for the always-present Investigate
          panel. paddingRight tracks the panel's live width. */}
            <div
                className={`flex-grow overflow-auto ${
                    panel.resizing ? "" : "transition-[padding] duration-200"
                }`}
                style={{ paddingRight: panel.effectiveWidth }}
            >
                <Routes>
                    <Route path="/" element={<Navigate to="/search" replace />} />
                    <Route path="/search" element={<SearchPage />} />
                    <Route path="/dashboards" element={<DashboardsPage />} />
                    <Route path="/dashboards/:id" element={<DashboardViewPage />} />
                    <Route
                        path="/saved"
                        element={
                            <SavedSearchesPanel current={readUrl()} onLoad={handleLoadFromSaved} />
                        }
                    />
                    {/* Old sub-routes kept as redirects: monitors moved under Alerts,
              and Saved is now a single page. */}
                    <Route path="/saved/searches" element={<Navigate to="/saved" replace />} />
                    <Route
                        path="/saved/monitors"
                        element={<Navigate to="/alerts/monitors" replace />}
                    />
                    <Route path="/alerts" element={<AlertsPage />}>
                        <Route index element={<Navigate to="inbox" replace />} />
                        <Route path="inbox" element={<AlertsInboxPanel />} />
                        <Route path="history" element={<AlertsHistoryPanel />} />
                        <Route path="monitors" element={<MonitorsPanel />} />
                    </Route>
                    <Route path="/admin" element={<AdminLayout />}>
                        <Route index element={<Navigate to="general" replace />} />
                        <Route path="general" element={<GeneralPanel />} />
                        {/* Users & SSO — accounts and SAML config under one tabbed section. */}
                        <Route path="users" element={<UsersLayout />}>
                            <Route index element={<Navigate to="accounts" replace />} />
                            <Route path="accounts" element={<UsersPanel />} />
                            <Route path="saml" element={<SamlPanel />} />
                        </Route>
                        {/* Back-compat redirect from the old standalone SAML route. */}
                        <Route path="saml" element={<Navigate to="/admin/users/saml" replace />} />
                        {/* Data ingestion — Sources / Syslog / Ingest tokens under one tabbed section. */}
                        <Route path="ingestion" element={<IngestionLayout />}>
                            <Route index element={<Navigate to="sources" replace />} />
                            <Route path="sources" element={<SourcesPanel />} />
                            <Route path="syslog" element={<SyslogPanel />} />
                            <Route path="tokens" element={<PushTokensPanel />} />
                        </Route>
                        {/* Back-compat redirects from the old standalone routes. */}
                        <Route
                            path="sources"
                            element={<Navigate to="/admin/ingestion/sources" replace />}
                        />
                        <Route
                            path="syslog"
                            element={<Navigate to="/admin/ingestion/syslog" replace />}
                        />
                        <Route
                            path="ingest-tokens"
                            element={<Navigate to="/admin/ingestion/tokens" replace />}
                        />
                        <Route path="api-keys" element={<ApiKeysPanel />} />
                        <Route path="environments" element={<EnvironmentsPanel />} />
                        <Route path="indexes" element={<IndexesPanel />} />
                        <Route path="mcp" element={<McpPanel />} />
                        <Route path="integrations" element={<IntegrationsPanel />} />
                        <Route path="agent" element={<AgentPanel />} />
                    </Route>
                    <Route path="/account" element={<AccountPage />} />
                    <Route path="*" element={<Navigate to="/search" replace />} />
                </Routes>
            </div>

            <AgentDrawer panel={panel} />
            <AlertToasts />
        </div>
    );
}

function TopNav({
    theme,
    onToggleTheme,
    userName,
    isAdmin,
    showLogout,
    onLogout,
}: {
    theme: "light" | "dark";
    onToggleTheme: () => void;
    userName: string;
    isAdmin: boolean;
    showLogout: boolean;
    onLogout: () => void;
}) {
    const navigate = useNavigate();
    // Restore the last search criteria (in the current env) instead of resetting.
    const restoreSearch = (e: React.MouseEvent<HTMLAnchorElement>) => {
        const last = loadLastSearch();
        if (last) {
            e.preventDefault();
            navigate(searchHref(last));
        }
    };
    return (
        <nav className="h-12 bg-stone-900 text-stone-300 flex items-center px-3 gap-1 flex-shrink-0">
            <Link
                to="/search"
                className="flex items-center gap-2 mr-4 pr-3 border-r border-stone-700 py-1"
                title="HeliosLogs"
            >
                <div className="w-7 h-7 rounded-lg bg-gradient-to-br from-orange-500 to-orange-700 flex items-center justify-center text-white">
                    <Layers className="w-4 h-4" />
                </div>
                <span className="font-semibold text-white">HeliosLogs</span>
            </Link>
            <TopNavLink
                to="/search"
                icon={<SearchIcon className="w-4 h-4" />}
                onClick={restoreSearch}
            >
                Search
            </TopNavLink>
            <TopNavLink to="/dashboards" icon={<LayoutDashboard className="w-4 h-4" />}>
                Dashboards
            </TopNavLink>
            <TopNavLink to="/saved" icon={<Star className="w-4 h-4" />}>
                Saved
            </TopNavLink>
            <AlertsNavLink />
            {isAdmin && (
                <TopNavLink to="/admin" icon={<Settings className="w-4 h-4" />}>
                    Admin
                </TopNavLink>
            )}

            <div className="flex-grow" />

            <EnvPicker />

            <Link
                to="/account"
                className="px-2 py-1 text-stone-300 hover:text-white hover:bg-stone-800 rounded-md transition mr-1"
                title="Account"
            >
                {userName || "account"}
            </Link>
            {showLogout && (
                <button
                    type="button"
                    onClick={onLogout}
                    className="p-2 hover:bg-stone-800 hover:text-white rounded-md transition"
                    title="Sign out"
                    aria-label="sign out"
                >
                    <LogOut className="w-4 h-4" />
                </button>
            )}

            <button
                type="button"
                onClick={onToggleTheme}
                className="p-2 hover:bg-stone-800 hover:text-white rounded-md transition"
                title={theme === "dark" ? "Switch to light mode" : "Switch to dark mode"}
                aria-label="toggle theme"
            >
                {theme === "dark" ? <Sun className="w-4 h-4" /> : <Moon className="w-4 h-4" />}
            </button>
        </nav>
    );
}

// Alerts nav link with unacked-count badge. Polls on a slow timer + listens for
// `helios-alerts-changed` so the badge zeroes immediately on acknowledge.
function AlertsNavLink() {
    const count = useUnackedAlertCount();
    return (
        <NavLink
            to="/alerts"
            className={({ isActive }) =>
                `px-3 py-1.5 rounded-md flex items-center gap-1.5 transition ${
                    isActive ? "bg-stone-800 text-white" : "hover:bg-stone-800 hover:text-white"
                }`
            }
        >
            <Bell className="w-4 h-4" />
            Alerts
            {count > 0 && (
                <span className="ml-0.5 inline-flex items-center justify-center min-w-[1.25rem] h-5 px-1.5 rounded-full bg-orange-500 text-white text-xs font-semibold">
                    {count > 99 ? "99+" : count}
                </span>
            )}
        </NavLink>
    );
}

function TopNavLink({
    to,
    icon,
    children,
    onClick,
}: {
    to: string;
    icon: React.ReactNode;
    children: React.ReactNode;
    onClick?: React.MouseEventHandler<HTMLAnchorElement>;
}) {
    const base = "px-3 py-1.5 rounded-md flex items-center gap-1.5 transition";
    return (
        <NavLink
            to={to}
            onClick={onClick}
            className={({ isActive }) =>
                isActive
                    ? `${base} bg-stone-800 text-white`
                    : `${base} hover:bg-stone-800 hover:text-white`
            }
        >
            {icon}
            {children}
        </NavLink>
    );
}
