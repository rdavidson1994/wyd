use chrono::{serde::ts_seconds, DateTime, Duration, Local, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fmt::Display,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
extern crate clap;
use clap::{App, AppSettings, Arg, ArgMatches, SubCommand};
use ron::ser::{self, PrettyConfig};
use std::default::Default;
use std::time::Duration as StdDuration;
use notify_rust::Notification;


const MIN_NOTIFICATION_DELAY_SECONDS: i64 =  60 * 3;

#[derive(Serialize, Deserialize, Clone)]
struct Job {
    label: String,
    #[serde(with = "ts_seconds")]
    begin_date: DateTime<Utc>,
    timebox: Option<StdDuration>,
    last_notifiaction: Option<DateTime<Utc>>,
}

impl Job {
    fn timebox_remaining(&self) -> Option<StdDuration> {
        match self.timebox {
            Some(timebox) => {
                let dur_result = (self.begin_date
                    + Duration::from_std(timebox).expect("Duration out of range.")
                    - Utc::now())
                .to_std();
                match dur_result {
                    Ok(dur) => Some(dur),
                    Err(_) => Some(StdDuration::new(0, 0)),
                }
            }
            None => None,
        }
    }
    fn timebox_expired(&self) -> bool {
        self.timebox_remaining() == Some(StdDuration::new(0, 0))
    }
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
struct WydApplication {
    job_board: JobBoard,
    app_dir: PathBuf,
}

impl WydApplication {
    fn save(&self) {
        let new_file_text = ser::to_string_pretty(&self.job_board, PrettyConfig::new())
            .expect("Attempt to reserialize updated job list failed.");
        fs::write(self.app_dir.join("jobs.ron"), new_file_text)
            .expect("Failed to write updated job list.");
    }

    fn print(&self, message: &str) {
        self.append_to_log(&(message.to_owned() + "\n"));
        println!("{}", message.trim());
    }

    fn get_indent(&self) -> String {
        let mut output = String::new();
        for _ in &self.job_board.active_stack {
            output.push(' ');
        }
        output
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

    fn add_job(&mut self, job: Job) {
        let mut log_line = String::new();
        log_line.push_str(&self.get_indent());
        log_line.push_str(&format!("{}", job));
        self.job_board.push(job);
        self.print(&log_line);
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct JobBoard {
    active_stack: JobStack,
    suspended_stacks: VecDeque<SuspendedStack>,
}

trait StringMatch: FnMut(&str) -> bool {}

impl<T> StringMatch for T where T: FnMut(&str) -> bool {}

impl JobBoard {
    #[allow(dead_code)]
    fn empty() -> Self {
        JobBoard {
            active_stack: default(),
            suspended_stacks: default(),
        }
    }

    fn load(app_dir: &Path) -> Self {
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
        if contents.is_empty() {
            default()
        } else {
            ron::from_str(&contents).expect(&bad_path("Stack file at {} is malformed."))
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

    fn push(&mut self, job: Job) {
        self.active_stack.push(job);
    }

    fn pop(&mut self) -> Option<Job> {
        self.active_stack.pop()
    }

    fn num_active_jobs(&self) -> usize {
        self.active_stack.len()
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
    let job_board = JobBoard::load(&app_dir);
    let mut app = WydApplication { app_dir, job_board };

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
                last_notifiaction: None,
            };
            app.add_job(job);
            app.save();
        }
        ("suspend", Some(m)) => {
            let pattern = m
                .value_of("pattern")
                .expect("Mandatory argument")
                .to_owned();
            let reason = m.value_of("reason").expect("Mandatory argument").to_owned();

            let matcher = substring_matcher(&pattern);

            if app.job_board.suspend_matching(matcher, reason).is_ok() {
                println!("Job suspended.");
            } else {
                println!("No matching job to suspend.")
            }
            app.save();
        }
        ("done", Some(_)) => match app.job_board.pop() {
            Some(job) => {
                let duration = Local::now().signed_duration_since(job.begin_date);
                let non_negative_dur = Duration::seconds(duration.num_seconds())
                    .to_std()
                    .unwrap_or(StdDuration::new(0, 0));
                let duration_str = humantime::format_duration(non_negative_dur);

                let log_line = format!(
                    "{}Completed job \"{}\" (time elapsed: {})",
                    app.get_indent(),
                    job.label,
                    duration_str
                );
                app.print(&log_line);
                if let Some(new_job) = app.job_board.active_stack.last() {
                    println!("{}", new_job)
                } else {
                    print!("{}", app.job_board.get_summary())
                }
                app.save();
            }
            None => {
                print!("{}", app.job_board.empty_stack_message())
            }
        },
        ("resume", Some(m)) => {
            let outcome = match m.value_of("pattern") {
                Some(pattern) => app.job_board.resume_matching(substring_matcher(&pattern)),
                None => app.job_board.resume_at_index(0),
            };

            if outcome.is_ok() {
                println!("Job resumed: {}", app.job_board.active_stack[0]);
            } else {
                println!("No matching job to resume.");
            }
            app.save();
        }
        ("remind", Some(_)) => {
            for mut job in &mut app.job_board.active_stack {
                if job.timebox_expired() {
                    let should_notify = match job.last_notifiaction {
                        Some(notification_date) => {
                            Utc::now()
                                .signed_duration_since(notification_date)
                                .num_seconds()
                                > MIN_NOTIFICATION_DELAY_SECONDS
                        }
                        None => true,
                    };
                    if should_notify {
                        Notification::new()
                            .summary("Expired timebox")
                            .body(&format!("The timebox for task \"{}\" has expired.", job.label))
                            .timeout(0)
                            .appname("wyd")
                            //.icon("firefox")
                            .show().expect("Unable to show notification");
                        job.last_notifiaction = Some(Utc::now());
                    }
                }
            }
            app.save();
        }
        (missing, Some(_)) => {
            unimplemented!("No implementation for subcommand {}", missing)
        }
        ("", None) => {
            print!("{}", app.job_board.get_summary());
        }
        (invalid, None) => {
            panic!("Invalid subcommand {}", invalid)
        }
    };
}
