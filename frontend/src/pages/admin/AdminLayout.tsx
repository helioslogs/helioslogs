// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Admin shell: sticky left side-nav rail + full-width panel content.
// Admin-only — non-admin users are bounced back to /search.

import {
    Bot,
    Boxes,
    Database,
    Import,
    KeySquare,
    Settings as SettingsIcon,
    Sparkles,
    Users as UsersIcon,
    Webhook,
} from "lucide-react";
import { Navigate, NavLink, Outlet } from "react-router-dom";
import { useAuth } from "../../state/useAuth";

export function AdminLayout() {
    const { user } = useAuth();
    if (user && !user.is_admin) {
        return <Navigate to="/search" replace />;
    }

    return (
        <div className="flex">
            <aside className="w-60 flex-shrink-0 border-r border-stone-200 dark:border-stone-800 bg-white dark:bg-stone-900 sticky top-0 self-start">
                <div className="px-4 pt-4 pb-2 font-semibold uppercase tracking-wider text-stone-700 dark:text-stone-300">
                    Admin
                </div>
                <nav className="px-2 pb-3 space-y-0.5">
                    <SideNavLink to="general" icon={<SettingsIcon className="w-4 h-4" />}>
                        General settings
                    </SideNavLink>
                    <SideNavLink to="users" icon={<UsersIcon className="w-4 h-4" />}>
                        Users & SSO
                    </SideNavLink>
                    <SideNavLink to="ingestion" icon={<Import className="w-4 h-4" />}>
                        Data Ingestion
                    </SideNavLink>
                    <SideNavLink to="api-keys" icon={<KeySquare className="w-4 h-4" />}>
                        API keys
                    </SideNavLink>
                    <SideNavLink to="environments" icon={<Boxes className="w-4 h-4" />}>
                        Environments
                    </SideNavLink>
                    <SideNavLink to="agent" icon={<Sparkles className="w-4 h-4" />}>
                        LLM Provider
                    </SideNavLink>
                    <SideNavLink to="mcp" icon={<Bot className="w-4 h-4" />}>
                        MCP server
                    </SideNavLink>
                    <SideNavLink to="integrations" icon={<Webhook className="w-4 h-4" />}>
                        Integrations
                    </SideNavLink>
                    <SideNavLink to="indexes" icon={<Database className="w-4 h-4" />}>
                        Index management
                    </SideNavLink>
                </nav>
            </aside>
            <main className="flex-1 min-w-0 bg-white dark:bg-stone-900">
                <Outlet />
            </main>
        </div>
    );
}

function SideNavLink({
    to,
    icon,
    children,
}: {
    to: string;
    icon: React.ReactNode;
    children: React.ReactNode;
}) {
    const base = "flex items-center gap-2 px-3 py-2 rounded-md transition";
    return (
        <NavLink
            to={to}
            end={false}
            className={({ isActive }) =>
                isActive
                    ? `${base} bg-orange-50 text-orange-900 dark:bg-orange-950/40 dark:text-orange-100 font-medium`
                    : `${base} text-stone-700 dark:text-stone-300 hover:bg-stone-100 dark:hover:bg-stone-800`
            }
        >
            {icon}
            {children}
        </NavLink>
    );
}
