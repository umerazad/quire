//! The Quire MCP server as its own small binary, so tests can run `quire --mcp-server`'s exact
//! code as a native process without building the whole app into the test. It reports version
//! "test", as the reference server built for tests does.

#[cfg(unix)]
#[path = "../src/notebook_tools/stdio.rs"]
mod stdio;

fn main() {
    #[cfg(unix)]
    stdio::run(
        std::env::temp_dir().join("quire-mcp-test-missing-bridge.json"),
        "test",
    );
}
