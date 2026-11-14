//! TOML detection manifests.
//!
//! A manifest names an agent and lists rules. Rules are tried from highest to
//! lowest priority and the first match decides the state; if nothing matches,
//! an identified agent is idle. A rule matches when:
//!   - every top-level `contains` / `regex` matcher matches its region, and
//!   - at least one `any` entry matches (when `any` is present), and
//!   - no `not` entry matches.
//!
//! `contains` is case-insensitive. Regions:
//!   - `title`: the OSC window title
//!   - `bottom(N)`: the last N non-empty screen lines
//!   - `screen`: every visible line

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::Deserialize;

use super::{AgentState, Detection, Snapshot};

const BUILTIN_MANIFESTS: &[&str] = &[
    include_str!("../../manifests/claude.toml"),
    include_str!("../../manifests/codex.toml"),
];

/// Every known agent manifest.
pub struct Detectors {
    manifests: Vec<Manifest>,
}

impl Detectors {
    pub fn builtin() -> Self {
        let manifests = BUILTIN_MANIFESTS
            .iter()
            .map(|source| Manifest::parse(source).expect("builtin manifest must be valid"))
            .collect();
        Self { manifests }
    }

    /// Returns the manifest of the first process name that names a known agent.
    /// Names may be paths; only the basename is compared.
    pub fn identify<'a>(
        &self,
        process_names: impl IntoIterator<Item = &'a str>,
    ) -> Option<&Manifest> {
        process_names.into_iter().find_map(|name| {
            let base = name
                .rsplit('/')
                .next()
                .unwrap_or(name)
                .trim_start_matches('-');
            self.manifests.iter().find(|m| m.answers_to(base))
        })
    }
}

pub struct Manifest {
    pub id: String,
    aliases: Vec<String>,
    rules: Vec<Rule>,
}

impl Manifest {
    pub fn parse(source: &str) -> Result<Self> {
        let file: ManifestFile = toml::from_str(source).context("invalid manifest TOML")?;
        let mut rules = file
            .rules
            .into_iter()
            .map(|rule| Rule::compile(rule).with_context(|| format!("in manifest {:?}", file.id)))
            .collect::<Result<Vec<_>>>()?;
        rules.sort_by_key(|rule| std::cmp::Reverse(rule.priority));
        Ok(Self {
            id: file.id,
            aliases: file.aliases,
            rules,
        })
    }

    pub fn detect(&self, snapshot: &Snapshot) -> Detection {
        match self.rules.iter().find(|rule| rule.matches(snapshot)) {
            Some(rule) => Detection {
                state: rule.state,
                rule: Some(rule.id.clone()),
            },
            None => Detection {
                state: AgentState::Idle,
                rule: None,
            },
        }
    }

    fn answers_to(&self, name: &str) -> bool {
        self.id == name || self.aliases.iter().any(|alias| alias == name)
    }
}

