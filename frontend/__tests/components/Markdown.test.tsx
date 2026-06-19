// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { Markdown } from "../../src/components/Markdown";

const renderMd = (src: string) =>
    render(
        <MemoryRouter>
            <Markdown>{src}</Markdown>
        </MemoryRouter>,
    );

describe("<Markdown>", () => {
    it("renders basic markdown formatting", () => {
        const { container } = renderMd("**bold** text");
        expect(container.querySelector("strong")?.textContent).toBe("bold");
    });

    it("renders links", () => {
        renderMd("see [results](/search?q=*)");
        const a = screen.getByRole("link", { name: "results" });
        expect(a).toHaveAttribute("href", "/search?q=*");
    });

    it("does not render raw HTML script tags (no XSS)", () => {
        const { container } = renderMd("<script>alert(1)</script> safe");
        expect(container.querySelector("script")).toBeNull();
        expect(container.textContent).toContain("safe");
    });
});
