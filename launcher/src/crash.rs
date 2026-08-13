use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CrashCategory {
    Mod,
    Mixin,
    Memory,
    Java,
    Graphics,
    Resourcepack,
    Auth,
    Network,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashAnalysis {
    pub category: CrashCategory,
    pub probable_cause: String,
    pub evidence: Vec<String>,
    pub actions: Vec<String>,
    pub exit_code: Option<i32>,
    pub suspected_mod: Option<String>,
}

pub fn analyze_crash(
    crash_report: Option<&str>,
    log: &str,
    exit_code: Option<i32>,
) -> CrashAnalysis {
    let combined = format!(
        "{}\n{}",
        crash_report.unwrap_or_default(),
        tail_lines(log, 500)
    );
    let lower = combined.to_ascii_lowercase();
    let rules = [
        (
            CrashCategory::Memory,
            [
                "outofmemoryerror",
                "java heap space",
                "unable to create native thread",
            ]
            .as_slice(),
            "Minecraft ran out of memory",
            ["Increase allocated RAM", "Disable memory-heavy mods"].as_slice(),
        ),
        (
            CrashCategory::Mixin,
            [
                "mixin apply failed",
                "mixintransformererror",
                "invalidmixinexception",
            ]
            .as_slice(),
            "A Mixin transformation failed",
            [
                "Disable the mod named near the Mixin error",
                "Check mod and loader versions",
            ]
            .as_slice(),
        ),
        (
            CrashCategory::Graphics,
            [
                "opengl",
                "glfw error",
                "failed to create window",
                "pixel format",
            ]
            .as_slice(),
            "The graphics driver or OpenGL initialization failed",
            [
                "Update the graphics driver",
                "Disable shaders and graphics mods",
            ]
            .as_slice(),
        ),
        (
            CrashCategory::Java,
            [
                "unsupportedclassversionerror",
                "jni error",
                "could not create the java virtual machine",
            ]
            .as_slice(),
            "The selected Java runtime is incompatible",
            [
                "Select the Java version required by Minecraft",
                "Remove unsupported JVM arguments",
            ]
            .as_slice(),
        ),
        (
            CrashCategory::Resourcepack,
            [
                "resource reload failed",
                "failed to load resource pack",
                "pack.mcmeta",
            ]
            .as_slice(),
            "A resource pack failed to load",
            ["Disable the resource pack", "Check its Minecraft version"].as_slice(),
        ),
        (
            CrashCategory::Auth,
            [
                "invalid session",
                "failed to verify username",
                "authentication servers",
            ]
            .as_slice(),
            "The Minecraft session is invalid",
            ["Sign in again", "Check Microsoft service availability"].as_slice(),
        ),
        (
            CrashCategory::Network,
            [
                "unknownhostexception",
                "connection timed out",
                "connection refused",
            ]
            .as_slice(),
            "A required network connection failed",
            [
                "Check the connection",
                "Retry after the service is available",
            ]
            .as_slice(),
        ),
        (
            CrashCategory::Mod,
            [
                "mod loading has failed",
                "modresolutionexception",
                "incompatible mods found",
                "fabricloader",
            ]
            .as_slice(),
            "A mod is missing or incompatible",
            [
                "Open the log and inspect the named mod",
                "Disable or update the suspected mod",
            ]
            .as_slice(),
        ),
    ];
    for (category, needles, cause, actions) in rules {
        if let Some(needle) = needles.iter().find(|needle| lower.contains(**needle)) {
            let evidence = combined
                .lines()
                .filter(|line| line.to_ascii_lowercase().contains(needle))
                .take(3)
                .map(str::to_string)
                .collect();
            let suspected_mod = if matches!(category, CrashCategory::Mod | CrashCategory::Mixin) {
                find_suspected_mod(&combined)
            } else {
                None
            };
            return CrashAnalysis {
                category,
                probable_cause: cause.to_string(),
                evidence,
                actions: actions.iter().map(|value| value.to_string()).collect(),
                exit_code,
                suspected_mod,
            };
        }
    }
    CrashAnalysis {
        category: CrashCategory::Unknown,
        probable_cause: "No known local crash signature was found".to_string(),
        evidence: combined
            .lines()
            .rev()
            .filter(|line| !line.trim().is_empty())
            .take(3)
            .map(str::to_string)
            .collect(),
        actions: vec![
            "Open the full log".to_string(),
            "Restore the last profile snapshot".to_string(),
        ],
        exit_code,
        suspected_mod: find_suspected_mod(&combined),
    }
}

fn find_suspected_mod(value: &str) -> Option<String> {
    value
        .split(|character: char| {
            character.is_whitespace()
                || matches!(
                    character,
                    '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | ':' | '"'
                )
        })
        .map(|token| token.trim_matches(|character: char| character == '\'' || character == '`'))
        .find(|token| token.to_ascii_lowercase().ends_with(".jar") && token.len() > 4)
        .map(|token| PathLikeName::file_name(token).unwrap_or(token).to_string())
}

struct PathLikeName;

impl PathLikeName {
    fn file_name(value: &str) -> Option<&str> {
        value
            .rsplit(['/', '\\'])
            .next()
            .filter(|name| !name.is_empty())
    }
}

fn tail_lines(value: &str, count: usize) -> String {
    let mut lines = value.lines().rev().take(count).collect::<Vec<_>>();
    lines.reverse();
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_local_crash_corpus() {
        for (log, category) in [
            (
                "Mixin apply failed for example.ModMixin",
                CrashCategory::Mixin,
            ),
            (
                "java.lang.OutOfMemoryError: Java heap space",
                CrashCategory::Memory,
            ),
            (
                "UnsupportedClassVersionError: class version 66",
                CrashCategory::Java,
            ),
            ("GLFW error: OpenGL unavailable", CrashCategory::Graphics),
            (
                "Failed to load resource pack pack.mcmeta",
                CrashCategory::Resourcepack,
            ),
            (
                "Invalid session; authentication servers unavailable",
                CrashCategory::Auth,
            ),
            (
                "java.net.UnknownHostException: resources.download.minecraft.net",
                CrashCategory::Network,
            ),
            (
                "Mod loading has failed: Forge mod incompatible",
                CrashCategory::Mod,
            ),
            ("FabricLoader ModResolutionException", CrashCategory::Mod),
        ] {
            assert_eq!(analyze_crash(None, log, Some(1)).category, category);
        }
    }

    #[test]
    fn extracts_suspected_mod_without_reading_more_than_log_tail() {
        assert_eq!(
            analyze_crash(None, "Mixin apply failed in mods/example-mod.jar", Some(1))
                .suspected_mod
                .as_deref(),
            Some("example-mod.jar")
        );
    }
}