#[derive(Deserialize)]
struct ManifestFile {
    id: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    rules: Vec<RuleFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleFile {
    id: String,
    state: AgentState,
    #[serde(default)]
    priority: i32,
    region: String,
    #[serde(default)]
    contains: Vec<String>,
    #[serde(default)]
    regex: Vec<String>,
    #[serde(default)]
    any: Vec<MatcherFile>,
    #[serde(default)]
    not: Vec<MatcherFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MatcherFile {
    #[serde(default)]
    contains: Vec<String>,
    #[serde(default)]
    regex: Vec<String>,
}

struct Rule {
    id: String,
    state: AgentState,
    priority: i32,
    region: Region,
    all: Matcher,
    any: Vec<Matcher>,
    not: Vec<Matcher>,
}

impl Rule {
    fn compile(file: RuleFile) -> Result<Self> {
        let context = || format!("rule {:?}", file.id);
        let all = Matcher::compile(file.contains, file.regex).with_context(context)?;
        let compile_all = |matchers: Vec<MatcherFile>| {
            matchers
                .into_iter()
                .map(|m| Matcher::compile(m.contains, m.regex))
                .collect::<Result<Vec<_>>>()
        };
        Ok(Self {
            region: Region::parse(&file.region).with_context(context)?,
            any: compile_all(file.any).with_context(context)?,
            not: compile_all(file.not).with_context(context)?,
            all,
            id: file.id,
            state: file.state,
            priority: file.priority,
        })
    }

    fn matches(&self, snapshot: &Snapshot) -> bool {
        let text = self.region.extract(snapshot);
        let haystack = Haystack {
            lower: text.to_lowercase(),
            text,
        };
        self.all.matches(&haystack)
            && (self.any.is_empty() || self.any.iter().any(|m| m.matches(&haystack)))
            && !self.not.iter().any(|m| m.matches(&haystack))
    }
}

#[derive(Debug, PartialEq)]
enum Region {
    Title,
    Bottom(usize),
    Screen,
}

impl Region {
    fn parse(spec: &str) -> Result<Self> {
        let spec = spec.trim();
        if spec == "title" {
            return Ok(Self::Title);
        }
        if spec == "screen" {
            return Ok(Self::Screen);
        }
        if let Some(count) = spec
            .strip_prefix("bottom(")
            .and_then(|s| s.strip_suffix(')'))
        {
            return Ok(Self::Bottom(
                count.trim().parse().context("bottom(N) needs a number")?,
            ));
        }
        bail!("unknown region {spec:?}");
    }

    fn extract(&self, snapshot: &Snapshot) -> String {
        match self {
            Self::Title => snapshot.title.to_owned(),
            Self::Screen => snapshot.lines.join("\n"),
            Self::Bottom(count) => {
                let mut lines: Vec<&str> = snapshot
                    .lines
                    .iter()
                    .rev()
                    .map(String::as_str)
                    .filter(|line| !line.trim().is_empty())
                    .take(*count)
                    .collect();
                lines.reverse();
                lines.join("\n")
            }
        }
    }
}

struct Haystack {
    text: String,
    lower: String,
}

/// All of `contains` (case-insensitive) and all of `regex` must match.
/// An empty matcher matches everything.
struct Matcher {
    contains: Vec<String>,
    regex: Vec<Regex>,
}

impl Matcher {
    fn compile(contains: Vec<String>, regex: Vec<String>) -> Result<Self> {
        Ok(Self {
            contains: contains.into_iter().map(|s| s.to_lowercase()).collect(),
            regex: regex
                .iter()
                .map(|pattern| {
                    Regex::new(pattern).with_context(|| format!("bad regex {pattern:?}"))
                })
                .collect::<Result<_>>()?,
        })
    }

    fn matches(&self, haystack: &Haystack) -> bool {
        self.contains
            .iter()
            .all(|needle| haystack.lower.contains(needle))
            && self.regex.iter().all(|re| re.is_match(&haystack.text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(screen: &str) -> Vec<String> {
        screen.lines().map(|l| l.trim_end().to_owned()).collect()
    }

    fn detect(detectors: &Detectors, agent: &str, title: &str, screen: &str) -> Detection {
        let manifest = detectors.identify([agent]).expect("agent manifest");
        let lines = lines(screen);
        manifest.detect(&Snapshot {
            title,
            lines: &lines,
        })
    }

    #[test]
    fn parses_regions() {
        assert_eq!(Region::parse("title").unwrap(), Region::Title);
        assert_eq!(Region::parse("bottom(12)").unwrap(), Region::Bottom(12));
        assert_eq!(Region::parse("screen").unwrap(), Region::Screen);
        assert!(Region::parse("bottom(x)").is_err());
        assert!(Region::parse("top(3)").is_err());
    }

    #[test]
    fn rule_combinators() {
        let manifest = Manifest::parse(
            r#"
            id = "toy"
            [[rules]]
            id = "blocked"
            state = "blocked"
            priority = 10
            region = "bottom(3)"
            contains = ["proceed?"]
            any = [{ contains = ["yes"] }, { regex = ['^\s*1\.'] }]
            not = [{ contains = ["cancelled"] }]

            [[rules]]
            id = "working"
            state = "working"
            region = "title"
            regex = ['^\*']
            "#,
        )
        .unwrap();
        let run = |title: &str, screen: &str| {
            let lines = lines(screen);
            manifest
                .detect(&Snapshot {
                    title,
                    lines: &lines,
                })
                .state
        };

        assert_eq!(run("* busy", "Proceed?\n YES / no"), AgentState::Blocked);
        assert_eq!(run("* busy", "Proceed?\n maybe"), AgentState::Working);
        assert_eq!(run("idle", "Proceed? yes\ncancelled"), AgentState::Idle);
        assert_eq!(run("idle", "nothing here"), AgentState::Idle);
    }

    #[test]
    fn rejects_unknown_rule_fields() {
        let source = "id='x'\n[[rules]]\nid='r'\nstate='idle'\nregion='screen'\ncontain=['typo']";
        assert!(Manifest::parse(source).is_err());
    }

    #[test]
    fn identifies_agents_by_basename() {
        let detectors = Detectors::builtin();
        assert_eq!(
            detectors.identify(["/usr/local/bin/claude"]).unwrap().id,
            "claude"
        );
        assert_eq!(detectors.identify(["zsh", "codex"]).unwrap().id, "codex");
        assert!(detectors.identify(["-zsh", "vim"]).is_none());
    }

    #[test]
    fn claude_states() {
        let detectors = Detectors::builtin();
        let prompt = "╭──────────╮\n│ >        │\n╰──────────╯\n  ? for shortcuts";

        assert_eq!(
            detect(&detectors, "claude", "✳ Claude Code", prompt).state,
            AgentState::Idle
        );
        assert_eq!(
            detect(&detectors, "claude", "⠂ Fixing tests", prompt).state,
            AgentState::Working
        );
        assert_eq!(
            detect(
                &detectors,
                "claude",
                "✳ Claude Code",
                "✻ Pondering… (12s · ↓ 300 tokens · esc to interrupt)\n\n│ > │"
            )
            .state,
            AgentState::Working
        );
        assert_eq!(
            detect(
                &detectors,
                "claude",
                "⠂ Running",
                "Bash command\n  rm -rf build\nDo you want to proceed?\n❯ 1. Yes\n  2. No\nEsc to cancel"
            )
            .state,
            AgentState::Blocked
        );
    }

    #[test]
    fn codex_states() {
        let detectors = Detectors::builtin();
        assert_eq!(
            detect(&detectors, "codex", "", "› Ask Codex").state,
            AgentState::Idle
        );
        assert_eq!(
            detect(
                &detectors,
                "codex",
                "",
                "• Working (5s • esc to interrupt)\n› "
            )
            .state,
            AgentState::Working
        );
        assert_eq!(
            detect(
                &detectors,
                "codex",
                "",
                "Allow command?\n  cargo test\n› 1. Yes\n  2. No"
            )
            .state,
            AgentState::Blocked
        );
    }
}
