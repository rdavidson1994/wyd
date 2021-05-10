use chrono::{serde::ts_seconds, DateTime, Duration, Local, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fmt::Display,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
};
extern crate clap;
use clap::{App, AppSettings, Arg, ArgMatches, SubCommand};
use ron::ser::{self, PrettyConfig};
use std::default::Default;

#[derive(Serialize, Deserialize, Clone)]
struct Job {
    label: String,
    #[serde(with = "ts_seconds")]
    begin_date: DateTime<Utc>,
    timebox: Option<std::time::Duration>,
}

fn default<D: Default>() -> D {
    Default::default()
}

#[derive(Serialize, Deserialize, Clone)]
struct SuspendedStack {
    data: JobStack,
    reason: String,
    #[serde(with = "ts_seconds")]
    date_suspended: DateTime<Utc>,
}

impl Display for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)?;
        f.write_str(" | started at ")?;
        let local_time = DateTime::<Local>::from(self.begin_date);
        let formatted_date = local_time.format("%r");
        formatted_date.fmt(f)?;
        Ok(())
    }
}

type JobStack = Vec<Job>;

#[derive(Serialize, Deserialize, Clone)]
struct JobBoard {
    active_stack: JobStack,
    suspended_stacks: VecDeque<SuspendedStack>,
    app_dir: PathBuf,
}

trait StringMatch: FnMut(&str) -> bool {}

impl<T> StringMatch for T where T: FnMut(&str) -> bool {}

impl JobBoard {
    #[allow(dead_code)]
    fn empty(app_dir: PathBuf) -> Self {
        JobBoard {
            active_stack: default(),
            suspended_stacks: default(),
            app_dir,
        }
    }

    fn load(app_dir: PathBuf) -> Self {
        let stack_file_path = app_dir.join("jobs.ron");
        let bad_path = |s: &str| s.replace("{}", &format!("{:?}", &stack_file_path));
        OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&stack_file_path)
            .expect(&bad_path("Failed to open or create file {}"));
        let contents =
            fs::read_to_string(&stack_file_path).expect(&bad_path("Failed to read file {}"));
        let (active_stack, suspended_stacks) = if contents.is_empty() {
            default()
        } else {
            ron::from_str(&contents).expect(&bad_path("Stack file at {} is malformed."))
        };
        JobBoard {
            app_dir,
            active_stack,
            suspended_stacks,
        }
    }

    fn find_job(&self, mut predicate: impl StringMatch) -> Option<(usize, &Job)> {
        for (index, job) in self.active_stack.iter().enumerate() {
            if predicate(&job.label) {
                return Some((index, job));
            }
        }
        None
    }

    fn suspend_at(&mut self, index: usize, reason: String) -> Result<(), ()> {
        if index >= self.active_stack.len() {
            return Err(());
        }
        let jobs_to_suspend = self.active_stack.split_off(index);
        let suspended_stack = SuspendedStack {
            data: jobs_to_suspend,
            reason,
            date_suspended: Utc::now(),
        };
        self.suspended_stacks.push_back(suspended_stack);
        Ok(())
    }

    fn suspend_matching(&mut self, pattern: impl StringMatch, reason: String) -> Result<(), ()> {
        if let Some((i, _job)) = self.find_job(pattern) {
            self.suspend_at(i, reason)
        } else {
            Err(())
        }
    }

    fn resume_matching(&mut self, mut pattern: impl StringMatch) -> Result<(), ()> {
        let mut found_index = self.suspended_stacks.len();
        for (i, stack) in self.suspended_stacks.iter().enumerate() {
            if pattern(&stack.data[0].label) {
                found_index = i;
                break;
            }
        }
        self.resume_at_index(found_index)
    }

    fn resume_at_index(&mut self, index: usize) -> Result<(), ()> {
        match self.suspended_stacks.remove(index) {
            Some(mut suspended_stack) => {
                for mut job in &mut suspended_stack.data {
                    job.begin_date = Utc::now();
                }
                self.active_stack.extend(suspended_stack.data);
                Ok(())
            }
            None => Err(()),
        }
    }

    fn save(self) {
        let new_file_text = ser::to_string_pretty(
            &(self.active_stack, self.suspended_stacks),
            PrettyConfig::new(),
        )
        .expect("Attempt to reserialize updated job list failed.");
        fs::write(self.app_dir.join("jobs.ron"), new_file_text)
            .expect("Failed to write updated job list.");
    }

    fn append_to_log(&self, text: &str) {
        let date = Local::now();
        let log_file_name = format!("{}", date.format("wyd-%F.log"));
        let log_path = self.app_dir.join(log_file_name);

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .expect(&format!("Failed to open log file at {:?}", log_path));

        file.write(text.as_bytes())
            .expect(&format!("Failed to write to log file at {:?}", log_path));
    }

    fn push(&mut self, job: Job) {
        self.active_stack.push(job);
    }

    fn pop(&mut self) -> Option<Job> {
        self.active_stack.pop()
    }

    fn num_active_jobs(&self) -> usize {
        self.active_stack.len()
    }

    fn get_indent(&self) -> String {
        let mut output = String::new();
        for _ in 0..self.num_active_jobs() {
            output.push(' ');
        }
        output
    }

    fn print(&self, message: &str) {
        self.append_to_log(&(message.to_owned() + "\n"));
        println!("{}", message.trim());
    }

    fn get_summary(&self) -> String {
        if self.num_active_jobs() == 0 {
            format!("{}", self.empty_stack_message())
        } else {
            self.active_stack
                .iter()
                .map(|job| format!("{}\n", job))
                .collect()
        }
    }

    fn empty_stack_message(&self) -> String {
        let mut output = String::new();
        if self.suspended_stacks.len() > 0 {
            output.push_str("You finished your jobs in progress. Yay! Use `wyd resume` to resume the topmost suspended task:\n");
            for stack in &self.suspended_stacks {
                for (i, job) in stack.data.iter().enumerate() {
                    if i == 0 {
                        output.push_str(&job.label);
                        output.push_str(" (suspended at ");
                        output.push_str(&format!(
                            "{}",
                            DateTime::<Local>::from(stack.date_suspended).format("%r")
                        ));
                        output.push_str(")");
                    } else {
                        output.push_str("    ");
                        output.push_str(&job.label);
                    }
                    output.push('\n');
                }
            }
        } else {
            output.push_str("No jobs in progress, and no suspended tasks! Use `wyd push [some arbitrary label]` to start a new task.")
        }
        output
    }
}

