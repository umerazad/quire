if (process.platform === "darwin" && process.env.CODEX_SANDBOX === "seatbelt") {
  console.error("Quire cannot access macOS desktop services inside the Codex sandbox. Run bun run dev from a regular macOS terminal. Use cargo test -p quire --features test-support for headless input checks.");
  process.exit(1);
}
const child = Bun.spawn(["cargo", "run", "--locked", "-p", "quire"], {
  stdin: "inherit", stdout: "inherit", stderr: "inherit",
});
process.exit(await child.exited);
