// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Admin → Ingestion: one home for the ways logs get into HeliosLogs. A header + tab
// bar over the Sources / Syslog / Ingest-tokens panels (rendered via nested routes).

import { FolderInput, KeyRound, Network } from "lucide-react";
import { NavLink, Outlet } from "react-router-dom";

export function IngestionLayout() {
    return (
        <div>
            <div className="px-6 pt-5">
                <h1 className="text-lg font-semibold text-stone-900 dark:text-stone-100">
                    Data Ingestion
                </h1>
                <p className="mt-0.5 text-sm text-stone-500 dark:text-stone-400">
                    How logs get into HeliosLogs — pull from files or S3, receive syslog over the
                    network, or accept pushes authenticated with scoped tokens.
                </p>
                <nav className="mt-4 -mb-px flex gap-1">
                    <Tab to="sources" icon={<FolderInput className="w-4 h-4" />}>
                        Sources
                    </Tab>
                    <Tab to="syslog" icon={<Network className="w-4 h-4" />}>
                        Syslog
                    </Tab>
                    <Tab to="tokens" icon={<KeyRound className="w-4 h-4" />}>
                        Ingest tokens
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
