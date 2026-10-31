import { describe, it, expect } from "vitest";
import { importRoamJSON, exportRoamJSON } from "./core/roam";
import { parseBlocks, references } from "./markdown/markdown";
describe("Roam migration", () => {
  it("round-trips case-sensitive Roam titles and links through portable filenames", () => {
    const original = [
      { title: "Case", children: [{ uid: "upper", string: "[[case]]" }] },
      { title: "case", children: [{ uid: "lower", string: "[[Case]]" }] },
    ];
    const imported = importRoamJSON(original);
    expect(exportRoamJSON(imported.documents, imported.metadata).pages).toEqual(
      original,
    );
  });
  it("normalizes filename suffixes after truncation and bounds Unicode byte length", () => {
    const titles = [
      "a".repeat(129) + "." + "b".repeat(52),
      "a".repeat(129) + " " + "b".repeat(52),
      "語".repeat(140),
      "CON",
      "..",
      "a".repeat(129),
    ];
    const imported = importRoamJSON(titles.map((title) => ({ title })));
    expect(
      new Set(imported.documents.map((doc) => doc.path.toLowerCase())).size,
    ).toBe(titles.length);
    for (const doc of imported.documents) {
      expect(Buffer.byteLength(doc.path)).toBeLessThan(255);
      expect(doc.path.slice(0, -3)).not.toMatch(/[. ]$/);
    }
    expect(
      Object.values(imported.metadata.pages).map((page) => page.title),
    ).toEqual(titles);
  });
  it("preserves hierarchy, identities, references, and editable embeds", () => {
    const imported = importRoamJSON([
      {
        title: "One",
        children: [
          {
            uid: "parent",
            string: "Parent",
            children: [{ uid: "child", string: "Child" }],
          },
        ],
      },
      {
        title: "Two",
        children: [
          { uid: "ref", string: "((child))" },
          { uid: "embed", string: "{{embed: ((parent))}}" },
        ],
      },
    ]);
    expect(imported.documents).toHaveLength(2);
    expect(
      parseBlocks(imported.documents[0].text).map((block) => block.id),
    ).toEqual(["parent", "child"]);
    expect(
      references(imported.documents[1].text).map((ref) => ref.kind),
    ).toEqual(["block", "embed"]);
    expect(imported.report[0]).toContain("4 blocks");
  });
  it("makes portable filenames and reports unresolved references", () => {
    const imported = importRoamJSON([
      { title: "A/B", children: [{ uid: "a", string: "((missing))" }] },
      { title: "a:b", children: [] },
    ]);
    expect(
      new Set(imported.documents.map((doc) => doc.path.toLowerCase())).size,
    ).toBe(2);
    expect(imported.report.join("\n")).toContain(
      "Unresolved reference: missing",
    );
  });
  it("keeps hidden, trimmed and Unicode-normalized page names discoverable and distinct", () => {
    const imported = importRoamJSON([
      { title: " A" },
      { title: "A" },
      { title: ".hidden" },
      { title: "e\u0301" },
      { title: "é" },
    ]);
    expect(
      new Set(imported.documents.map((doc) => doc.path.toLowerCase())).size,
    ).toBe(5);
    expect(imported.documents.every((doc) => !doc.path.startsWith("."))).toBe(
      true,
    );
    expect(Object.keys(imported.metadata.pages)).toHaveLength(5);
  });
  it("rejects malformed exports and duplicate IDs", () => {
    expect(() => importRoamJSON({})).toThrow("Roam JSON");
    expect(() =>
      importRoamJSON([
        {
          title: "Page",
          children: [
            { uid: "same", string: "A" },
            { uid: "same", string: "B" },
          ],
        },
      ]),
    ).toThrow("duplicate");
  });
  it("round-trips nested Roam data with metadata, code, tasks and native embeds", () => {
    const original = [
      {
        title: "A/B",
        uid: "page-one",
        "create-time": 123,
        "edit-user": { ":user/uid": "writer" },
        children: [
          {
            uid: "parent",
            string: "Parent",
            heading: 2,
            open: false,
            "text-align": "center",
            children: [
              {
                uid: "child",
                string: "First line\nSecond line\n",
                "edit-time": 456,
              },
              {
                uid: "code",
                string: "```js\nconst example = '((child))';\n```",
              },
            ],
          },
          { uid: "task", string: "{{[[TODO]]}} Finish ((child))" },
          { uid: "embed", string: "{{[[embed]]: ((parent))}}" },
        ],
      },
    ];
    const imported = importRoamJSON(original);
    expect(
      parseBlocks(imported.documents[0].text).map((block) => block.id),
    ).toEqual(["parent", "child", "code", "task", "embed"]);
    expect(exportRoamJSON(imported.documents, imported.metadata).pages).toEqual(
      original,
    );
  });
  it("exports native Markdown hierarchy and converts qualified block references", () => {
    const documents = [
      {
        path: "Folder/A.md",
        text: "- Parent ^parent\n  - Child ^child\n    - [ ] Task ^task\n- ## Heading ^heading\n",
      },
      {
        path: "B.md",
        text: "![[Folder/A#^parent]]\n\n[[Folder/A#^child]]\n\n![image](assets/photo.png)\n\n![[Folder/A]]\n",
      },
    ];
    const exported = exportRoamJSON(documents);
    expect(exported.pages[0].children).toMatchObject([
      {
        uid: "parent",
        string: "Parent",
        children: [
          {
            uid: "child",
            string: "Child",
            children: [{ uid: "task", string: "{{[[TODO]]}} Task" }],
          },
        ],
      },
      { uid: "heading", string: "Heading", heading: 2 },
    ]);
    expect(exported.pages[1].children?.map((block) => block.string)).toEqual([
      "{{[[embed]]: ((parent))}}",
      "((child))",
      "![image](assets/photo.png)",
      "{{[[embed]]: [[A]]}}",
    ]);
    expect(exported.report.join("\n")).toContain("attachment");
    expect(documents[0].text).toBe(
      "- Parent ^parent\n  - Child ^child\n    - [ ] Task ^task\n- ## Heading ^heading\n",
    );
    const imported = importRoamJSON(exported.pages);
    expect(exportRoamJSON(imported.documents, imported.metadata).pages).toEqual(
      exported.pages,
    );
  });
  it("keeps metadata for edited blocks while exporting their current text", () => {
    const imported = importRoamJSON([
      {
        title: "Page",
        children: [
          {
            uid: "one",
            string: "Original",
            "create-time": 1,
            "edit-time": 2,
            open: false,
          },
        ],
      },
    ]);
    imported.documents[0].text = imported.documents[0].text.replace(
      "- Original ^one",
      "- Edited ^one",
    );
    const block = exportRoamJSON(imported.documents, imported.metadata).pages[0]
      .children![0];
    expect(block).toMatchObject({
      uid: "one",
      string: "Edited",
      "create-time": 1,
      open: false,
    });
    expect(block["edit-time"]).toBeUndefined();
  });
  it("keeps multiline list text inside its original Roam block when another block is edited", () => {
    const original = [
      {
        title: "Page",
        children: [
          {
            uid: "literal",
            string: "- First\n2. Second",
            children: [{ uid: "actual-child", string: "Real child" }],
          },
          { uid: "edited", string: "Before" },
        ],
      },
    ];
    const imported = importRoamJSON(original);
    expect(
      parseBlocks(imported.documents[0].text).map((block) => block.id),
    ).toEqual(["literal", "actual-child", "edited"]);
    imported.documents[0].text = imported.documents[0].text.replace(
      "- Before ^edited",
      "- After ^edited",
    );
    const output = exportRoamJSON(imported.documents, imported.metadata)
      .pages[0];
    expect(output.children![0]).toEqual(original[0].children[0]);
    expect(output.children![1].string).toBe("After");
  });
  it("keeps descendants nested under empty imported blocks", () => {
    const imported = importRoamJSON([
      {
        title: "Page",
        children: [
          {
            uid: "empty",
            string: "",
            children: [{ uid: "child", string: "Child" }],
          },
        ],
      },
    ]);
    const blocks = parseBlocks(imported.documents[0].text);
    expect(blocks.map((block) => block.id)).toEqual(["empty", "child"]);
    expect(blocks[1].parent).toBe("empty");
  });
  it("remaps unsupported IDs consistently without modifying code examples", () => {
    const imported = importRoamJSON([
      {
        title: "Page",
        children: [
          { uid: "space id", string: "Original" },
          { uid: "ref", string: "((space id)) and `((space id))`" },
        ],
      },
    ]);
    const exported = exportRoamJSON(imported.documents, imported.metadata);
    const uid = exported.pages[0].children![0].uid;
    expect(uid).not.toBe("space id");
    expect(exported.pages[0].children![1].string).toBe(
      `((${uid})) and \`((space id))\``,
    );
  });
  it("rejects malformed graphs and ambiguous exports with actionable errors", () => {
    expect(() => importRoamJSON([null])).toThrow("page has no title");
    expect(() => importRoamJSON([{ title: "Page", children: {} }])).toThrow(
      "children must be an array",
    );
    expect(() =>
      exportRoamJSON([
        { path: "A.md", text: "- A ^same" },
        { path: "B.md", text: "- B ^same" },
      ]),
    ).toThrow("Duplicate block");
    expect(() =>
      exportRoamJSON([
        { path: "A.md", text: '---\ntitle: "Same"\n---\nA' },
        { path: "B.md", text: '---\ntitle: "Same"\n---\nB' },
      ]),
    ).toThrow("distinct titles");
  });
});
