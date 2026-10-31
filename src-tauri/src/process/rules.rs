//! Which tool a process belongs to, from its name, command line or app bundle.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::raw::RawProcess;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolKind {
    App,
    Test,
    Dev,
    Agent,
    Other,
}

/// (executable or package name, kind, label). Agent tools come first so they win a tie.
const TOOLS: &[(&str, ToolKind, &str)] = &[
    ("claude", ToolKind::Agent, "Claude Code"),
    ("codex", ToolKind::Agent, "Codex"),
    ("cursor-agent", ToolKind::Agent, "Cursor Agent"),
    ("vitest", ToolKind::Test, "Vitest"),
    ("jest", ToolKind::Test, "Jest"),
    ("playwright", ToolKind::Test, "Playwright"),
    ("next", ToolKind::Dev, "next dev"),
    ("next-server", ToolKind::Dev, "next dev"),
    ("vite", ToolKind::Dev, "Vite"),
    ("tsx", ToolKind::Dev, "tsx"),
    ("bun", ToolKind::Dev, "Bun"),
];

fn base_name(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

fn args_mention(cmd: &[String], tool: &str) -> bool {
    let package_dir = format!("node_modules/{tool}/");
    let bin = format!(".bin/{tool}");
    cmd.iter().any(|a| a.contains(&package_dir) || a.ends_with(&bin))
}

/// Matches the executable name (`cmd[0]` or the process name), or an argument that points into
/// `node_modules/<tool>/` or `.bin/<tool>`.
pub fn match_tool(p: &RawProcess) -> Option<(ToolKind, &'static str)> {
    let first = p.cmd.first().map(|c| base_name(c));
    TOOLS
        .iter()
        .find(|(tool, _, _)| first == Some(*tool) || p.name == *tool || args_mention(&p.cmd, tool))
        .map(|&(_, kind, label)| (kind, label))
}

/// The outermost `.app` directory in the path, so helpers nested inside a bundle join their parent app.
pub fn outer_app_bundle(exe: &Path) -> Option<PathBuf> {
    let mut bundle = PathBuf::new();
    for part in exe.components() {
        bundle.push(part);
        if part.as_os_str().to_str().is_some_and(|s| s.ends_with(".app")) {
            return Some(bundle);
        }
    }
    None
}

/// An agent process the owner started. Claude's browser bridge (`--chrome-native-host`) is not a session.
pub fn is_agent_session(p: &RawProcess) -> bool {
    matches!(match_tool(p), Some((ToolKind::Agent, _)))
        && !p.cmd.iter().any(|a| a == "--chrome-native-host")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::test_support::proc;

    #[test]
    fn claude_inside_an_app_bundle_is_still_the_agent() {
        let p = proc(10, 1)
            .exe("/Users/x/.local/share/claude/ClaudeCode.app/Contents/MacOS/claude")
            .cmd(&["claude"]);
        assert_eq!(match_tool(&p), Some((ToolKind::Agent, "Claude Code")));
    }

    #[test]
    fn chrome_helpers_join_the_outermost_bundle() {
        let exe = Path::new("/Applications/Google Chrome.app/Contents/Frameworks/G.framework/Helpers/Google Chrome Helper.app/Contents/MacOS/Google Chrome Helper");
        assert_eq!(outer_app_bundle(exe), Some(PathBuf::from("/Applications/Google Chrome.app")));
        assert_eq!(outer_app_bundle(Path::new("/usr/bin/zsh")), None);
    }

    #[test]
    fn tools_match_by_package_path_but_not_by_prefix() {
        let vitest = proc(1, 0).name("node").cmd(&["node", "/w/a/node_modules/.bin/vitest"]);
        assert_eq!(match_tool(&vitest), Some((ToolKind::Test, "Vitest")));
        let vite = proc(2, 0).name("node").cmd(&["node", "/w/a/node_modules/vite/bin/vite.js"]);
        assert_eq!(match_tool(&vite), Some((ToolKind::Dev, "Vite")));
        let plain = proc(3, 0).name("node").cmd(&["node", "server.js"]);
        assert_eq!(match_tool(&plain), None);
    }

    #[test]
    fn chrome_native_host_is_not_an_agent_session() {
        let host = proc(1, 0).name("claude").cmd(&["/u/.local/bin/claude", "--chrome-native-host"]);
        assert!(!is_agent_session(&host));
        assert!(is_agent_session(&proc(2, 0).name("claude").cmd(&["claude"])));
    }
}
