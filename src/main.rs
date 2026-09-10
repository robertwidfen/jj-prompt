use std::collections::{HashMap, VecDeque};
use std::env;
use std::io::{self, BufRead, Write};
use std::process::{Command, Stdio};

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

fn main() -> io::Result<()> {
    let mut args: Vec<String> = env::args().skip(1).collect();

    let test = args.iter().any(|arg| arg == "-t");
    args.retain(|arg| arg != "-t");

    let mut cmd = Command::new("jj");
    cmd.arg("log")
            .arg("--ignore-working-copy")
            .arg("--reversed")
            .arg("--no-graph")
            .arg("--color")
            .arg("never")
            .arg("-T")
            .arg(
                r#"change_id.shortest() ++ " " ++
                parents.map(|p| p.change_id().shortest()).join(",") ++ " " ++
                if(self.current_working_copy(), "@", "-") ++ " " ++
                if(self.immutable(), "i", "-") ++ " " ++ 
                if(self.conflict(), "x", "-") ++ " " ++ 
                if(self.current_working_copy(), 
                  self.diff().files().len() ++ " " ++ self.diff().stat().total_added() ++ " " ++ self.diff().stat().total_removed(), 
                  "0 0 0") ++ " " ++ 
                bookmarks.join(",") ++ "\n""#,
            )
            .stdout(Stdio::piped());
    if test {
        println!("Running jj log with command: {:?}", cmd);
    }
    let mut process = cmd.spawn()?;
    let process_stdout = process
        .stdout
        .take()
        .expect("jj log stdout should be piped");
    let jj_process = Some(process);
    let input: Box<dyn BufRead> = { Box::new(io::BufReader::new(process_stdout)) };

    // build a graph of changes based on parents
    type Id = String;
    let mut graph: HashMap<Id, Change> = HashMap::new();
    let mut changes = Vec::new();
    let mut children_to_add = Vec::new();
    let mut main_id = String::new();
    let mut working_copy_id = String::new();
    let mut conflict_count = 0;
    let mut bookmark_count = 0;

    for line in input.lines().map_while(Result::ok) {
        // id parents workingcopy immutabled conflict files_modified lines_added lines_removed branches
        let items: Vec<&str> = line.split(" ").collect();

        let id = items[0];
        let parents: Vec<String> = items[1]
            .split(",")
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        let is_working_copy = items[2] == "@";
        let is_immutable = items[3] == "i";
        let has_conflict = items[4] == "x";
        let files_modified = items[5].parse().unwrap_or(0);
        let lines_added = items[6].parse().unwrap_or(0);
        let lines_removed = items[7].parse().unwrap_or(0);
        let bookmarks: Vec<String> = items[8]
            .split(",")
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();

        // changes in log order
        changes.push(id.to_string());

        for parent_id in &parents {
            children_to_add.push((parent_id.clone(), id.to_string()));
        }

        bookmark_count += bookmarks.len();

        if is_working_copy && working_copy_id.is_empty() {
            working_copy_id = id.to_string();
        }
        if has_conflict {
            conflict_count += 1;
        }

        graph.insert(
            id.to_string(),
            Change {
                id: id.to_string(),
                parents: parents,
                is_working_copy,
                is_immutable,
                has_conflict,
                files_modified,
                lines_added,
                lines_removed,
                bookmarks,
                children: Vec::new(),
                distance: 0,
            },
        );
    }

    // add children to their respective parents in graph
    for (parent_id, child_id) in children_to_add {
        if let Some(parent_change) = graph.get_mut(&parent_id) {
            parent_change.children.push(child_id.clone());
        }
    }

    // collect branch and merge counts
    let mut branch_count = 0;
    let mut merge_count = 0;
    for (_, change) in &graph {
        if change.children.len() > 1 {
            branch_count += change.children.len() - 1;
        }
        if change.parents.len() > 1 {
            merge_count += change.parents.len() - 1;
        }
    }

    // backward graph search and distance calculation starting from the working copy
    let mut queue = VecDeque::new();

    let mut first_conflict_id = String::new();
    let mut prev_conflict_id = String::new();
    let mut prev_branch_id = String::new();
    queue.push_back((working_copy_id.clone(), 0));
    while let Some((current_id, distance)) = queue.pop_front() {
        if let Some(current_node) = graph.get_mut(&current_id) {
            if current_node.distance == 0 {
                current_node.distance = distance;
            } else {
                current_node.distance = distance.min(current_node.distance);
            }
            if current_node.is_immutable && main_id.is_empty() {
                main_id = current_id.clone();
            }
            if current_id != working_copy_id {
                if current_node.has_conflict {
                    if prev_conflict_id.is_empty() {
                        prev_conflict_id = current_id.clone();
                    }
                    first_conflict_id = current_id.clone();
                }
                if !current_node.bookmarks.is_empty() && prev_branch_id.is_empty() {
                    prev_branch_id = current_id.clone();
                }
            }
            for parent_id in &current_node.parents {
                queue.push_back((parent_id.clone(), distance + 1));
            }
        }
    }

    if conflict_count > 0 && first_conflict_id.is_empty() {
        for id in &changes {
            if let Some(node) = graph.get(id) {
                if node.has_conflict {
                    first_conflict_id = id.clone();
                    break;
                }
            }
        }
    }

    // foreward graph search and distance calculation starting from the working copy
    let mut next_conflict_id = String::new();
    let mut next_branch_id = String::new();
    let mut last_conflict_id = String::new();
    let mut last_change_id = String::new();
    queue.clear();
    queue.push_back((working_copy_id.clone(), 0));
    while let Some((current_id, distance)) = queue.pop_front() {
        if let Some(current_node) = graph.get_mut(&current_id) {
            if current_node.distance == 0 {
                current_node.distance = distance;
            } else {
                current_node.distance = distance.min(current_node.distance);
            }
            if current_id != working_copy_id {
                last_change_id = current_id.clone();
                if current_node.has_conflict {
                    if next_conflict_id.is_empty() {
                        next_conflict_id = current_id.clone();
                    }
                    last_conflict_id = current_id.clone();
                }
                if !current_node.bookmarks.is_empty() && next_branch_id.is_empty() {
                    next_branch_id = current_id.clone();
                }
            }
            for child_id in &current_node.children {
                queue.push_back((child_id.clone(), distance + 1));
            }
        }
    }

    if test {
        println!("change_count: {}", changes.len());
        println!("conflict_count: {}", conflict_count);
        println!("main_id: {}", main_id);
        println!("working_copy_id: {}", working_copy_id);
        println!("first_conflict_id: {}", first_conflict_id);
        println!("prev_conflict_id: {}", prev_conflict_id);
        println!("next_conflict_id: {}", next_conflict_id);
        println!("last_conflict_id: {}", last_conflict_id);
        println!("prev_branch_id: {}", prev_branch_id);
        println!("next_branch_id: {}", next_branch_id);
    }

    // generate the prompt, e.g.
    // [×NEXT_CONFLICT_ID ][FILES+LINES-LINES ]@WORKING_ID [⇡/⇣/→CURRENT_BRANCH ][⇣NEXT_BRANCH ]MAIN_POSITION⇡◆
    // other symbols ⌃⌄▴▾▲▼ - see https://en.wikipedia.org/wiki/Geometric_Shapes_(Unicode_block)
    // https://en.wikipedia.org/wiki/Arrows_(Unicode_block)

    if !changes.is_empty() {
        let main = graph.get(&main_id).unwrap();
        let working_copy = graph.get(&working_copy_id).unwrap();

        writeln!(
            &mut std::io::stdout(),
            "{}{}{}{}{}{}{}{}{}{}{RESET}{}",
            // @ indicator and ID
            if working_copy.has_conflict {
                format!("{RED}@{MAGENTA}{}", working_copy.id)
            } else if working_copy.is_immutable {
                format!("{BLUE}@{MAGENTA}{}", working_copy.id)
            } else {
                format!("{GREEN}@{MAGENTA}{}", working_copy.id)
            },
            // working copy files modified and lines changed indicator
            if working_copy.files_modified > 0 {
                format!(
                    " {RESET}{}{GREEN}+{}{RED}-{}{RESET}",
                    working_copy.files_modified,
                    working_copy.lines_added,
                    working_copy.lines_removed
                )
            } else {
                // format!("{GREEN}◌{RESET} ")
                "".to_string()
            },
            // conflict indicator
            if conflict_count > 0 {
                format!(
                    " {}{}{}{}{}{RESET}",
                    // maybe use larger x, X or ❌️ than ×?
                    if conflict_count > 3 {
                        format!("{RESET}{}{RED}×", conflict_count)
                    } else {
                        "".to_string()
                    },
                    if let Some(first_conflict) = graph.get(&first_conflict_id)
                        && first_conflict_id != working_copy.id
                        && first_conflict_id != prev_conflict_id
                    {
                        let distance = match first_conflict.distance {
                            0 => "".to_string(),
                            d => d.to_string(),
                        };
                        format!(" {RESET}{}⤒{RED}×{MAGENTA}{}", distance, first_conflict.id)
                    } else {
                        "".to_string()
                    },
                    if let Some(prev_conflict) = graph.get(&prev_conflict_id)
                        && prev_conflict_id != working_copy_id
                        && prev_conflict_id != first_conflict_id
                    {
                        format!(
                            " {RESET}{}⇡{RED}×{MAGENTA}{}",
                            prev_conflict.distance, prev_conflict.id
                        )
                    } else {
                        "".to_string()
                    },
                    if let Some(next_conflict) = graph.get(&next_conflict_id)
                        && next_conflict_id != working_copy.id
                        && next_conflict_id != last_conflict_id
                    {
                        format!(
                            " {RESET}{}⇣{RED}×{MAGENTA}{}",
                            next_conflict.distance, next_conflict.id
                        )
                    } else {
                        "".to_string()
                    },
                    if let Some(last_conflict) = graph.get(&last_conflict_id)
                        && last_conflict_id != working_copy_id
                        && last_conflict_id != next_conflict_id
                    {
                        let distance = match last_conflict.distance {
                            0 => "".to_string(),
                            d => d.to_string(),
                        };
                        format!(" {RESET}{}⤓{RED}×{MAGENTA}{}", distance, last_conflict.id)
                    } else {
                        "".to_string()
                    },
                )
            } else {
                "".to_string()
            },
            // current branch indicator
            if let Some(prev_branch) = graph.get(&prev_branch_id)
                && prev_branch.id != main.id
            {
                format!(
                    " {RESET}{}{MAGENTA}{}",
                    if prev_branch.id == working_copy.id {
                        "→".to_string()
                    } else {
                        format!("{}⇡", prev_branch.distance)
                    },
                    prev_branch.bookmarks[0]
                )
            } else {
                "".to_string()
            },
            // next branch indicator
            if let Some(next_branch) = graph.get(&next_branch_id) {
                format!(
                    " {RESET}{}{MAGENTA}{}",
                    if next_branch_id == working_copy.id {
                        "→".to_string()
                    } else {
                        format!("{}⇣", next_branch.distance)
                    },
                    next_branch.bookmarks[0]
                )
            } else {
                "".to_string()
            },
            // immutable (main) position indicator
            if main.distance > 0 {
                format!(
                    " {RESET}{}⤒{BLUE}◆{}",
                    main.distance,
                    if !main.bookmarks.is_empty() {
                        format!("{MAGENTA}{}", main.bookmarks[0])
                    } else {
                        "".to_string()
                    }
                )
            } else {
                "".to_string()
            },
            // last change indicator
            if let Some(last_change) = graph.get(&last_change_id) {
                format!(
                    " {RESET}{}⤓{RED}{MAGENTA}{}",
                    last_change.distance, last_change.id
                )
            } else {
                "".to_string()
            },
            if changes.len() <= 2 {
                "".to_string()
            } else {
                format!(" {RESET}{}○", changes.len() - 1)
            },
            if branch_count > 0 {
                format!(" {RESET}{}{BLUE}", branch_count)
            } else {
                "".to_string()
            },
            if merge_count > 0 {
                format!(" {RESET}{}{BLUE}", merge_count)
            } else {
                "".to_string()
            },
            if bookmark_count <= 1 {
                "".to_string()
            } else {
                format!(" {RESET}{}{MAGENTA}", bookmark_count)
            },
        )?;
    }

    if let Some(mut process) = jj_process {
        process.wait()?;
    }

    Ok(())
}
