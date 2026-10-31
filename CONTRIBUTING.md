# Contributing

Bug reports and fixes are welcome. For anything large, open an issue first so we can agree on the approach before you spend time on it.

## Setup

You need macOS, Node.js, [pnpm](https://pnpm.io), [Rust](https://rustup.rs) and the Xcode Command Line Tools.

```bash
pnpm install
pnpm tauri dev
```

The interface is in Indonesian only for now. Keep new interface text in Indonesian to match.

## Before opening a pull request

Run all of these. They should pass with no warnings:

```bash
pnpm vitest run
pnpm tsc --noEmit
cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings
```

## Rules that keep the project what it is

- **No network, no telemetry.** Pantau makes no network requests and the webview's CSP allows none. Don't add a dependency or a feature that changes that.
- **No kill or cleanup without a click.** Every process kill and every delete starts from a user action in the panel. Nothing runs on a timer, on startup or from a notification.
- **Chat history is never touched.** Pantau never deletes conversation history or `state.vscdb`, and it cleans only locations registered in `src-tauri/src/storage/rules.rs`. A new cleanup needs a registered location, a test that it refuses anything outside it, and a plain description of what it deletes.
- **Kills go through `authorize`.** Don't add a second path that sends a signal.
- **Tests never signal or delete on a real machine.** Use temporary directories and fake process tables. A test that sends a signal to a real pid, or removes anything under the real home folder, will be rejected.
- **Don't word a guess as a fact.** If Pantau can't know something, such as who owns a file, the interface says so.

By contributing, you agree that your work is released under the [MIT License](LICENSE).
