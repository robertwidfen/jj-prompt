use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Write};
use std::process::{Command, ExitCode, Stdio};
use std::str::FromStr;

use clap::{Parser, ValueEnum};

const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const BLUE: &str = "\x1b[34m";
const MAGENTA: &str = "\x1b[35m";

// [48;2; means background color, [38;2; means foreground color
// remaining values are RGB values

// const RED: &str = "\x1b[38;2;224;108;117m";
// const GREEN: &str = "\x1b[38;2;152;195;121m";
// const BLUE: &str = "\x1b[38;2;97;175;239m";
// const MAGENTA: &str = "\x1b[38;2;198;120;221m";

// const LIGHT_GRAY: &str = "\x1b[38;2;120;120;120m";
// const BG_GREY: &str = "\x1b[48;2;40;44;52m";
// const BG_BLUE: &str = "\x1b[48;2;30;34;52m";

const RESET: &str = "\x1b[0m";

// make background to go to end of line
// const CLEAR_LINE: &str = "\x1b[K";

#[derive(Debug)]
pub struct Change {
    pub id: String,
    pub parents: Vec<String>,
    pub is_working_copy: bool,
    pub is_immutable: bool,
    pub has_conflict: bool,
    pub files_modified: usize,
    pub lines_added: usize,
    pub lines_removed: usize,
    pub bookmarks: Vec<String>,
    pub children: Vec<String>,
    pub distance: usize,
}

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Print the jj command and diagnostic information
    #[arg(short, long)]
    test: bool,

    /// Select prompt sections in output order; unique prefixes are accepted
    #[arg(
        short,
        long,
        action = clap::ArgAction::Append,
        value_name = "SECTION",
        help = "Select sections in output order; unique prefixes accepted (@, stats, conflicts, branches, main, last-change, counts)",
        value_parser = clap::value_parser!(OutputSection)
    )]
    show: Vec<OutputSection>,

    /// Mark color codes as zero-width for the shell using the prompt directly
    #[arg(long, value_enum, default_value_t = Shell::Plain)]
    shell: Shell,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Shell {
    /// Raw color codes, e.g. for starship which wraps them itself
    Plain,
    /// Wrap color codes in readline's \001 and \002 markers
    Bash,
    /// Wrap color codes in %{ %} and escape % as %%
    Zsh,
    /// Raw color codes, fish needs no wrapping
    Fish,
}

impl Shell {
    /// Make the color codes in `prompt` zero-width for the shell, so it can
    /// compute the prompt width correctly (otherwise line editing breaks).
    fn format(self, prompt: &str) -> String {
        let (start, end) = match self {
            Shell::Plain | Shell::Fish => return prompt.to_string(),
            // \[ and \] are decoded before command substitution, so they would
            // not work with PS1='$(jj-prompt)'; readline's own markers always do
            Shell::Bash => ("\x01", "\x02"),
            Shell::Zsh => ("%{", "%}"),
        };

        let mut result = String::with_capacity(prompt.len() * 2);
        let mut rest = prompt;
        while let Some(escape_start) = rest.find('\x1b') {
            let (text, escape) = rest.split_at(escape_start);
            let escape_len = escape.find('m').map_or(escape.len(), |m| m + 1);
            result += &self.escape_text(text);
            result += start;
            result += &escape[..escape_len];
            result += end;
            rest = &escape[escape_len..];
        }
        result += &self.escape_text(rest);
        result
    }

    /// Escape characters in plain text the shell would interpret in a prompt.
    fn escape_text(self, text: &str) -> String {
        match self {
            Shell::Zsh => text.replace('%', "%%"),
            _ => text.to_string(),
        }
    }
}

