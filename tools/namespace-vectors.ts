import { writeFileSync } from "node:fs";
import { namespaceOf } from "../tests/reference/daily/workspace";

const reference = ["C/Survivorship Bias", "roam/js/clock", "Projects/Active", "Podcasts/Cooking (shows)", "Reading List/Archive", "Plain title", "20/20", "TCP / IP", "queries/", "/", "/Haskell/etc.", "[[roam/js/clock]]", "tweets: 1/ A thread about bread", "articles: Sourdough // A Baking Blog", "Read https://example.com/guide first", "Based on @Notes/Plan.md write a summary", "Garden Plan - Beds A and B/Watering", "books/Distributed systems"];
const parts = ["a", "Ab", "20", "2a", "a2", "C", "roam", "js", "é", "é", "中文", "مرحبا", "٣", "Ⅳ", "²", "🙂", "-x", "_x", ".x", "x.", "x-", "a b", "a  b", "Tom & Jerry", "O'Brien", "(x)", "x(", "[[x", "x]]", "@x", "https:", "x:", "#x", "x,y", "", " ", " a", "a ", " a", "a ", "﻿a", "a　", " a", "a\t", "᠎a", "a​"];
let state = 0x5eed1e;
const random = (length: number) => { state ^= state << 13; state ^= state >>> 17; state ^= state << 5; return (state >>> 0) % length; };
const separators = ["/", "/", "/", " / ", "//"];
const seeded = Array.from({ length: 2000 }, () => {
  const count = 1 + random(4);
  let title = parts[random(parts.length)];
  for (let index = 1; index < count; index++) title += separators[random(separators.length)] + parts[random(parts.length)];
  return title;
});
const singles = parts.flatMap(part => [part, `${part}/Leaf`, `Root/${part}`, `${part}/${part}/Leaf`]);
const titles = [...new Set([...reference, ...singles, ...seeded])];
writeFileSync("tests/fixtures/namespace-vectors.json", "[\n" + titles.map(title => JSON.stringify([title, namespaceOf(title)])).join(",\n") + "\n]\n");
console.log(`Generated ${titles.length} namespaceOf vectors from the frozen workspace module.`);
