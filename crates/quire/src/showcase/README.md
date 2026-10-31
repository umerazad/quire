# Rendering at a glance

One page to check text, color, spacing, hierarchy and different scripts. Switch between Write and Source above. Click inside a block to edit; in Vim, press i first. Reset sample restores this page. Sample edits reset when the app restarts.

AI commands: ((readme-ai)). Click the reference to jump to the section.

## English · اردو · العربية

English: The quick brown fox jumps over the lazy dog. Clear notes make room for the next thought.

اردو: یہ آپ کی یادداشتوں کی ایک مثال ہے۔ خوب صورت لکھائی، واضح الفاظ اور نئے خیالات کو یہاں دیکھیں۔

العربية: هذه صفحة لتجربة عرض الملاحظات. نكتب الأفكار بوضوح، ثم نعود إليها عندما نحتاجها.

Arabic with full vowel marks (tashkeel), to check that marks sit clear of each other and of the next line:

بِسْمِ اللَّهِ الرَّحْمَٰنِ الرَّحِيمِ

كَتَبَ الطَّالِبُ الدَّرْسَ بِقَلَمٍ جَدِيدٍ، وَقَرَأَهُ عَلَى أُسْتَاذِهِ قِرَاءَةً حَسَنَةً.

الإِعْرَابُ: الطَّالِبُ فَاعِلٌ مَرْفُوعٌ، وَعَلَامَةُ رَفْعِهِ الضَّمَّةُ الظَّاهِرَةُ عَلَى آخِرِهِ. الدَّرْسَ مَفْعُولٌ بِهِ مَنْصُوبٌ.

الهَمْزَةُ وَالتَّنْوِينُ: سَأَلَ، يَسْأَلُ، سُؤَالٌ، شَيْءٌ، مَاءً، سَمَاءٍ · شُكْرًا جَزِيلًا

Mixed: Project منصوبہ / مشروع · Meeting at 10:30 صبح · Version 2.0 · ۱۲۳۴۵۶۷۸۹۰ · ١٢٣٤٥٦٧٨٩٠

- English outline: review the notes.
- اردو: آج کی ملاقات کے اہم نکات۔
- العربية: الخطوة التالية للمشروع.
- Mixed block: Review جائزہ ومراجعة #meeting

## Inline text and punctuation

Plain text · **bold emphasis** · _italic emphasis_ · _**bold and italic**_ · ~~completed thought~~ · `inline code`

Punctuation: “quoted words”, ‘single quotes’, (parentheses), [brackets], {braces}, + = / \ | @ & %.

Typography: office, efficient, affinity, ->, =>, !=, ===, <=, >=. Emoji: 📝 ✅ 🌱. Accents: café, naïve, résumé.

Escaped syntax: \#not-a-tag, \*not italic\*, \[literal brackets\].

## Tags and colors

#inbox #next #waiting #someday #done #meeting #project #blocked #idea

Nested and multilingual: #meeting/team #project/research #اردو #العربية #unclassified

Click a tag to browse this sample’s blocks. The examples stay separate from your real notebook tags and tasks.

## Tasks

- [ ] Capture a thought #inbox
- [ ] Send a short proposal #next
- [ ] Wait for feedback #waiting
- [ ] Explore an idea another day #someday
- [x] Finish a small action #next
- [ ] A task marked complete by a tag #done
- {{[[TODO]]}} Imported Roam task #waiting
- {{[[DONE]]}} Imported completed Roam task
- [ ] اردو: اگلا کام مکمل کریں۔ #next
- [ ] العربية: راجع ملاحظات الاجتماع. #waiting

Use Command/Ctrl-Enter on a block to create, complete or reopen a task. Command/Ctrl-Shift-Enter cycles its state.

## Nested blocks and vertical guides

Plain text stays plain text on Enter. To try nesting in Write, start a list with `- `, type a thought, press Enter, then Tab before typing a child. Repeat to nest further. Shift-Tab moves a block and its children out. Enter on an empty list item outdents or exits the outline. In Vim, use Insert mode for typing.

- Parent: this caret collapses the complete subtree. ^readme-outline
  - First child: follow the vertical guide.
    - Grandchild: the second indentation level.
      - Great-grandchild: the third indentation level.
    - Another grandchild: check where this guide ends.
  - Last child: the parent’s guide ends below this block.
- Sibling: the previous parent’s guide must not continue here.

1. An ordered item.
2. Another ordered item.
   1. An ordered child.
   2. Another ordered child.

## Links, block references and editable embeds

