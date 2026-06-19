// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { EnvBadge } from "../../src/components/EnvBadge";
import { VisibilityBadge } from "../../src/components/VisibilityBadge";

describe("<EnvBadge>", () => {
    it("shows the env name with a tooltip", () => {
        render(<EnvBadge env="prod" />);
        const el = screen.getByText("prod");
        expect(el).toBeInTheDocument();
        expect(el.closest("span")).toHaveAttribute("title", "Environment: prod");
    });

    it("renders nothing when env is undefined", () => {
        const { container } = render(<EnvBadge env={undefined} />);
        expect(container).toBeEmptyDOMElement();
    });
});

describe("<VisibilityBadge>", () => {
    it("renders Public / Private per the flag", () => {
        const { rerender } = render(<VisibilityBadge isPublic={true} />);
        expect(screen.getByText("Public")).toBeInTheDocument();
        rerender(<VisibilityBadge isPublic={false} />);
        expect(screen.getByText("Private")).toBeInTheDocument();
    });
});
