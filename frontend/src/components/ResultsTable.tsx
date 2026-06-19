// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { Loader2 } from "lucide-react";
import type { TableResult } from "../api/types";

interface Props {
    table: TableResult;
    loading?: boolean;
}

function fmt(v: string | number | null): string {
    if (v === null || v === undefined) return "—";
    if (typeof v === "number") {
        if (Number.isInteger(v)) return v.toLocaleString();
        return v.toFixed(2);
    }
    return v;
}

function isNumericValue(v: string | number | null): boolean {
    return typeof v === "number";
}

export function ResultsTable({ table, loading }: Props) {
    const { columns, rows, search, stages, took_us, scanned_docs, partitions_scanned } = table;
    // Decide alignment per column based on the first non-null sample.
    const colAlign: ("right" | "left")[] = columns.map((_, ci) => {
        for (const row of rows) {
            if (row[ci] !== null && row[ci] !== undefined) {
                return isNumericValue(row[ci]) ? "right" : "left";
            }
        }
        return "left";
    });

    return (
        <div className="rounded-xl border border-stone-200 dark:border-stone-800 bg-white dark:bg-stone-900">
            <div className="flex items-center justify-between gap-3 px-4 py-2 border-b border-stone-200 dark:border-stone-800 text-stone-500 dark:text-stone-400 flex-wrap">
                <span className="inline-flex items-center gap-1.5">
                    {loading && <Loader2 className="w-3.5 h-3.5 animate-spin" aria-hidden="true" />}
                    {loading ? (
                        <span>{rows.length > 0 ? "refreshing…" : "executing…"}</span>
                    ) : (
                        <>
                            <span className="font-semibold text-stone-700 dark:text-stone-200">
                                {rows.length.toLocaleString()}
                            </span>{" "}
                            row{rows.length === 1 ? "" : "s"} from{" "}
                            <span className="font-semibold text-stone-700 dark:text-stone-200">
                                {scanned_docs.toLocaleString()}
                            </span>{" "}
                            events across {partitions_scanned} partition
                            {partitions_scanned === 1 ? "" : "s"} in{" "}
                            {took_us < 1000 ? `${took_us}µs` : `${(took_us / 1000).toFixed(2)}ms`}
                        </>
                    )}
                </span>
                <span className="flex items-center gap-1.5 flex-wrap font-mono">
                    <code className="px-1.5 py-0.5 rounded bg-stone-100 dark:bg-stone-800 text-stone-700 dark:text-stone-300">
                        {search || "*"}
                    </code>
                    {stages.map((s, i) => (
                        <span key={i} className="flex items-center gap-1.5">
                            <span className="text-stone-300 dark:text-stone-600">|</span>
                            <code className="px-1.5 py-0.5 rounded bg-orange-50 text-orange-800 dark:bg-orange-950/40 dark:text-orange-200">
                                {s}
                            </code>
                        </span>
                    ))}
                </span>
            </div>
            <div className="overflow-auto">
                <table className="w-full">
                    <thead className="bg-stone-50 dark:bg-stone-900/50 sticky top-0">
                        <tr>
                            {columns.map((c, i) => (
                                <th
                                    key={i}
                                    className={`px-3 py-2 font-semibold uppercase tracking-wider text-stone-500 dark:text-stone-400 border-b border-stone-200 dark:border-stone-800 ${
                                        colAlign[i] === "right" ? "text-right" : "text-left"
                                    }`}
                                >
                                    {c}
                                </th>
                            ))}
                        </tr>
                    </thead>
                    <tbody className="divide-y divide-stone-100 dark:divide-stone-800">
                        {rows.map((row, ri) => (
                            <tr key={ri} className="hover:bg-stone-50 dark:hover:bg-stone-800/40">
                                {row.map((v, ci) => (
                                    <td
                                        key={ci}
                                        className={`px-3 py-1.5 ${
                                            colAlign[ci] === "right" ? "text-right" : "text-left"
                                        } ${isNumericValue(v) ? "font-mono tabular-nums" : ""} text-stone-700 dark:text-stone-300`}
                                    >
                                        {fmt(v)}
                                    </td>
                                ))}
                            </tr>
                        ))}
                        {rows.length === 0 && loading && (
                            <tr>
                                <td
                                    colSpan={columns.length || 1}
                                    className="px-3 py-10 text-stone-500 dark:text-stone-400"
                                >
                                    <span className="inline-flex w-full items-center justify-center gap-2">
                                        <Loader2
                                            className="w-4 h-4 animate-spin"
                                            aria-hidden="true"
                                        />
                                        <span>executing…</span>
                                    </span>
                                </td>
                            </tr>
                        )}
                        {rows.length === 0 && !loading && (
                            <tr>
                                <td
                                    colSpan={columns.length || 1}
                                    className="px-3 py-8 text-center text-stone-400 dark:text-stone-500"
                                >
                                    no results
                                </td>
                            </tr>
                        )}
                    </tbody>
                </table>
            </div>
        </div>
    );
}
