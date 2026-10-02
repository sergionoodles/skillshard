/// Accept explicit read commands joined by AND: a successful final status proves
/// every read ran. Refuse branching, substitutions, pipelines and redirections.
pub(super) fn read_paths(command: &str) -> Vec<String> {
    read_targets(command)
        .into_iter()
        .filter(|path| super::evidence::is_skill_path(path))
        .collect()
}

pub(super) fn read_targets(command: &str) -> Vec<String> {
    let Some(commands) = split_and_commands(command.trim()) else {
        return Vec::new();
    };
    commands
        .into_iter()
        .map(|command| read_single_targets(command.trim()))
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect()
}

fn split_and_commands(command: &str) -> Option<Vec<&str>> {
    let mut commands = Vec::new();
    let mut characters = command.char_indices();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    while let Some((index, character)) = characters.next() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if Some(character) == quote {
            quote = None;
            continue;
        }
        if quote.is_some() {
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
            continue;
        }
        if character == '&' {
            let (next, '&') = characters.next()? else {
                return None;
            };
            commands.push(&command[start..index]);
            start = next + 1;
        }
    }
    commands.push(&command[start..]);
    Some(commands)
}

fn read_single_targets(command: &str) -> Option<Vec<String>> {
    let tokens = tokenize(command)?;
    let executable = tokens.first().map(String::as_str)?;
    let arguments = &tokens[1..];
    match executable {
        "cat" | "/bin/cat" | "/usr/bin/cat" => cat_paths(arguments),
        "head" | "tail" | "/usr/bin/head" | "/usr/bin/tail" => end_paths(arguments),
        "sed" | "/bin/sed" | "/usr/bin/sed" => sed_paths(arguments),
        _ => None,
    }
}

fn tokenize(command: &str) -> Option<Vec<String>> {
    if command
        .chars()
        .any(|character| "\n\r;|&<>$`*?{}[]".contains(character))
    {
        return None;
    }
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in command.chars() {
        if escaped {
            token.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if Some(character) == quote {
            quote = None;
            continue;
        }
        if quote.is_some() {
            token.push(character);
            continue;
        }
        if character == '\'' || character == '"' {
            quote = Some(character);
            continue;
        }
        if character.is_whitespace() {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
            continue;
        }
        token.push(character);
    }
    if quote.is_some() || escaped {
        return None;
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    Some(tokens)
}

fn cat_paths(arguments: &[String]) -> Option<Vec<String>> {
    let mut paths = Vec::new();
    let mut has_options = true;
    for argument in arguments {
        if argument == "--" && has_options {
            has_options = false;
            continue;
        }
        if has_options && argument.starts_with('-') {
            if !matches!(
                argument.as_str(),
                "-n" | "-b" | "-s" | "-v" | "-A" | "-E" | "-T"
            ) {
                return None;
            }
            continue;
        }
        paths.push(argument.clone());
    }
    Some(paths)
}

fn end_paths(arguments: &[String]) -> Option<Vec<String>> {
    let mut paths = Vec::new();
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        if matches!(argument.as_str(), "-n" | "-c") {
            let count = arguments.next()?;
            if count.parse::<i64>().is_err() {
                return None;
            }
            continue;
        }
        if argument == "--" {
            paths.extend(arguments.cloned());
            break;
        }
        if let Some(count) = argument.strip_prefix('-') {
            if count.parse::<u64>().is_err() {
                return None;
            }
            continue;
        }
        paths.push(argument.clone());
    }
    Some(paths)
}

fn sed_paths(arguments: &[String]) -> Option<Vec<String>> {
    let mut arguments = arguments.iter();
    let first = arguments.next()?;
    let pattern = if first == "-n" {
        let next = arguments.next()?;
        if next == "-e" {
            arguments.next()?
        } else {
            next
        }
    } else if first == "-e" {
        arguments.next()?
    } else {
        first
    };
    if !pattern.ends_with('p')
        || !pattern[..pattern.len() - 1]
            .chars()
            .all(|character| character.is_ascii_digit() || ",~".contains(character))
    {
        return None;
    }
    let paths: Vec<_> = arguments.cloned().collect();
    if paths.iter().any(|path| path.starts_with('-')) {
        return None;
    }
    Some(paths)
}

#[cfg(test)]
mod tests {
    use super::read_paths;

    #[test]
    fn trailing_shell_whitespace_does_not_hide_a_skill_read() {
        assert_eq!(
            read_paths("  cat '/fixture/alpha/SKILL.md'\n\r\n"),
            ["/fixture/alpha/SKILL.md"]
        );
    }

    #[test]
    fn successful_and_chains_preserve_each_explicit_read() {
        assert_eq!(
            read_paths("cat /fixture/alpha/SKILL.md && sed -n '1,80p' /fixture/beta/SKILL.md"),
            ["/fixture/alpha/SKILL.md", "/fixture/beta/SKILL.md"]
        );
    }

    #[test]
    fn shell_branches_and_mixed_commands_are_not_guessed() {
        for command in [
            "false || cat /fixture/alpha/SKILL.md",
            "echo cat /fixture/alpha/SKILL.md && true",
            "cat /fixture/alpha/SKILL.md && rm /fixture/file",
            "cat /fixture/alpha/SKILL.md; cat /fixture/beta/SKILL.md",
            "cat /fixture/alpha/SKILL.md | head",
            "cat /fixture/alpha/SKILL.md &&",
            "cat '/fixture/a&&b/SKILL.md'",
        ] {
            assert!(read_paths(command).is_empty(), "{command}");
        }
    }
}
