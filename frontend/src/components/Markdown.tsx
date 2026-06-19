// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useMemo } from "react";
import { useNavigate } from "react-router-dom";
import DOMPurify from "dompurify";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

// In-app routes, recognised even when the agent emits a wrong-host absolute URL.
const HELIOS_ROUTES = ["/search", "/saved", "/admin"];

function isHeliosRoute(pathname: string): boolean {
    return HELIOS_ROUTES.some((r) => pathname === r || pathname.startsWith(`${r}/`));
}

// Returns the in-app path if `href` is client-routable (same-origin, or any
// known helios route regardless of host), else null for external links.
function internalPath(href: string): string | null {
    try {
        const url = new URL(href, window.location.origin);
        const path = `${url.pathname}${url.search}${url.hash}`;
        if (url.origin === window.location.origin) return path;
        if (isHeliosRoute(url.pathname)) return path;
        return null;
    } catch {
        return null;
    }
}

// Decode the handful of HTML entities an agent might slip into an href or
// label. `&amp;` is decoded last so "&amp;gt;" yields the literal "&gt;".
function decodeEntities(s: string): string {
    return s
        .replace(/&lt;/gi, "<")
        .replace(/&gt;/gi, ">")
        .replace(/&quot;/gi, '"')
        .replace(/&#0*39;|&apos;/gi, "'")
        .replace(/&amp;/gi, "&");
}

// Agent search links use HTML anchors (quoted hrefs hold spaces/pipes/parens a
// markdown destination can't); rewrite them to markdown links since react-markdown skips raw HTML.
const HTML_ANCHOR_RE = /<a\b[^>]*?\bhref\s*=\s*["']([^"']*)["'][^>]*>(.*?)<\/a>/gis;

function htmlAnchorsToLinks(src: string): string {
    return src.replace(HTML_ANCHOR_RE, (_m, rawHref: string, rawLabel: string) => {
        const dest = decodeEntities(rawHref)
            .trim()
            .replace(/ /g, "%20")
            .replace(/</g, "%3C")
            .replace(/>/g, "%3E");
        const label = decodeEntities(rawLabel)
            .replace(/\s+/g, " ")
            .replace(/[[\]]/g, "\\$&")
            .trim();
        return `[${label || dest}](${dest})`;
    });
}

// DOMPurify the agent's embedded SVG charts: SVG profile only, strips scripts/handlers/external hrefs.
function sanitizeSvg(raw: string): string {
    return DOMPurify.sanitize(raw, {
        USE_PROFILES: { svg: true, svgFilters: true },
        // Defense in depth: explicit URI allowlist (no javascript:), beyond the SVG profile.
        ALLOWED_URI_REGEXP: /^(?:(?:https?|mailto):|[^a-z]|[a-z+.-]+(?:[^a-z+.\-:]|$))/i,
    });
}

// Tailwind component overrides for assistant chat markdown (react-markdown + remark-gfm).

function buildComponents(): Components {
    return {
        p: ({ children }) => (
            <p className="my-1.5 first:mt-0 last:mb-0 leading-relaxed">{children}</p>
        ),
        h1: ({ children }) => (
            <h1 className="mt-3 mb-1.5 first:mt-0 font-semibold text-stone-900 dark:text-stone-100">
                {children}
            </h1>
        ),
        h2: ({ children }) => (
            <h2 className="mt-3 mb-1 first:mt-0 font-semibold text-stone-900 dark:text-stone-100">
                {children}
            </h2>
        ),
        h3: ({ children }) => (
            <h3 className="mt-2.5 mb-1 first:mt-0 font-semibold text-stone-700 dark:text-stone-200">
                {children}
            </h3>
        ),
        ul: ({ children }) => <ul className="my-1.5 ml-4 list-disc space-y-0.5">{children}</ul>,
        ol: ({ children }) => <ol className="my-1.5 ml-4 list-decimal space-y-0.5">{children}</ol>,
        li: ({ children }) => <li className="leading-snug">{children}</li>,
        strong: ({ children }) => (
            <strong className="font-semibold text-stone-900 dark:text-stone-100">{children}</strong>
        ),
        em: ({ children }) => <em className="italic">{children}</em>,
        a: ({ children, href }) => (
            // Plain anchor; the wrapper's delegated handler routes in-app links client-side.
            <a
                href={href}
                className="text-orange-700 dark:text-orange-300 underline decoration-orange-300/40 hover:decoration-orange-500"
            >
                {children}
            </a>
        ),
        blockquote: ({ children }) => (
            <blockquote className="my-2 pl-3 border-l-2 border-stone-200 dark:border-stone-700 text-stone-600 dark:text-stone-400">
                {children}
            </blockquote>
        ),
        code: ({ children, className }) => {
            // Inline `code` has no className; fenced blocks get language-X, styled by `pre`.
            if (className) {
                // Inside a <pre> — let `pre` handle styling; just emit plain monospace text.
                return <code className={className}>{children}</code>;
            }
            return (
                <code className="px-1 py-0.5 rounded bg-stone-100 dark:bg-stone-800 font-mono text-stone-800 dark:text-stone-200 break-all">
                    {children}
                </code>
            );
        },
        pre: ({ children }) => {
            // Render language-svg fences as inline sanitized SVG so the agent can embed charts.
            const child = Array.isArray(children) ? children[0] : children;
            if (
                child &&
                typeof child === "object" &&
                "props" in child &&
                child.props?.className === "language-svg"
            ) {
                const raw = String(child.props.children ?? "");
                const clean = sanitizeSvg(raw);
                return (
                    <div
                        className="my-2 p-2 rounded-md bg-white dark:bg-stone-950 border border-stone-200 dark:border-stone-800 text-stone-700 dark:text-stone-200 overflow-x-auto [&_svg]:max-w-full [&_svg]:h-auto"
                        // clean is DOMPurify-sanitized SVG (see sanitizeSvg)
                        dangerouslySetInnerHTML={{ __html: clean }}
                    />
                );
            }
            return (
                <pre className="my-2 p-2.5 rounded-md bg-stone-100 dark:bg-stone-950 border border-stone-200 dark:border-stone-800 font-mono leading-snug text-stone-800 dark:text-stone-200 overflow-x-auto whitespace-pre">
                    {children}
                </pre>
            );
        },
        table: ({ children }) => (
            <div className="my-2 overflow-x-auto">
                <table className="border-collapse">{children}</table>
            </div>
        ),
        thead: ({ children }) => (
            <thead className="bg-stone-100 dark:bg-stone-800/60">{children}</thead>
        ),
        th: ({ children }) => (
            <th className="px-2 py-1 text-left font-semibold border border-stone-200 dark:border-stone-700">
                {children}
            </th>
        ),
        td: ({ children }) => (
            <td className="px-2 py-1 border border-stone-200 dark:border-stone-800 align-top">
                {children}
            </td>
        ),
        hr: () => <hr className="my-3 border-stone-200 dark:border-stone-700" />,
    };
}

// Component overrides have no per-render dependencies — build once.
const MARKDOWN_COMPONENTS = buildComponents();

interface Props {
    children: string;
}

export function Markdown({ children }: Props) {
    const navigate = useNavigate();
    const source = useMemo(() => htmlAnchorsToLinks(children), [children]);

    // Delegated handler: in-app links route client-side; modified clicks and
    // external links fall through to default browser behaviour.
    const handleClick = (e: React.MouseEvent<HTMLDivElement>) => {
        if (
            e.defaultPrevented ||
            e.button !== 0 ||
            e.metaKey ||
            e.ctrlKey ||
            e.shiftKey ||
            e.altKey
        ) {
            return;
        }
        const anchor = (e.target as HTMLElement).closest?.("a");
        const href = anchor?.getAttribute("href");
        if (!href) return;
        const internal = internalPath(href);
        if (internal) {
            e.preventDefault();
            navigate(internal);
        }
    };

    return (
        <div className="text-stone-800 dark:text-stone-200 break-words" onClick={handleClick}>
            <ReactMarkdown remarkPlugins={[remarkGfm]} components={MARKDOWN_COMPONENTS}>
                {source}
            </ReactMarkdown>
        </div>
    );
}
