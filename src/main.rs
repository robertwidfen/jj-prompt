use regex::Regex;
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

// // make background to go to end of line
// const CLEAR_LINE: &str = "\x1b[K";

// prompt = '''
// change_id.shortest() ++ format_bookmarks ++ "\n"
// '''

//   jj log --ignore-working-copy --reversed --color never -T 'change_id.shortest() ++ " " ++
//       self.diff().files().len() ++ " " ++
//       self.diff().stat().total_added() ++ " " ++
//       self.diff().stat().total_removed() ++ " " ++
//       format_bookmarks ++
//       "\\n"'
// ◆  z 0 0 0
// ○    op 5 323 0  main
// ├─╮
// │ ○  r 1 1 0
// @  s 1 18 37  branch
// ×  ot 1 8 1

enum OutputTarget {
    Stdout(io::Stdout),
    Pager { child: std::process::Child },
}

impl Write for OutputTarget {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            OutputTarget::Stdout(stdout) => stdout.write(buf),
            OutputTarget::Pager { child } => child.stdin.as_mut().unwrap().write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            OutputTarget::Stdout(stdout) => stdout.flush(),
            OutputTarget::Pager { child } => child.stdin.as_mut().unwrap().flush(),
        }
    }
}

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();

    let read_from_stdin = args.first().is_some_and(|arg| arg == "-");
    let pager_program = if read_from_stdin {
        args.get(1).cloned()
    } else if !args.is_empty() {
        args.first().cloned()
    } else {
        None
    };
    let pager_args = if read_from_stdin {
        args.get(2..).unwrap_or_default().to_vec()
    } else {
        args.get(1..).unwrap_or_default().to_vec()
    };

    let mut fd = if let Some(pager_program) = pager_program {
        let mut process = Command::new(pager_program)
            .args(&pager_args)
            .stdin(Stdio::piped())
            .spawn()?;
        let _ = process.stdin.take();
        OutputTarget::Pager { child: process }
    } else {
        OutputTarget::Stdout(io::stdout())
    };

    let mut jj_process = None;
    let input: Box<dyn BufRead> = if read_from_stdin {
        Box::new(io::stdin().lock())
    } else {
        let mut cmd = Command::new("jj");
        cmd.arg("log")
            .arg("--ignore-working-copy")
            .arg("--reversed")
            .arg("--color")
            .arg("never")
            .arg("-T")
            .arg(
                "change_id.shortest() ++ \" \" ++ \
                if(self.current_working_copy(),
                  self.diff().files().len() ++ \" \" ++ \
                  self.diff().stat().total_added() ++ \" \" ++ \
                  self.diff().stat().total_removed(),\
                  \"0 0 0\") ++ \" \" ++ \
                format_bookmarks ++ \"\\n\"",
            )
            .stdout(Stdio::piped());
        let mut process = cmd.spawn()?;
        let stdout = process
            .stdout
            .take()
            .expect("jj log stdout should be piped");
        jj_process = Some(process);
        Box::new(io::BufReader::new(stdout))
    };

    let mut line_count = 0;
    let mut _normal_count = 0;
    let mut confict_count = 0;
    let mut main_position = 0;

    let mut at_position = 0;
    let mut at_id = String::new();
    let mut at_current_branch = String::new();
    let mut at_current_branch_pos = 0;
    let mut at_next_branch = String::new();
    let mut at_next_branch_pos = 0;
    let mut files_changed = 0;
    let mut lines_added = 0;
    let mut lines_removed = 0;

    let mut all_branches: Vec<(i32, String)> = Vec::new();
    let mut frist_conflict_id = String::new();

    let re_change = Regex::new(r"^[│ ]*([^ ]+) +([a-z]+) (\d+) (\d+) (\d+)(  ([^ ,].*))?").unwrap();

    for line in input.lines().map_while(Result::ok) {
        if line.starts_with("├") || line.starts_with("│ ") {
            continue;
        }

        if let Some(captures) = re_change.captures(&line) {
            line_count += 1;
            let mark = captures.get(1).map_or("", |m| m.as_str());
            let id = captures.get(2).map_or("", |m| m.as_str());
            // writeln!(fd, " '{}' '{}'", mark, id)?;
            let branches = captures.get(7).map_or("", |m| m.as_str());
            if !branches.is_empty() {
                // println!("Branches: '{}'", branches);
                let branches: Vec<&str> = branches.split(", ").collect();
                if at_position == 0 {
                    at_current_branch = branches[0].to_string();
                    at_current_branch_pos = line_count;
                } else if at_position > 0 && at_next_branch.is_empty() {
                    at_next_branch = branches[0].to_string();
                    at_next_branch_pos = line_count;
                }
                for b in branches {
                    // println!("Branch: '{}'", b);
                    all_branches.push((line_count, b.to_string()));
                }
            }
            match mark {
                "@" => {
                    at_position = line_count;
                    files_changed = captures
                        .get(3)
                        .map_or("", |m| m.as_str())
                        .parse()
                        .unwrap_or(0);
                    lines_added = captures
                        .get(4)
                        .map_or("", |m| m.as_str())
                        .parse()
                        .unwrap_or(0);
                    lines_removed = captures
                        .get(5)
                        .map_or("", |m| m.as_str())
                        .parse()
                        .unwrap_or(0);
                    at_id = id.to_string();
                }
                "◆" => {
                    main_position = line_count;
                }
                "○" => _normal_count += 1,
                "×" => {
                    confict_count += 1;
                    if frist_conflict_id.is_empty() {
                        frist_conflict_id = id.to_string();
                    }
                }
                _ => {
                    eprintln!("Unknown mark: '{}'", mark);
                }
            }
        } else {
            writeln!(fd, "{RED}ERROR: '{}'{RESET}", line)?;
        }
    }

    // generate the prompt
    // [×NEXT_CONFLICT_ID ][FILES+LINES-LINES ]@WIRKING_ID
    // [⇡/⇣/→CURRENT_BRANCH ][⇣NEXT_BRANCH ]MAIN_POSITION⇡◆
    if line_count > 0 {
        writeln!(
            &mut fd,
            "{}{}{GREEN}@{MAGENTA}{} {}{}{RESET}{}⇡{BLUE}◆ {BLUE}{}{RESET}",
            if confict_count > 0 {
                format!("{RED}{}×{} ", confict_count, frist_conflict_id)
            } else {
                "".to_string()
            },
            if files_changed > 0 {
                format!(
                    " {RESET}{}{GREEN}+{}{RED}-{}{RESET} ",
                    files_changed, lines_added, lines_removed
                )
            } else {
                "".to_string()
            },
            at_id,
            if at_current_branch.is_empty() {
                " ".to_string()
            } else {
                format!(
                    "{RESET}{}{BLUE}{} ",
                    if at_current_branch_pos == at_position {
                        "→".to_string()
                    } else {
                        format!("{}⇡", at_position - at_current_branch_pos)
                    },
                    at_current_branch
                )
            },
            if at_next_branch.is_empty() {
                "".to_string()
            } else {
                format!(
                    "{RESET}{}{BLUE}{} ",
                    if at_next_branch_pos == at_position {
                        "→".to_string()
                    } else {
                        format!("{}⇣", at_next_branch_pos - at_position)
                    },
                    at_next_branch
                )
            },
            at_position - main_position,
            // line_count,
            if all_branches.is_empty() {
                String::new()
            } else {
                // let branches = all_branches
                //     .iter()
                //     .map(|(_line, branch)| branch.to_string())
                //     .collect::<Vec<String>>()
                //     .join(", ");
                let branches = format!("{BLUE}{}{RESET}", all_branches.len());
                branches
            }
        )?;
    }

    // jj log --ignore-working-copy --reversed -T prompt --color never | ./target/debug/jj-prompt

    if let OutputTarget::Pager { child } = &mut fd {
        drop(child.stdin.take());
        child.wait()?;
    }
    if let Some(mut process) = jj_process {
        process.wait()?;
    }

    Ok(())
}