Page link: [[README]]. External link: [Markdown reference](https://commonmark.org/help/).

- One reusable thought, with children. ^readme-shared
  - A detail travels with its parent.
  - اردو اور العربية can sit beside English.

Inline reference: ((readme-shared)). The standalone embed below edits that same block and includes its children.

{{embed: ((readme-shared))}}

## AI commands

- Rewrite, draft and ask with the Claude Code or Codex command line tool signed in on this computer. Turn it on in Settings > AI. ^readme-ai
  - `:ai tighten` proposes a rewrite of the current block and its children. Enter accepts it as one undo step; Escape rejects it.
  - On a blank line, `:ai list three questions to ask` drafts new text in place.
  - `:ai` alone opens the answer pane. `:ai due`, `:ai week` and `:ai tagged reading` ask ready-made questions.
  - With Vim off, press Command/Ctrl-K and type AI.
- Answers cite pages as [[README]] and blocks as ((readme-shared)), so every claim opens its source.
- Changes the AI proposes wait under its answer until you press Apply. #idea

## Quotes and callout text

> A useful note makes the next thought easier.
>
> **Remember:** small, concrete actions are easier to start.

> [!NOTE]
> Obsidian callout syntax is shown here to compare its current rendering with an ordinary quote.

## Tables

| Type     |     English      |     اردو / العربية |
| :------- | :--------------: | -----------------: |
| Plain    | A clear thought  |           ایک خیال |
| **Bold** | **Next action**  | **الخطوة التالية** |
| Tag      |     #meeting     |              #اردو |
| Code     | `const next = 1` |              `۱۲۳` |

## Code blocks

```typescript
type Action = { title: string; done: boolean };
const action: Action = { title: "One small step", done: false };
console.log(action.title); // Syntax color, indentation and ligatures.
```

```json
{
  "english": "A clear next step",
  "urdu": "اگلا قدم",
  "arabic": "الخطوة التالية"
}
```

## Images and PDFs

These small samples are contained in this file and work offline.

![[data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSI0ODAiIGhlaWdodD0iMTIwIiB2aWV3Qm94PSIwIDAgNDgwIDEyMCI+PHJlY3Qgd2lkdGg9IjQ4MCIgaGVpZ2h0PSIxMjAiIHJ4PSIxMiIgZmlsbD0iI2VkZjNlZiIvPjxjaXJjbGUgY3g9IjU0IiBjeT0iNjAiIHI9IjI1IiBmaWxsPSIjMzU2YjViIi8+PHBhdGggZD0iTTQyIDYwbDkgOSAxNy0yMCIgc3Ryb2tlPSJ3aGl0ZSIgc3Ryb2tlLXdpZHRoPSI1IiBmaWxsPSJub25lIi8+PHRleHQgeD0iMTAwIiB5PSI1NSIgZm9udC1mYW1pbHk9InNhbnMtc2VyaWYiIGZvbnQtc2l6ZT0iMjIiIGZpbGw9IiMyODM3MmMiPkFuIG9mZmxpbmUgaW1hZ2Ugc2FtcGxlPC90ZXh0Pjx0ZXh0IHg9IjEwMCIgeT0iODMiIGZvbnQtZmFtaWx5PSJzYW5zLXNlcmlmIiBmb250LXNpemU9IjE2IiBmaWxsPSIjNDU2NjU3Ij5FZGdlcywgY29sb3IsIHNwYWNpbmcgYW5kIHNjYWxlPC90ZXh0Pjwvc3ZnPg==]]

![[data:application/pdf;base64,JVBERi0xLjQKMSAwIG9iago8PCAvVHlwZSAvQ2F0YWxvZyAvUGFnZXMgMiAwIFIgPj4KZW5kb2JqCjIgMCBvYmoKPDwgL1R5cGUgL1BhZ2VzIC9LaWRzIFszIDAgUl0gL0NvdW50IDEgPj4KZW5kb2JqCjMgMCBvYmoKPDwgL1R5cGUgL1BhZ2UgL1BhcmVudCAyIDAgUiAvTWVkaWFCb3ggWzAgMCA0ODAgMjYwXSAvUmVzb3VyY2VzIDw8IC9Gb250IDw8IC9GMSA0IDAgUiA+PiA+PiAvQ29udGVudHMgNSAwIFIgPj4KZW5kb2JqCjQgMCBvYmoKPDwgL1R5cGUgL0ZvbnQgL1N1YnR5cGUgL1R5cGUxIC9CYXNlRm9udCAvSGVsdmV0aWNhID4+CmVuZG9iago1IDAgb2JqCjw8IC9MZW5ndGggMTgwID4+CnN0cmVhbQpCVCAvRjEgMjQgVGYgNTAgMTkwIFRkIChUaHJlYWRub3RlIFBERiBzYW1wbGUpIFRqIDAgLTQyIFRkIC9GMSAxMyBUZiAoT25lIHBhZ2UsIGVtYmVkZGVkIGRpcmVjdGx5IGluIFJFQURNRS5tZC4pIFRqIDAgLTI0IFRkIChDaGVjayBpbmxpbmUgc2Nyb2xsaW5nLCBzY2FsZSBhbmQgdGV4dCBjbGFyaXR5LikgVGogRVQKZW5kc3RyZWFtCmVuZG9iagp4cmVmCjAgNgowMDAwMDAwMDAwIDY1NTM1IGYgCjAwMDAwMDAwMDkgMDAwMDAgbiAKMDAwMDAwMDA1OCAwMDAwMCBuIAowMDAwMDAwMTE1IDAwMDAwIG4gCjAwMDAwMDAyNDEgMDAwMDAgbiAKMDAwMDAwMDMxMSAwMDAwMCBuIAp0cmFpbGVyCjw8IC9TaXplIDYgL1Jvb3QgMSAwIFIgPj4Kc3RhcnR4cmVmCjU0MgolJUVPRgo=]]

## Heading sizes and wrapping

### Heading level three

#### Heading level four

##### Heading level five

###### Heading level six

This longer paragraph checks line wrapping at different window widths. It should remain comfortable to read when the sidebar is open, in a narrow window, and when another note is open beside it. **Bold words**, _italic words_, `inline code` and #meeting should keep their shape as the line wraps.

- A long list block checks wrapped-line spacing without losing its indentation or vertical guide. Resize the window and watch how this sentence, its **emphasis**, and the words منصوبہ and مشروع fit together.
  - A nested child underneath the wrapped parent.

---

End of the rendering check. F1 opens the full Help & shortcuts guide.
