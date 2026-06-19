// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";
import { Highlight } from "../../src/components/Highlight";

describe("<Highlight>", () => {
    it("wraps matching terms in <mark> (case-insensitive)", () => {
        const { container } = render(<Highlight text="Hello World" terms={["world"]} />);
        const marks = container.querySelectorAll("mark");
        expect(marks).toHaveLength(1);
        expect(marks[0].textContent).toBe("World");
        expect(container.textContent).toBe("Hello World");
    });

    it("merges overlapping ranges into a single mark", () => {
        const { container } = render(<Highlight text="abc" terms={["ab", "bc"]} />);
        const marks = container.querySelectorAll("mark");
        expect(marks).toHaveLength(1);
        expect(marks[0].textContent).toBe("abc");
    });

    it("renders plain text when there are no terms or no match", () => {
        const { container: a } = render(<Highlight text="plain" terms={[]} />);
        expect(a.querySelector("mark")).toBeNull();
        const { container: b } = render(<Highlight text="plain" terms={["zzz"]} />);
        expect(b.querySelector("mark")).toBeNull();
    });
});
