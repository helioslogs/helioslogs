// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Admin → Users: account management + single sign-on, grouped as tabs (mirrors
// the Ingestion section) so SSO config isn't an orphan route.

import { ShieldCheck, Users as UsersIcon } from "lucide-react";
import { NavLink, Outlet } from "react-router-dom";

export function UsersLayout() {
    return (
        <div>
            <div className="px-6 pt-5">
                <h1 className="text-lg font-semibold text-stone-900 dark:text-stone-100">
                    Users & SSO
                </h1>
                <p className="mt-0.5 text-sm text-stone-500 dark:text-stone-400">
                    Manage accounts and per-user index access, and configure single sign-on with a
                    trusted SAML identity provider.
                </p>
                <nav className="mt-4 -mb-px flex gap-1">
                    <Tab to="accounts" icon={<UsersIcon className="w-4 h-4" />}>
                        Users
                    </Tab>
                    <Tab to="saml" icon={<ShieldCheck className="w-4 h-4" />}>
                        Single sign-on
                    </Tab>
                </nav>
            </div>
            <div className="border-b border-stone-200 dark:border-stone-800" />
            <Outlet />
        </div>
    );
}

function Tab({
    to,
    icon,
    children,
}: {
    to: string;
    icon: React.ReactNode;
    children: React.ReactNode;
}) {
    const base = "flex items-center gap-2 px-3 py-2 text-sm border-b-2 transition";
    return (
        <NavLink
            to={to}
            className={({ isActive }) =>
                isActive
                    ? `${base} border-orange-500 text-orange-700 dark:text-orange-300 font-medium`
                    : `${base} border-transparent text-stone-600 dark:text-stone-400 hover:text-stone-900 dark:hover:text-stone-100`
            }
        >
            {icon}
            {children}
        </NavLink>
    );
}
