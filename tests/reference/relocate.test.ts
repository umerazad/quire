// Checks that the relocation extraction reproduces rt's planner: each expectation below comes
// from rt tests/relocate.test.ts at 5ea5c22310ff, run over an in-memory notebook.
import { expect, it } from "vitest";
import { editorText, pageName, pageTitle } from "./markdown/markdown";
import { planRelocation, type Notebook } from "./core/relocate";

function notebook(pages: Record<string, string>): Notebook {
  const texts = new Map(Object.entries(pages).map(([path, text]) => [path, editorText(text)]));
  const title = (path: string) => pageTitle(path, texts.get(path) ?? "");
  const key = (target: string) => target.split("#")[0].replace(/\.md$/i, "").toLocaleLowerCase();
  return {
    documents: new Map([...texts.keys()].map((path) => [path, { revision: "r", state: { lineBreak: "\n" } }])),
    pagePaths: () => [...texts.keys()],
    title,
    findPage: (target) =>
      [...texts.keys()].find((path) => pageName(path).toLocaleLowerCase() === key(target)) ??
      ((titled) => (titled.length === 1 ? titled[0] : undefined))([...texts.keys()].filter((path) => title(path).toLocaleLowerCase() === key(target))),
    all: () => [...texts].map(([path, text]) => ({ path, text })),
  };
}

it("renames a page's title with it: links follow and no title is kept", () => {
  const model = notebook({
    "Old name.md": "- Body\n",
    "Titled.md": '---\ntags: [x]\ntitle: "Titled"\n---\n\n- title: in the body\n',
    "Links.md": "- [[Old name]] and [[Old name#Part|alias]] and [[Titled]]\n",
  });
  const plan = planRelocation(model, "Old name.md", "New name.md", { rename: true });
  const texts = new Map(plan.edits.map((edit) => [edit.path, edit.text]));
  expect(texts.get("New name.md")).toBe("- Body\n");
  expect(texts.get("Links.md")).toBe("- [[New name]] and [[New name#Part|alias]] and [[Titled]]\n");
  const titled = planRelocation(model, "Titled.md", "Renamed.md", { rename: true });
  expect(titled.edits.find((edit) => edit.path === "Renamed.md")?.text).toBe(
    '---\ntags: [x]\ntitle: "Renamed"\n---\n\n- title: in the body\n',
  );
  const moved = planRelocation(model, "Old name.md", "Archive/Old name 2.md");
  expect(moved.edits.find((edit) => edit.path === "Archive/Old name 2.md")?.text).toBe('---\ntitle: "Old name"\n---\n\n- Body\n');
  expect(moved.edits.some((edit) => edit.path === "Links.md")).toBe(false);
});

it("rejects collisions and traversal", () => {
  const model = notebook({ "Old.md": "Original", "Exists.md": "Existing" });
  expect(() => planRelocation(model, "Old.md", "../Escape")).toThrow();
  expect(() => planRelocation(model, "Old.md", "exists.md")).toThrow("already");
});
