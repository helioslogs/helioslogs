// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect } from "vitest";
import { analyzeContext } from "../../src/lib/suggestContext";

// Helper: analyze with the cursor at end of `text` unless a caret marker `|`
// position is given via an explicit cursor.
const at = (text: string, cursor = text.length) => analyzeContext(text, cursor);

describe("analyzeContext — main segment", () => {
    it("treats a bare word as a field prefix", () => {
        const c = at("lev");
        expect(c.kind).toBe("field");
        expect(c.prefix).toBe("lev");
        expect(c.start).toBe(0);
        expect(c.end).toBe(3);
        expect(c.atTermBoundary).toBe(false);
    });

    it("flags a term boundary after a completed clause", () => {
        const c = at("level:error ");
        expect(c.kind).toBe("field");
        expect(c.atTermBoundary).toBe(true);
    });

    it("does not flag a boundary right after AND/OR/NOT", () => {
        const c = at("level:error AND ");
        expect(c.atTermBoundary).toBe(false);
    });

    it("classifies text after `field:` as a value", () => {
        const c = at("level:er");
        expect(c.kind).toBe("value");
        expect(c.field).toBe("level");
        expect(c.prefix).toBe("er");
        expect(c.start).toBe(6);
        expect(c.end).toBe(8);
    });

    it("strips a leading comparison operator from the value prefix", () => {
        const c = at("lat:>=10");
        expect(c.kind).toBe("value");
        expect(c.field).toBe("lat");
        expect(c.prefix).toBe("10");
        expect(c.start).toBe(6); // after `>=`
    });
});

describe("analyzeContext — quotes", () => {
    it("treats an open quoted value as a value with quoted=true", () => {
        const c = at('msg:"foo ba');
        expect(c.kind).toBe("value");
        expect(c.field).toBe("msg");
        expect(c.prefix).toBe("foo ba");
        expect(c.quoted).toBe(true);
        expect(c.start).toBe(5);
    });

    it("suggests nothing inside a bare phrase quote", () => {
        const c = at('"foo');
        expect(c.kind).toBe("none");
    });
});

describe("analyzeContext — pipe segments", () => {
    it("offers commands as the first token after a pipe", () => {
        const c = at("* | sta");
        expect(c.kind).toBe("command");
        expect(c.prefix).toBe("sta");
    });

    it("offers aggs inside stats", () => {
        const c = at("* | stats co");
        expect(c.kind).toBe("agg");
        expect(c.prefix).toBe("co");
    });

    it("switches to group-by fields after `by`", () => {
        const c = at("* | stats count by ho");
        expect(c.kind).toBe("stats-field");
        expect(c.prefix).toBe("ho");
    });

    it("offers a field arg for top/rare", () => {
        expect(at("* | top sta").kind).toBe("arg-field");
        expect(at("* | rare pa").kind).toBe("arg-field");
    });

    it("strips a leading - / + from a sort field arg", () => {
        const c = at("* | sort -lat");
        expect(c.kind).toBe("arg-field");
        expect(c.prefix).toBe("lat");
        expect(c.start).toBe(10); // after the `-`
    });

    it("suggests nothing for head/tail (numeric args)", () => {
        expect(at("* | head 1").kind).toBe("none");
    });

    it("ignores a pipe inside a quoted phrase", () => {
        // The `|` is inside quotes, so we're still in the main segment value.
        const c = at('msg:"a | b');
        expect(c.kind).toBe("value");
        expect(c.field).toBe("msg");
    });
});
