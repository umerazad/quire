// Checks that the frozen export code and the import extraction reproduce rt's expectations: the
// export cases come from rt tests/page-export.test.ts and the import case from
// tests/engine.test.ts at 5ea5c22310ff, run over in-memory notebooks and folders.
import { expect, it } from "vitest";
import { pageName, pageTitle, parseBlocks } from "./markdown/markdown";
import { exportMarkdown } from "./core/page-export";
import { exportName, localizeAttachments } from "./core/export-files";
import { importMarkdown } from "./core/markdown-import";
import type { Notebook } from "./core/export-notebook";

function notebookWith(pages: Record<string, string>): Notebook {
  const paths = Object.keys(pages);
  const title = (path: string) => pageTitle(path, pages[path]);
  const findPage = (target: string) =>
    paths.find((path) => pageName(path).toLocaleLowerCase() === target.toLocaleLowerCase()) ??
    ((titled) => (titled.length === 1 ? titled[0] : undefined))(paths.filter((path) => title(path).toLocaleLowerCase() === target.toLocaleLowerCase()));
  const blocks = paths.flatMap((path) => parseBlocks(pages[path]).filter((block) => block.id).map((block) => ({ path, block })));
  return {
    text: (path) => pages[path] ?? "",
    title,
    findPage,
    resolve: (target) => {
      const found = blocks.filter((item) => item.block.id === target);
      return found.length === 1 ? found[0] : { error: "missing" };
    },
  };
}

it("exports a page as standalone Markdown with references resolved", () => {
  const notebook = notebookWith({
    "Plan.md":
      "# Plan\n\n- Goals for [[Launch|the launch]] ➕ [[2026-10-29]] ^goal\n  - Ship by **December**\n- See ((goal))\n- {{embed: ((src))}}\n- ![[diagram.png]]\n- ![[Notes]]\n- ![Chart](img/chart.png)\n- [[Missing page]] stays as text\n",
    "Source.md": "- Embedded root ^src\n  - Embedded child [[Plan]]\n",
    "Notes.md": "- Note body\n",
  });
  expect(exportMarkdown(notebook, "Plan.md")).toBe(
    "# Plan\n\n- Goals for the launch\n  - Ship by **December**\n- See Goals for the launch\n- Embedded root\n  - Embedded child Plan\n- ![diagram.png](quire-vault:diagram.png)\n- Notes\n  - Note body\n- ![Chart](quire-vault:img%2Fchart.png)\n- Missing page stays as text\n",
  );
});

it("points attachments at files inside the notebook only", () => {
  const inside: Record<string, string> = { "img/chart.png": "/real/notebook/img/chart.png" };
  expect(
    localizeAttachments(
      "![a](quire-vault:img%2Fchart.png) ![b](quire-vault:..%2Fsecret.txt) ![c](quire-vault:img%2Fescape.png) ![d](quire-vault:img%2Fgone.png)",
      (relative) => inside[relative],
    ),
  ).toBe("![a](</real/notebook/img/chart.png>) ![b](../secret.txt) ![c](img/escape.png) ![d](img/gone.png)");
  expect(exportName('Q3: plan / "draft"', "pdf")).toBe("Q3- plan - -draft-.pdf");
});

it("imports a Markdown folder with its files, leaving pages already here alone", () => {
  const result = importMarkdown(
    [
      { name: "People", children: [{ name: "Maram.md", file: true }] },
      { name: ".obsidian", children: [{ name: "x.md", file: true }] },
      { name: "assets", children: [{ name: "a.png", file: true }] },
      { name: "Plan.md", file: true },
    ],
    ["Plan.md"],
  );
  expect({ pages: result.pages, skipped: result.skipped, files: result.files.length }).toEqual({ pages: 1, skipped: 1, files: 1 });
  expect(result.written).toEqual(["People/Maram.md"]);
});
