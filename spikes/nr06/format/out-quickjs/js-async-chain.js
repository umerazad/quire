async function load(id) {
  const res = await fetch(`/api/pages/${id}`, {
    headers: { Accept: "application/json" },
  });
  if (!res.ok) throw new Error("failed " + res.status);
  return res
    .json()
    .then((data) =>
      data.items
        .filter((i) => i.visible)
        .map((i) => ({ ...i, title: i.title.trim() })),
    );
}
