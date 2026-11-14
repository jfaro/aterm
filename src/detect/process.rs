//! Process inspection used to identify which agent owns a pane.

use std::collections::HashMap;
use std::process::Command;

/// Maps each process group id to the program names of its members.
///
/// For every process, both argv[0] and argv[1] are recorded so that agents
/// launched through an interpreter (`node /path/to/codex`) are still found.
pub fn process_groups() -> HashMap<i32, Vec<String>> {
    let output = match Command::new("ps")
        .args(["-A", "-o", "pgid=,args="])
        .output()
    {
        Ok(output) if output.status.success() => output.stdout,
        _ => return HashMap::new(),
    };
    parse_ps(&String::from_utf8_lossy(&output))
}

fn parse_ps(output: &str) -> HashMap<i32, Vec<String>> {
    let mut groups: HashMap<i32, Vec<String>> = HashMap::new();
    for line in output.lines() {
        let Some((pgid, args)) = line.trim_start().split_once(char::is_whitespace) else {
            continue;
        };
        let Ok(pgid) = pgid.parse() else { continue };
        let names = groups.entry(pgid).or_default();
        names.extend(args.split_whitespace().take(2).map(str::to_owned));
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::parse_ps;

    #[test]
    fn groups_programs_by_pgid() {
        let groups =
            parse_ps("  101 -zsh\n  202 /opt/bin/claude --resume\n  202 bash -c ls\n  bogus\n");
        assert_eq!(groups[&101], vec!["-zsh"]);
        assert_eq!(
            groups[&202],
            vec!["/opt/bin/claude", "--resume", "bash", "-c"]
        );
        assert_eq!(groups.len(), 2);
    }
}
