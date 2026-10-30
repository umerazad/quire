import { expect, it } from "vitest";
import cases from "../fixtures/merges.json";
import { diffRegions, mergeText } from "./core/merge";

it.each(cases)("$name", ({ base, first, second, text, overlaps }) => {
  expect(mergeText(base, first, second)).toEqual({ text, overlaps });
});

it("finds the shortest edit between two line lists", () => {
  expect(diffRegions(["a", "b", "c"], ["a", "x", "c"])).toEqual([
    { aStart: 1, aEnd: 2, bStart: 1, bEnd: 2 },
  ]);
  expect(diffRegions(["a", "b"], ["a", "b"])).toEqual([]);
  expect(diffRegions([], ["a"])).toEqual([
    { aStart: 0, aEnd: 0, bStart: 0, bEnd: 1 },
  ]);
});

it("keeps every line either side added when nothing overlaps", () => {
  const base = Array.from({ length: 200 }, (_, i) => `- line ${i}`);
  const first = [...base],
    second = [...base];
  first.splice(150, 0, "- added near the end");
  second.splice(10, 1, "- line 10 reworded");
  const merged = mergeText(
    base.join("\n"),
    first.join("\n"),
    second.join("\n"),
  );
  expect(merged.overlaps).toBe(0);
  expect(merged.text.split("\n")).toContain("- added near the end");
  expect(merged.text.split("\n")).toContain("- line 10 reworded");
  expect(merged.text.split("\n")).toHaveLength(201);
});
