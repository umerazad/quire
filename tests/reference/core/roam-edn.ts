import { readEDN, ednJSON } from "./edn";
import { importRoamJSON, type RoamBlock, type RoamPage } from "./roam";

export function importRoamEDN(text: string) {
  const parsed = readEDN(text) as { tag?: string; value?: unknown } | null;
  const invalid = (reason = "outline"): never => {
    throw new Error(
      `The Roam EDN database has an invalid ${reason}. Export the graph again as JSON or EDN and retry.`,
    );
  };
  if (parsed?.tag !== "datascript/DB" || !(parsed.value instanceof Map))
    return invalid();
  const schema = parsed.value.get(":schema");
  const datoms = parsed.value.get(":datoms");
  if (!(schema instanceof Map) || !Array.isArray(datoms)) return invalid();
  const entities = new Map<number, Map<string, unknown>>();
  for (const datom of datoms) {
    if (!Array.isArray(datom) || datom.length < 4 || datom.length > 5)
      return invalid("datom shape");
    const [id, attribute, value, transaction, added] = datom;
    if (
      !Number.isSafeInteger(id) ||
      typeof attribute !== "string" ||
      !attribute.startsWith(":") ||
      !Number.isSafeInteger(transaction) ||
      added === false
    )
      return invalid("datom value");
    if (!entities.has(id)) entities.set(id, new Map());
    const entity = entities.get(id)!;
    const declaration = schema.get(attribute);
    if (
      declaration instanceof Map &&
      declaration.get(":db/cardinality") === ":db.cardinality/many"
    ) {
      const values = (entity.get(attribute) ?? []) as unknown[];
      values.push(value);
      entity.set(attribute, values);
    } else {
      if (entity.has(attribute)) return invalid("single-valued attribute");
      entity.set(attribute, value);
    }
  }
  const children = new Map<number, number[]>();
  const parent = new Map<number, number>();
  const pages = [...entities].filter(([, entity]) => entity.has(":node/title"));
  const blocks = [...entities].filter(([, entity]) =>
    entity.has(":block/string"),
  );
  let missingOrders = 0;
  for (const [id, entity] of entities) {
    const list = entity.get(":block/children") ?? [];
    if (!Array.isArray(list)) return invalid();
    for (const child of list) {
      if (
        !Number.isSafeInteger(child) ||
        !entities.get(child)?.has(":block/string") ||
        parent.has(child)
      )
        return invalid("child or repeated parent");
      parent.set(child, id);
      const order = entities.get(child)!.get(":block/order");
      if (order === undefined) missingOrders++;
      else if (typeof order !== "number" || !Number.isFinite(order))
        return invalid();
    }
    children.set(
      id,
      [...list].sort((a, b) => {
        const left = entities.get(a)!.get(":block/order");
        const right = entities.get(b)!.get(":block/order");
        return (
          (typeof left === "number" ? left : Number.MAX_SAFE_INTEGER) -
            (typeof right === "number" ? right : Number.MAX_SAFE_INTEGER) ||
          a - b
        );
      }),
    );
  }
  const aliases: Record<string, string> = {
    ":block/uid": "uid",
    ":block/string": "string",
    ":node/title": "title",
    ":block/open": "open",
    ":block/heading": "heading",
    ":block/text-align": "text-align",
    ":children/view-type": "children-view-type",
    ":block/view-type": "view-type",
    ":create/time": "create-time",
    ":edit/time": "edit-time",
  };
  const attributes = (entity: Map<string, unknown>) =>
    Object.fromEntries(
      [...entity]
        .filter(([key]) => key !== ":block/children")
        .map(([key, value]) => {
          if (key === ":create/user" || key === ":edit/user") {
            const user = entities.get(value as number);
            const uid = user?.get(":user/uid");
            if (typeof uid === "string") return [key, { ":user/uid": uid }];
          }
          return [aliases[key] ?? key, ednJSON(value)];
        }),
    );
  const visited = new Set<number>();
  let repairedIds = 0;
  const build = (id: number, depth: number): RoamBlock => {
    if (depth > 128 || visited.has(id))
      return invalid("cyclic, repeated or excessively nested block");
    visited.add(id);
    const entity = entities.get(id)!;
    if (typeof entity.get(":block/string") !== "string") return invalid();
    const node = attributes(entity);
    if (node.uid === "") {
      delete node.uid;
      repairedIds++;
    }
    if (children.get(id)!.length)
      node.children = children.get(id)!.map((child) => build(child, depth + 1));
    return node as RoamBlock;
  };
  const output = pages.map(([id, entity]) => {
    if (entity.has(":block/string") || parent.has(id)) return invalid();
    return {
      ...attributes(entity),
      children: children.get(id)!.map((child) => build(child, 0)),
    } as RoamPage;
  });
  const detached = blocks.filter(([id]) => !visited.has(id));
  if (detached.length) {
    const titles = new Set(
      pages.map(([, entity]) => entity.get(":node/title")),
    );
    let title = "Roam recovered blocks",
      suffix = 2;
    while (titles.has(title)) title = `Roam recovered blocks ${suffix++}`;
    const recovered = detached
      .filter(
        ([id]) =>
          !parent.has(id) ||
          !entities.get(parent.get(id)!)?.has(":block/string"),
      )
      .map(([id]) => build(id, 0));
    output.push({ title, children: recovered });
  }
  if (visited.size !== blocks.length)
    return invalid(
      `unreachable outline (${blocks.length - visited.size} text blocks)`,
    );
  const result = importRoamJSON(output);
  result.report.splice(
    1,
    0,
    `Read ${datoms.length} DataScript datoms. Recovered ${detached.length} detached text blocks on a separate recovery page. The original database archive retains all non-outline entities and settings.`,
    ...(repairedIds
      ? [
          `Assigned new identifiers to ${repairedIds} blocks with empty database UIDs; original values remain in the source archive.`,
        ]
      : []),
    ...(missingOrders
      ? [
          `${missingOrders} blocks had no order; placed them after ordered siblings using their database IDs.`,
        ]
      : []),
  );
  return result;
}
