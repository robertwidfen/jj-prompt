use regex::Regex;
use std::env;
use std::io::{self, BufRead, Write};
use std::process::{Command, Stdio};

// const RED: &str = "\x1b[31m";
// const GREEN: &str = "\x1b[32m";
// const BLUE: &str = "\x1b[34m";
// const MAGENTA: &str = "\x1b[35m";

// [48;2; means background color, [38;2; means foreground color
// remaining values are RGB values

const RED: &str = "\x1b[38;2;224;108;117m";
const GREEN: &str = "\x1b[38;2;152;195;121m";
const BLUE: &str = "\x1b[38;2;97;175;239m";
const MAGENTA: &str = "\x1b[38;2;198;120;221m";

// const LIGHT_GRAY: &str = "\x1b[38;2;120;120;120m";
// const BG_GREY: &str = "\x1b[48;2;40;44;52m";
// const BG_BLUE: &str = "\x1b[48;2;30;34;52m";

const RESET: &str = "\x1b[0m";

// // make background to go to end of line
// const CLEAR_LINE: &str = "\x1b[K";

// prompt = '''
// change_id.shortest() ++ format_bookmarks ++ "\n"
// '''

//  jj log --ignore-working-copy --reversed -T prompt

// ◆    lt main
// ├─╮
// │ ○  q feat/touchpad_zoom_pan_config
// ○  un feat/aspect-ratios
// ○  mvk feat/keep-aspect-ratio
// ○  ry feat/pointer-center-marker*
// ○  vu feat/high-contrast-helper-elements*
// ○  w
// ○  s feat/limit-crop-to-image-bounds*
// ○  ln
// ○  x
// ○  t
// ○  pu
// ○  n
// ○  oo
// ○  vs
// ○  mz
// ○  mvn
// ○    um feat/spotlight*
// ├─╮
// │ ○      rw
// │ ├─┬─╮
// │ │ │ ○  lv
// │ │ │ ○  pr
// │ │ ○  ol
// │ ○  pn WIP*
// @  y

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let re_change = Regex::new(r"^[│ ]*([@◆○×])  ([a-z]+) (.*)$").unwrap();

    let args: Vec<String> = env::args().collect();

    let mut stdout = io::stdout();
    let mut child = None;

    let fd: &mut dyn Write = if args.len() == 1 {
        &mut stdout
    } else {
        let pager_program = &args[1];
        let pager_flags = &args[2..];
        let cmd = Command::new(pager_program)
            .args(pager_flags)
            .stdin(Stdio::piped())
            .spawn()?;
        child = Some(cmd);
        child.as_mut().unwrap().stdin.as_mut().unwrap()
    };

    let mut line_count = 0;
    let mut normal_count = 0;
    let mut confict_count = 0;
    let mut main_position = 0;
    let mut at_position = 0;
    let mut all_branches = "".to_string();
    let mut at_id = "";
    let mut conflict_id = "";

    for line in stdin.lock().lines().map_while(Result::ok) {
        if let Some(captures) = re_change.captures(&line) {
            let mark = captures.get(1).map_or("", |m| m.as_str());
            let id = captures.get(2).map_or("", |m| m.as_str());
            let branches = captures.get(3).map_or("", |m| m.as_str());
            writeln!(fd, " {} {} {}", mark, id, branches)?;
            match mark {
                "@" => {
                    at_position = line_count;
                    at_id = id.clone();
                }
                "◆" => {
                    main_position = line_count;
                }
                "○" => normal_count += 1,
                "×" => {
                    confict_count += 1;
                    conflict_id = id.clone();
                }
                _ => {
                    eprintln!("Unknown mark: {}", mark);
                }
            }
            if branches.is_empty() {
                all_branches = format!("{} {}", all_branches.to_string(), branches);
            }
        } else {
            writeln!(fd, "{RED}ERROR: {}{RESET}", line)?;
        }
        line_count += 1;
    }

    if line_count > 0 {
        // @z 13⇣main feat/limit…[1]
        writeln!(
            fd,
            "{GREEN}@{MAGENTA}{}{RESET} {}⇣{} {BLUE}{}{RESET}",
            at_id,
            at_position - main_position,
            if confict_count > 0 {
                format!("{RED}{}×{}", confict_count, conflict_id)
            } else {
                String::new()
            },
            all_branches
        )?;
    }

    if let Some(mut child_process) = child {
        drop(child_process.stdin.take());
        child_process.wait()?;
    }

    Ok(())
}