impl Args {
    fn sections(&self) -> Vec<OutputSection> {
        let requested = if self.show.is_empty() {
            vec![
                OutputSection::WorkingCopy,
                OutputSection::Stats,
                OutputSection::Conflicts,
                OutputSection::Branches,
                OutputSection::Main,
                OutputSection::LastChange,
                OutputSection::Counts,
            ]
        } else {
            self.show.clone()
        };

        requested
            .into_iter()
            .fold(Vec::new(), |mut sections, section| {
                if !sections.contains(&section) {
                    sections.push(section);
                }
                sections
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputSection {
    WorkingCopy,
    Stats,
    Conflicts,
    Branches,
    Main,
    LastChange,
    Counts,
}

impl FromStr for OutputSection {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let matching: Vec<_> = [
            ("@", Self::WorkingCopy),
            ("stats", Self::Stats),
            ("conflicts", Self::Conflicts),
            ("branches", Self::Branches),
            ("main", Self::Main),
            ("last-change", Self::LastChange),
            ("counts", Self::Counts),
        ]
        .into_iter()
        .filter(|(name, _)| name.starts_with(value))
        .collect();

        match matching.as_slice() {
            [(_, section)] => Ok(*section),
            [] => Err(format!(
                "unknown section '{value}'; expected @, stats, conflicts, branches, main, last-change, or counts"
            )),
            _ => Err(format!(
                "ambiguous section prefix '{value}'; use a longer prefix"
            )),
        }
    }
}

fn append_section(output: &mut String, fragment: &str) {
    let fragment = fragment.trim();
    if fragment.is_empty() {
        return;
    }

    if !output.is_empty() {
        output.push(' ');
    }
    output.push_str(fragment);
}

// --ignore-working-copy: do not snapshot the working copy. Snapshotting would
// make the prompt slower and users do not expect jj to snapshot when they did
// not run it themselves. As a consequence the stats of @ reflect the last
// snapshot, i.e. the state as of the last jj command the user ran.
//
// The diff stat of @ is emitted per file as "added:removed,..." so the
// (expensive) stat is computed only once; templates cannot bind values.
const JJ_LOG_TEMPLATE: &str = r#"change_id.shortest() ++ " " ++
    parents.map(|p| p.change_id().shortest()).join(",") ++ " " ++
    if(self.current_working_copy(), "@", "-") ++ " " ++
    if(self.immutable(), "i", "-") ++ " " ++
    if(self.conflict(), "x", "-") ++ " " ++
    if(self.current_working_copy(),
      self.diff().stat().files().map(|f| f.lines_added() ++ ":" ++ f.lines_removed()).join(","),
      "") ++ " " ++
    bookmarks.join(",") ++ "\n""#;

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Parse one line of `jj log` output produced by [`JJ_LOG_TEMPLATE`]:
/// id parents workingcopy immutable conflict file_stats bookmarks
fn parse_line(line: &str) -> Option<Change> {
    let items: Vec<&str> = line.split(' ').collect();
    let [
        id,
        parents,
        working_copy,
        immutable,
        conflict,
        file_stats,
        bookmarks,
    ] = items[..]
    else {
        return None;
    };

    let mut files_modified = 0;
    let mut lines_added = 0;
    let mut lines_removed = 0;
    for file_stat in file_stats.split(',').filter(|s| !s.is_empty()) {
        let (added, removed) = file_stat.split_once(':').unwrap_or((file_stat, ""));
        files_modified += 1;
        lines_added += added.parse().unwrap_or(0);
        lines_removed += removed.parse().unwrap_or(0);
    }

    Some(Change {
        id: id.to_string(),
        parents: split_list(parents),
        is_working_copy: working_copy == "@",
        is_immutable: immutable == "i",
        has_conflict: conflict == "x",
        files_modified,
        lines_added,
        lines_removed,
        bookmarks: split_list(bookmarks),
        children: Vec::new(),
        distance: 0,
    })
}

/// The changes of the log revset linked to their parents and children.
struct Graph {
    changes: HashMap<String, Change>,
    /// change ids in log order
    order: Vec<String>,
    working_copy_id: Option<String>,
}

impl Graph {
    fn new(changes: impl IntoIterator<Item = Change>) -> Self {
        let mut graph = Graph {
            changes: HashMap::new(),
            order: Vec::new(),
            working_copy_id: None,
        };
        for change in changes {
            if change.is_working_copy && graph.working_copy_id.is_none() {
                graph.working_copy_id = Some(change.id.clone());
            }
            graph.order.push(change.id.clone());
            graph.changes.insert(change.id.clone(), change);
        }

        // add children to their respective parents
        let edges: Vec<(String, String)> = graph
            .changes
            .values()
            .flat_map(|child| {
                child
                    .parents
                    .iter()
                    .map(|parent_id| (parent_id.clone(), child.id.clone()))
            })
            .collect();
        for (parent_id, child_id) in edges {
            if let Some(parent) = graph.changes.get_mut(&parent_id) {
                parent.children.push(child_id);
            }
        }
        graph
    }

    fn get(&self, id: &Option<String>) -> Option<&Change> {
        self.changes.get(id.as_deref()?)
    }

    fn working_copy(&self) -> Option<&Change> {
        self.get(&self.working_copy_id)
    }

    /// Breadth-first search from `start` following `next` (parents or children),
    /// setting the distance of each reached change and calling `visit` on it.
    /// The first visit of a change is via a shortest path, so each change is
    /// visited only once (otherwise merges would cause exponential revisits).
    fn walk(
        &mut self,
        start: &str,
        next: fn(&Change) -> &[String],
        mut visit: impl FnMut(&Change),
    ) {
        let mut queue = VecDeque::from([(start.to_string(), 0)]);
        let mut visited = HashSet::new();
        while let Some((id, distance)) = queue.pop_front() {
            if !visited.insert(id.clone()) {
                continue;
            }
            let Some(change) = self.changes.get_mut(&id) else {
                continue;
            };
            change.distance = distance;
            visit(change);
            queue.extend(next(change).iter().map(|id| (id.clone(), distance + 1)));
        }
    }
}

/// Everything the prompt shows besides the working copy itself.
#[derive(Debug, Default)]
struct State {
    conflict_count: usize,
    bookmark_count: usize,
    branch_count: usize,
    merge_count: usize,
    main_id: Option<String>,
    first_conflict_id: Option<String>,
    prev_conflict_id: Option<String>,
    next_conflict_id: Option<String>,
    last_conflict_id: Option<String>,
    prev_branch_id: Option<String>,
    next_branch_id: Option<String>,
    last_change_id: Option<String>,
}

/// Collect counts and walk the graph from the working copy, which also sets the
/// distance of every change related to it.
fn analyze(graph: &mut Graph) -> State {
    let mut state = State::default();

    for change in graph.changes.values() {
        state.bookmark_count += change.bookmarks.len();
        if change.has_conflict {
            state.conflict_count += 1;
        }
        if change.children.len() > 1 {
            state.branch_count += change.children.len();
        }
        if change.parents.len() > 1 {
            state.merge_count += 1;
        }
    }

    let Some(working_copy_id) = graph.working_copy_id.clone() else {
        return state;
    };

    // backward search through the ancestors
    graph.walk(
        &working_copy_id,
        |change| &change.parents,
        |change| {
            if change.is_immutable && state.main_id.is_none() {
                state.main_id = Some(change.id.clone());
            }
            if change.id != working_copy_id {
                if change.has_conflict {
                    state
                        .prev_conflict_id
                        .get_or_insert_with(|| change.id.clone());
                    state.first_conflict_id = Some(change.id.clone());
                }
                if !change.bookmarks.is_empty() {
                    state
                        .prev_branch_id
                        .get_or_insert_with(|| change.id.clone());
                }
            }
        },
    );

    // no conflicting ancestor, so take the first conflict in log order
    if state.conflict_count > 0 && state.first_conflict_id.is_none() {
        state.first_conflict_id = graph
            .order
            .iter()
            .find(|id| {
                graph
                    .changes
                    .get(*id)
                    .is_some_and(|change| change.has_conflict)
            })
            .cloned();
    }

    // forward search through the descendants
    graph.walk(
        &working_copy_id,
        |change| &change.children,
        |change| {
            if change.id != working_copy_id {
                state.last_change_id = Some(change.id.clone());
                if change.has_conflict {
                    state
                        .next_conflict_id
                        .get_or_insert_with(|| change.id.clone());
                    state.last_conflict_id = Some(change.id.clone());
                }
            }
            if !change.bookmarks.is_empty() {
                state
                    .next_branch_id
                    .get_or_insert_with(|| change.id.clone());
            }
        },
    );

    state
}

fn is(id: &Option<String>, change: &Change) -> bool {
    id.as_deref() == Some(change.id.as_str())
}

/// Render the given sections, e.g.
/// [×NEXT_CONFLICT_ID ][FILES+LINES-LINES ]@WORKING_ID [⇡/⇣/→CURRENT_BRANCH ][⇣NEXT_BRANCH ]MAIN_POSITION⇡◆
/// other symbols ⌃⌄▴▾▲▼ - see https://en.wikipedia.org/wiki/Geometric_Shapes_(Unicode_block)
/// https://en.wikipedia.org/wiki/Arrows_(Unicode_block)
///
/// Returns `None` if the working copy is not part of the log revset.
fn render(sections: &[OutputSection], graph: &Graph, state: &State) -> Option<String> {
    let working_copy = graph.working_copy()?;
    // main may be missing if no immutable ancestor is in the log revset
    let main = graph.get(&state.main_id);

    let mut output = String::new();
    for section in sections {
        let fragment = match section {
            OutputSection::WorkingCopy if working_copy.has_conflict => {
                format!("{RED}@{MAGENTA}{}", working_copy.id)
            }
            OutputSection::WorkingCopy if working_copy.is_immutable => {
                format!("{BLUE}@{MAGENTA}{}", working_copy.id)
            }
            OutputSection::WorkingCopy => format!("{GREEN}@{MAGENTA}{}", working_copy.id),
            OutputSection::Stats if working_copy.files_modified > 0 => format!(
                "{RESET}{}{GREEN}+{}{RED}-{}{RESET}",
                working_copy.files_modified, working_copy.lines_added, working_copy.lines_removed
            ),
            OutputSection::Stats => String::new(),
            OutputSection::Conflicts => render_conflicts(graph, state, working_copy),
            OutputSection::Branches => {
                let previous = match graph.get(&state.prev_branch_id) {
                    Some(prev_branch) if main.is_none_or(|main| prev_branch.id != main.id) => {
                        format!(
                            " {RESET}{}⇡{MAGENTA}{}",
                            prev_branch.distance, prev_branch.bookmarks[0]
                        )
                    }
                    _ => String::new(),
                };
                let next = match graph.get(&state.next_branch_id) {
                    Some(next_branch) => format!(
                        " {RESET}{}{MAGENTA}{}",
                        if next_branch.id == working_copy.id {
                            "→".to_string()
                        } else {
                            format!("{}⇣", next_branch.distance)
                        },
                        next_branch.bookmarks[0]
                    ),
                    None => String::new(),
                };
                format!("{previous}{next}")
            }
            OutputSection::Main => match main {
                Some(main) if main.distance > 0 => format!(
                    "{RESET}{}⤒{BLUE}◆{}",
                    main.distance,
                    match main.bookmarks.first() {
                        Some(bookmark) => format!("{MAGENTA}{bookmark}"),
                        None => String::new(),
                    }
                ),
                _ => String::new(),
            },
            OutputSection::LastChange => match graph.get(&state.last_change_id) {
                Some(last_change) => format!(
                    "{RESET}{}⤓{MAGENTA}{}",
                    last_change.distance, last_change.id
                ),
                None => String::new(),
            },
            OutputSection::Counts => {
                let mut counts = String::new();
                if graph.changes.len() > 2 {
                    counts += &format!(" {RESET}{}○", graph.changes.len() - 1);
                }
                if state.branch_count > 0 {
                    counts += &format!(" {RESET}{}{BLUE}", state.branch_count);
                }
                if state.merge_count > 0 {
                    counts += &format!(" {RESET}{}{BLUE}", state.merge_count);
                }
                if state.bookmark_count > 1 {
                    counts += &format!(" {RESET}{}{MAGENTA}", state.bookmark_count);
                }
                counts
            }
        };
        append_section(&mut output, &fragment);
    }
    Some(output)
}

fn render_conflicts(graph: &Graph, state: &State, working_copy: &Change) -> String {
    if state.conflict_count == 0 {
        return String::new();
    }

    let mut conflicts = String::new();
    if state.conflict_count > 3 {
        conflicts += &format!("{RESET}{}{RED}×", state.conflict_count);
    }
    if let Some(first) = graph.get(&state.first_conflict_id)
        && first.id != working_copy.id
        && !is(&state.prev_conflict_id, first)
        && !is(&state.next_conflict_id, first)
        && !is(&state.last_conflict_id, first)
    {
        let distance = match first.distance {
            0 => String::new(),
            d => d.to_string(),
        };
        conflicts += &format!(" {RESET}{distance}⤒{RED}×{MAGENTA}{}", first.id);
    }
    if let Some(prev) = graph.get(&state.prev_conflict_id) {
        conflicts += &format!(" {RESET}{}⇡{RED}×{MAGENTA}{}", prev.distance, prev.id);
    }
    if let Some(next) = graph.get(&state.next_conflict_id)
        && next.id != working_copy.id
    {
        conflicts += &format!(" {RESET}{}⇣{RED}×{MAGENTA}{}", next.distance, next.id);
    }
    if let Some(last) = graph.get(&state.last_conflict_id)
        && last.id != working_copy.id
        && !is(&state.next_conflict_id, last)
    {
        let distance = match last.distance {
            0 => String::new(),
            d => d.to_string(),
        };
        conflicts += &format!(" {RESET}{distance}⤓{RED}×{MAGENTA}{}", last.id);
    }

    // all parts may be empty, e.g. when @ is the only conflict
    if !conflicts.is_empty() {
        conflicts += RESET;
    }
    conflicts
}

fn main() -> io::Result<ExitCode> {
    let args = Args::parse();

    let mut cmd = Command::new("jj");
    cmd.args(["log", "--ignore-working-copy", "--reversed", "--no-graph"])
        .args(["--color", "never", "-T", JJ_LOG_TEMPLATE])
        .stdout(Stdio::piped());

    // a prompt must not print jj errors, e.g. when not inside a jj repo
    if !args.test {
        cmd.stderr(Stdio::null());
    }

    if args.test {
        println!("Running jj log with command: {:?}", cmd);
    }

    let jj_output = cmd.output()?;
    if !jj_output.status.success() {
        return Ok(ExitCode::FAILURE);
    }
    let input = String::from_utf8_lossy(&jj_output.stdout);

    let changes = input.lines().filter_map(|line| {
        let change = parse_line(line);
        if change.is_none() && args.test {
            println!("skipping unexpected line: {line:?}");
        }
        change
    });
    let mut graph = Graph::new(changes);
    let state = analyze(&mut graph);

    if args.test {
        println!("change_count: {}", graph.changes.len());
        println!("working_copy_id: {:?}", graph.working_copy_id);
        println!("{state:#?}");
    }

    if let Some(output) = render(&args.sections(), &graph, &state) {
        writeln!(
            io::stdout(),
            "{}",
            args.shell.format(&format!("{output}{RESET}"))
        )?;
    }

    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{
        Args, Graph, OutputSection, Shell, State, analyze, append_section, parse_line, render,
    };

    /// Build and analyze a graph from `jj log` output lines.
    fn graph(lines: &[&str]) -> (Graph, State) {
        let mut graph = Graph::new(lines.iter().map(|line| parse_line(line).unwrap()));
        let state = analyze(&mut graph);
        (graph, state)
    }

    fn strip_ansi(text: &str) -> String {
        let mut result = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                chars.by_ref().find(|&c| c == 'm');
            } else {
                result.push(c);
            }
        }
        result
    }

    fn render_all(graph: &Graph, state: &State) -> Option<String> {
        let args = Args::try_parse_from(["jj-prompt"]).unwrap();
        render(&args.sections(), graph, state).map(|output| strip_ansi(&output))
    }

    // log order (oldest first): root <- ma (main) <- a (feat, conflict) <- @b <- c
    const LINEAR: &[&str] = &[
        "zz  - i -  ",
        "ma zz - i -  main",
        "a ma - - x  feat",
        "b a @ - - 1:0,2:3 ",
        "c b - - -  ",
    ];

    const COLORED: &str = "\x1b[32m@\x1b[35mab 50%\x1b[0m";

    #[test]
    fn plain_and_fish_output_is_unchanged() {
        assert_eq!(Shell::Plain.format(COLORED), COLORED);
        assert_eq!(Shell::Fish.format(COLORED), COLORED);
    }

    #[test]
    fn bash_wraps_color_codes_in_readline_markers() {
        assert_eq!(
            Shell::Bash.format(COLORED),
            "\x01\x1b[32m\x02@\x01\x1b[35m\x02ab 50%\x01\x1b[0m\x02"
        );
    }

    #[test]
    fn zsh_wraps_color_codes_and_escapes_percent() {
        assert_eq!(
            Shell::Zsh.format(COLORED),
            "%{\x1b[32m%}@%{\x1b[35m%}ab 50%%%{\x1b[0m%}"
        );
    }

    #[test]
    fn shell_defaults_to_plain() {
        let args = Args::try_parse_from(["jj-prompt"]).unwrap();
        assert_eq!(args.shell, Shell::Plain);
        let args = Args::try_parse_from(["jj-prompt", "--shell", "zsh"]).unwrap();
        assert_eq!(args.shell, Shell::Zsh);
    }

    #[test]
    fn parse_line_reads_fields_and_sums_file_stats() {
        let change = parse_line("b a,c @ i x 1:0,2:3 main,feat").unwrap();

        assert_eq!(change.id, "b");
        assert_eq!(change.parents, vec!["a", "c"]);
        assert!(change.is_working_copy && change.is_immutable && change.has_conflict);
        assert_eq!(
            (
                change.files_modified,
                change.lines_added,
                change.lines_removed
            ),
            (2, 3, 3)
        );
        assert_eq!(change.bookmarks, vec!["main", "feat"]);
    }

    #[test]
    fn parse_line_rejects_malformed_lines() {
        assert!(parse_line("").is_none());
        assert!(parse_line("b a @ - -").is_none());
        assert!(parse_line("Error: something went wrong").is_none());
    }

    #[test]
    fn linear_history_renders_all_sections() {
        let (graph, state) = graph(LINEAR);

        assert_eq!(state.main_id.as_deref(), Some("ma"));
        assert_eq!(state.prev_conflict_id.as_deref(), Some("a"));
        assert_eq!(state.prev_branch_id.as_deref(), Some("a"));
        assert_eq!(state.next_branch_id, None);
        assert_eq!(state.last_change_id.as_deref(), Some("c"));
        assert_eq!(
            render_all(&graph, &state).unwrap(),
            "@b 2+3-3 1⇡×a 1⇡feat 2⤒◆main 1⤓c 4○ 2\u{f461}"
        );
    }

    #[test]
    fn distances_use_shortest_path_through_merges() {
        // r <- a <- b, c <- d (merge of b and c) <- @e
        let (graph, state) = graph(&[
            "r  - i -  ",
            "a r - - -  ",
            "b a - - -  ",
            "c a - - -  ",
            "d b,c - - -  ",
            "e d @ - -  ",
        ]);

        let distance = |id: &str| graph.changes[id].distance;
        assert_eq!(
            ["e", "d", "b", "c", "a", "r"].map(distance),
            [0, 1, 2, 2, 3, 4]
        );
        assert_eq!(state.main_id.as_deref(), Some("r"));
        assert_eq!((state.branch_count, state.merge_count), (2, 1));
    }

    #[test]
    fn conflicts_section_is_empty_when_only_working_copy_conflicts() {
        let (graph, state) = graph(&["r  - i -  ", "a r @ - x  "]);
        let sections = [OutputSection::Conflicts];

        assert_eq!(state.conflict_count, 1);
        assert_eq!(render(&sections, &graph, &state).unwrap(), "");
    }

    #[test]
    fn single_descendant_conflict_is_shown_as_next() {
        let (graph, state) = graph(&["r  - i -  ", "a r @ - -  ", "b a - - x  "]);
        let sections = [OutputSection::Conflicts];

        let output = render(&sections, &graph, &state).unwrap();
        assert_eq!(strip_ansi(&output), "1⇣×b");
    }

    #[test]
    fn descendant_conflicts_are_shown_as_next_and_last() {
        let (graph, state) = graph(&["r  - i -  ", "a r @ - -  ", "b a - - x  ", "c b - - x  "]);
        let sections = [OutputSection::Conflicts];

        let output = render(&sections, &graph, &state).unwrap();
        assert_eq!(strip_ansi(&output), "1⇣×b 2⤓×c");
    }

    #[test]
    fn nothing_is_rendered_without_working_copy() {
        let (graph, state) = graph(&["r  - i -  ", "a r - - -  "]);

        assert_eq!(render_all(&graph, &state), None);
    }

    #[test]
    fn missing_main_omits_main_section() {
        let (graph, state) = graph(&["a  @ - -  "]);

        assert_eq!(state.main_id, None);
        assert_eq!(render_all(&graph, &state).unwrap(), "@a");
    }

    #[test]
    fn sections_are_joined_with_single_spaces() {
        let mut output = String::new();
        append_section(&mut output, "  @id");
        append_section(&mut output, "");
        append_section(&mut output, "  stats  ");
        append_section(&mut output, " conflicts");

        assert_eq!(output, "@id stats conflicts");
    }

    #[test]
    fn no_section_selection_shows_everything() {
        let args = Args::try_parse_from(["jj-prompt"]).unwrap();

        assert_eq!(
            args.sections(),
            vec![
                OutputSection::WorkingCopy,
                OutputSection::Stats,
                OutputSection::Conflicts,
                OutputSection::Branches,
                OutputSection::Main,
                OutputSection::LastChange,
                OutputSection::Counts,
            ]
        );
    }

    #[test]
    fn repeated_section_arguments_select_only_those_sections() {
        let args = Args::try_parse_from([
            "jj-prompt",
            "--show",
            "conflicts",
            "-s",
            "last-change",
            "-s",
            "@",
        ])
        .unwrap();

        assert_eq!(
            args.sections(),
            vec![
                OutputSection::Conflicts,
                OutputSection::LastChange,
                OutputSection::WorkingCopy
            ]
        );
    }

    #[test]
    fn section_selection_preserves_order_and_ignores_duplicates() {
        let args = Args::try_parse_from([
            "jj-prompt",
            "--show",
            "branches",
            "--show",
            "stats",
            "--show",
            "branches",
        ])
        .unwrap();

        assert_eq!(
            args.sections(),
            vec![OutputSection::Branches, OutputSection::Stats]
        );
    }

    #[test]
    fn unique_section_prefix_is_accepted() {
        let args = Args::try_parse_from(["jj-prompt", "-s", "m"]).unwrap();

        assert_eq!(args.sections(), vec![OutputSection::Main]);
    }

    #[test]
    fn ambiguous_section_prefix_is_rejected() {
        let result = Args::try_parse_from(["jj-prompt", "-s", "c"]);

        let error = match result {
            Ok(_) => panic!("ambiguous prefixes should be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("ambiguous section prefix"));
    }
}
