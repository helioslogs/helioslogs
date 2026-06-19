// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Pagination } from "../../src/components/Pagination";

describe("<Pagination>", () => {
    it("renders nothing for a single page", () => {
        const { container } = render(<Pagination page={1} totalPages={1} onChange={() => {}} />);
        expect(container).toBeEmptyDOMElement();
    });

    it("disables prev on the first page and next on the last", () => {
        const { rerender } = render(<Pagination page={1} totalPages={5} onChange={() => {}} />);
        expect(screen.getByLabelText("previous page")).toBeDisabled();
        expect(screen.getByLabelText("next page")).toBeEnabled();
        rerender(<Pagination page={5} totalPages={5} onChange={() => {}} />);
        expect(screen.getByLabelText("next page")).toBeDisabled();
    });

    it("fires onChange with the chosen page", async () => {
        const onChange = vi.fn();
        render(<Pagination page={1} totalPages={5} onChange={onChange} />);
        await userEvent.click(screen.getByLabelText("page 3"));
        expect(onChange).toHaveBeenCalledWith(3);
    });

    it("marks the current page and does not fire onChange for it", async () => {
        const onChange = vi.fn();
        render(<Pagination page={2} totalPages={5} onChange={onChange} />);
        const current = screen.getByLabelText("page 2");
        expect(current).toHaveAttribute("aria-current", "page");
        await userEvent.click(current);
        expect(onChange).not.toHaveBeenCalled();
    });
});
