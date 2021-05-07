use std::{fmt::Display, fs::{self, OpenOptions}, io::Write, path::Path};
use chrono::{DateTime, Duration, Local, Utc, serde::ts_seconds};
use serde::{Serialize, Deserialize};
extern crate clap;
use clap::{App, AppSettings, Arg, SubCommand};

#[derive(Serialize, Deserialize, Clone)]
enum StackEntry {
    ActiveTask(ActiveTask)
}

#[derive(Serialize, Deserialize, Clone)]
struct ActiveTask {
    label: String,
    #[serde(with = "ts_seconds")]
    begin_date: DateTime<Utc>
}

impl Display for ActiveTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)?;
        f.write_str(" | started at ")?;
        let local_time = DateTime::<Local>::from(self.begin_date);
        let formatted_date = local_time.format("%r");
        formatted_date.fmt(f)?;
        Ok(())
    }
}

fn print_stack_empty() {
    println!("No tasks in progress. Use `wyd push [some arbitrary label]` to start one.");
}

fn append_to_log(text: &str, app_dir: &Path) {
    let date = Local::now();
    let log_file_name = format!("{}",date.format("wyd-%F.log"));
    let log_path = app_dir.join(log_file_name);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .expect(&format!("Failed to open log file at {:?}", log_path));
    file.write(text.as_bytes())
        .expect(&format!("Failed to write to log file at {:?}", log_path));
}

fn get_indent<T>(stack: &Vec<T>) -> String {
    let mut output = String::new();
    for _ in 0..stack.len() {
        output.push(' ');
    }
    output
}



fn main() {
    let app_dir = &dirs::data_local_dir()
        .expect("Could not locate current user's app data folder.")
        .join(".wyd");

    let stack_file_path = &app_dir.join("stack.ron");

    let bad_path = |s: &str| {
        s.replace("{}",&format!("{:?}",stack_file_path))
    };

    let print = |s: &str| {
        append_to_log(&(s.to_owned() + "\n"), app_dir);
        println!("{}",s.trim());
    };

    fs::create_dir_all(app_dir)
        .expect(&bad_path("Attempted to create {}, but directory creation failed."));
    

    OpenOptions::new().create(true).read(true).write(true).open(stack_file_path)
        .expect(&bad_path("Failed to open or create file {}"));
    

    let contents = fs::read_to_string(stack_file_path)
        .expect(&bad_path("Failed to read file {}"));

    let mut task_stack : Vec<StackEntry>;
    if contents.is_empty() {
        task_stack = vec![];
    } else {
        task_stack = ron::from_str(&contents)
            .expect(&bad_path("Stack file at {} is malformed."))
    }

    let save = |t| {
        let new_file_text = ron::to_string(&t)
            .expect("Attempt to reserialize updated task list failed.");

        fs::write(stack_file_path, new_file_text)
            .expect(&bad_path("Failed to write updated task list to {}"));
    };


    let matches = App::new("What're You Doing")
        .settings(&[AppSettings::InferSubcommands])
        .subcommand(SubCommand::with_name("push")
            .arg(Arg::with_name("word")
                .multiple(true)
            )
        )
        .subcommand(SubCommand::with_name("done"))
        .subcommand(SubCommand::with_name("remind"))
        .get_matches();
    
    match matches.subcommand() {
        ("push", Some(m)) => {
            let indent = get_indent(&task_stack);
            let label = m.values_of("word")
                .expect("Cannot create an empty entry.")
                .collect::<Vec<_>>()
                .join(" ");
            let task = ActiveTask {
                label,
                begin_date: Utc::now()
            };
            let mut log_line = String::new();
            log_line.push_str(&indent);
            log_line.push_str(&format!("{}", task));
            task_stack.push(StackEntry::ActiveTask(task));
            save(task_stack);
            print(&log_line);     
        }
        ("done", Some(_)) => {
            match task_stack.pop() {
                Some(StackEntry::ActiveTask(task)) => {
                    let duration = Local::now().signed_duration_since(task.begin_date);
                    let non_negative_dur = Duration::seconds(duration.num_seconds()).to_std().unwrap_or(std::time::Duration::new(0,0));
                    let duration_str = humantime::format_duration(non_negative_dur);

                    let log_line = format!(
                        "{}Completed task \"{}\" (time elapsed: {})",
                        get_indent(&task_stack),
                        task.label,
                        duration_str
                    );
                    
                    save(task_stack);
                    print(&log_line);
                }
                None => {
                    print_stack_empty();
                }
            }
        }
        ("remind", Some(_)) => {
            println!("[sent a reminder]")
        }
        (missing, Some(_)) => {
            unimplemented!("No implementation for subcommand {}", missing)
        }
        ("", None) => {
            if task_stack.len() == 0 {
                print_stack_empty();
            }
            for entry in task_stack {
                match entry {
                    StackEntry::ActiveTask(task) => {
                        println!("{}", task);
                    }
                }
            }

        }
        (invalid, None) => {
            panic!("Invalid subcommand {}", invalid)
        }
    };
}