fn word_args_to_string(args: &ArgMatches) -> String {
    args.values_of("word")
        .expect("Cannot create an empty entry.")
        .collect::<Vec<_>>()
        .join(" ")
}

fn substring_matcher(pattern: &str) -> impl Fn(&str) -> bool + '_ {
    move |s: &str| -> bool { s.contains(pattern) }
}

fn main() {
    let app_dir = dirs::data_local_dir()
        .expect("Could not locate current user's app data folder.")
        .join(".wyd");

    fs::create_dir_all(&app_dir).expect("Could not create application directory");
    let mut job_board = JobBoard::load(app_dir);

    let matches = App::new("What're You Doing")
        .settings(&[AppSettings::InferSubcommands])
        .subcommand(
            SubCommand::with_name("push")
                .arg(
                    Arg::with_name("timebox")
                        .long("timebox")
                        .short("t")
                        .takes_value(true),
                )
                .arg(Arg::with_name("word").multiple(true)),
        )
        .subcommand(SubCommand::with_name("done"))
        .subcommand(SubCommand::with_name("remind"))
        .subcommand(
            SubCommand::with_name("resume").arg(
                Arg::with_name("pattern")
                    .long("pattern")
                    .short("p")
                    .takes_value(true),
            ),
        )
        .subcommand(
            SubCommand::with_name("suspend")
                .arg(
                    Arg::with_name("pattern")
                        .required(true)
                        .long("pattern")
                        .short("p")
                        .takes_value(true),
                )
                .arg(
                    Arg::with_name("reason")
                        .long("reason")
                        .short("r")
                        .required(true)
                        .takes_value(true),
                ),
        )
        .get_matches();

    match matches.subcommand() {
        ("push", Some(m)) => {
            let indent = job_board.get_indent();
            let label = word_args_to_string(m);
            let timebox = match m.value_of("timebox") {
                Some(string) => {
                    let dur = humantime::parse_duration(string).expect("Invalid timebox value.");
                    Some(dur)
                }
                None => None,
            };

            let job = Job {
                label,
                begin_date: Utc::now(),
                timebox,
            };
            let mut log_line = String::new();
            log_line.push_str(&indent);
            log_line.push_str(&format!("{}", job));
            job_board.push(job);
            job_board.print(&log_line);
            job_board.save();
        }
        ("suspend", Some(m)) => {
            let pattern = m
                .value_of("pattern")
                .expect("Mandatory argument")
                .to_owned();
            let reason = m.value_of("reason").expect("Mandatory argument").to_owned();

            let matcher = substring_matcher(&pattern);

            if job_board.suspend_matching(matcher, reason).is_ok() {
                println!("Job suspended.");
            } else {
                println!("No matching job to suspend.")
            }
            job_board.save();
        }
        ("done", Some(_)) => match job_board.pop() {
            Some(job) => {
                let duration = Local::now().signed_duration_since(job.begin_date);
                let non_negative_dur = Duration::seconds(duration.num_seconds())
                    .to_std()
                    .unwrap_or(std::time::Duration::new(0, 0));
                let duration_str = humantime::format_duration(non_negative_dur);

                let log_line = format!(
                    "{}Completed job \"{}\" (time elapsed: {})",
                    job_board.get_indent(),
                    job.label,
                    duration_str
                );
                job_board.print(&log_line);
                if let Some(new_job) = job_board.active_stack.last() {
                    println!("{}", new_job)
                } else {
                    print!("{}", job_board.get_summary())
                }
                job_board.save();
            }
            None => {
                print!("{}", job_board.empty_stack_message())
            }
        },
        ("resume", Some(m)) => {
            let outcome = match m.value_of("pattern") {
                Some(pattern) => job_board.resume_matching(substring_matcher(&pattern)),
                None => job_board.resume_at_index(0),
            };

            if outcome.is_ok() {
                println!("Job resumed: {}", job_board.active_stack[0]);
            } else {
                println!("No matching job to resume.");
            }
            job_board.save();
        }
        ("remind", Some(_)) => {
            println!("[sent a reminder]")
        }
        (missing, Some(_)) => {
            unimplemented!("No implementation for subcommand {}", missing)
        }
        ("", None) => {
            print!("{}", job_board.get_summary());
        }
        (invalid, None) => {
            panic!("Invalid subcommand {}", invalid)
        }
    };
}
