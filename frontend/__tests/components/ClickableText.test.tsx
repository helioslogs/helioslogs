// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ClickableText } from "../../src/components/ClickableText";

describe("<ClickableText>", () => {
    it("renders words and reports the clicked word", async () => {
        const onPick = vi.fn();
        render(<ClickableText text="auth-svc failed" terms={[]} query="*" onPickTerm={onPick} />);
        await userEvent.click(screen.getByText("auth-svc"));
        expect(onPick).toHaveBeenCalledWith("auth-svc");
    });

    it("marks a word that's already an active query term", () => {
        render(
            <ClickableText
                text="timeout here"
                terms={[]}
                query='"timeout"'
                onPickTerm={() => {}}
            />,
        );
        const word = screen.getByText("timeout");
        expect(word.getAttribute("title")).toMatch(/remove from search/);
    });

    it("preserves surrounding punctuation as plain text", () => {
        const { container } = render(
            <ClickableText text="a, b" terms={[]} query="*" onPickTerm={() => {}} />,
        );
        expect(container.textContent).toBe("a, b");
    });
});
