// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useState } from "react";
import { useNavigate } from "react-router-dom";
import type { Widget } from "../../api/types";
import { WidgetFrame } from "./WidgetFrame";
import { TimeseriesWidget } from "./TimeseriesWidget";
import { StatWidget } from "./StatWidget";
import { TopNWidget } from "./TopNWidget";
import {
    ResultsViewControls,
    SearchResultsWidget,
    readResultsView,
    writeResultsView,
    type ResultsView,
} from "./SearchResultsWidget";
import { AlertsWidget } from "./AlertsWidget";
import { MonitorsWidget } from "./MonitorsWidget";
import { SavedSearchesWidget } from "./SavedSearchesWidget";
import { dashSearchHref, overrideRange, type DashRange } from "./util";

interface Props {
    widget: Widget;
    range: DashRange;
    refreshKey: number;
    editing: boolean;
    onEdit: () => void;
    onDelete: () => void;
}

// Renders one widget inside the shared frame, dispatching on `kind`. Query
// widgets report loading/error up so the frame can show a spinner / message.
export function WidgetView({ widget, range, refreshKey, editing, onEdit, onDelete }: Props) {
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);
    // Search-results display mode, owned here so its toggle can live in the
    // widget title bar (rendered via WidgetFrame.headerRight).
    const [resultsView, setResultsView] = useState<ResultsView>(() => readResultsView(widget.id));
    const navigate = useNavigate();

    // Query widgets honour a per-widget time override, falling back to the
    // dashboard range.
    const effRange = overrideRange(widget.time, range);

    const headerRight =
        widget.kind === "search_results" ? (
            <ResultsViewControls
                view={resultsView}
                onChange={(v) => {
                    setResultsView(v);
                    writeResultsView(widget.id, v);
                }}
                onOpen={() => navigate(dashSearchHref(widget.series?.[0]?.query || "*", effRange))}
            />
        ) : undefined;

    const body = (() => {
        switch (widget.kind) {
            case "timeseries":
                return (
                    <TimeseriesWidget
                        widget={widget}
                        range={effRange}
                        refreshKey={refreshKey}
                        onLoadingChange={setLoading}
                        onError={setError}
                    />
                );
            case "stat":
                return (
                    <StatWidget
                        widget={widget}
                        range={effRange}
                        refreshKey={refreshKey}
                        onLoadingChange={setLoading}
                        onError={setError}
                    />
                );
            case "topn":
                return (
                    <TopNWidget
                        widget={widget}
                        range={effRange}
                        refreshKey={refreshKey}
                        onLoadingChange={setLoading}
                        onError={setError}
                    />
                );
            case "search_results":
                return (
                    <SearchResultsWidget
                        widget={widget}
                        range={effRange}
                        refreshKey={refreshKey}
                        view={resultsView}
                        onLoadingChange={setLoading}
                        onError={setError}
                    />
                );
            case "alerts":
                return <AlertsWidget widget={widget} history={false} />;
            case "alerts_history":
                return <AlertsWidget widget={widget} history />;
            case "monitors":
                return <MonitorsWidget widget={widget} />;
            case "saved_searches":
                return <SavedSearchesWidget widget={widget} refreshKey={refreshKey} />;
            default:
                return null;
        }
    })();

    return (
        <WidgetFrame
            title={widget.title || "Untitled"}
            editing={editing}
            onEdit={onEdit}
            onDelete={onDelete}
            loading={loading}
            error={error}
            headerRight={headerRight}
        >
            {body}
        </WidgetFrame>
    );
}
