use chrono::{serde::ts_seconds, DateTime, Duration, Local, Utc};
use fs::File;
use serde::{Deserialize, Serialize};
use std::{
    fmt::Display,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
};
use uuid::Uuid;
extern crate clap;
use clap::{crate_version, App, AppSettings, Arg, ArgMatches, ArgSettings, SubCommand};
use notify_rust::Notification;
use ron::ser::{self, PrettyConfig};
use std::default::Default;
use std::time::Duration as StdDuration;
use url::Url;

mod job;
use job::Job;

mod job_board;
use job_board::{JobBoard, SuspendedStack};

pub const MIN_NOTIFICATION_DELAY_SECONDS: i64 = 60 * 3;

fn default<D: Default>() -> D {
    Default::default()
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

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct WydApplication {
    job_board: JobBoard,
    app_dir: PathBuf,
    icon_url: Url,
}

impl WydApplication {
    fn save(&self) {
        let new_file_text = ser::to_string_pretty(&self.job_board, PrettyConfig::new())
            .expect("Attempt to reserialize updated job list failed.");
        fs::write(self.app_dir.join("jobs.ron"), new_file_text)
            .expect("Failed to write updated job list.");
    }

    fn load(app_dir: PathBuf) -> WydApplication {
        let job_board = JobBoard::load(&app_dir);
        let icon_url =
            Url::from_file_path(app_dir.join("wyd-icon.png")).expect("Unable to load icon.");
        WydApplication {
            app_dir,
            job_board,
            icon_url,
        }
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

    fn add_suspended_job(&mut self, job: Job, reason: String, timer: Option<DateTime<Utc>>) {
        let new_stack = SuspendedStack {
            data: vec![job],
            reason,
            date_suspended: Utc::now(),
            timer,
            last_notifiaction: None,
        };
        self.job_board.add_suspended_stack(new_stack)
    }

    fn add_job(&mut self, job: Job) {
        let mut log_line = String::new();
        log_line.push_str(&self.get_indent());
        log_line.push_str(&format!("{}", job));
        self.job_board.push(job);
        self.print(&log_line);
    }

    fn send_reminders(&mut self, force: bool) {
        for mut job in &mut self.job_board.active_stack {
            if job.timebox_expired() {
                if !force && !should_notify(&job.last_notifiaction) {
                    continue;
                }
                Notification::new()
                    .summary("Expired timebox")
                    .body(&format!(
                        "The timebox for task \"{}\" has expired.",
                        job.label
                    ))
                    .timeout(0)
                    .appname("wyd")
                    .show()
                    .expect("Unable to show notification.");
                job.last_notifiaction = Some(Utc::now());
            }
        }
        for mut stack in &mut self.job_board.suspended_stacks {
            let timer_exhausted = match stack.timer {
                Some(timer) => timer < Utc::now(),
                None => false,
            };
            if !timer_exhausted {
                continue;
            }
            if !force && !should_notify(&stack.last_notifiaction) {
                continue;
            }

            let first_job_string = match stack.data.first() {
                Some(job) => job.to_string(),
                None => "[[Empty Job Stack D:]]".to_string(),
            };

            Notification::new()
                .summary("Timer!")
                .body(&format!(
                    "Reminder about this suspended task: \"{}\".\nSuspension reason: \"{}\"",
                    first_job_string, stack.reason
                ))
                .timeout(0)
                .appname("wyd")
                .show()
                .expect("Unable to show notification");
            stack.last_notifiaction = Some(Utc::now());
        }
    }
}

trait StringMatch: FnMut(&str) -> bool {}

impl<T> StringMatch for T where T: FnMut(&str) -> bool {}

fn word_args_to_string(args: &ArgMatches) -> String {
    args.values_of("word")
        .unwrap_or_default()
        .collect::<Vec<_>>()
        .join(" ")
}

fn substring_matcher(pattern: &str) -> impl Fn(&str) -> bool + '_ {
    move |s: &str| -> bool { s.contains(pattern) }
}

fn should_notify(last_notified: &Option<DateTime<Utc>>) -> bool {
    match last_notified {
        Some(date) => {
            Utc::now().signed_duration_since(*date).num_seconds() > MIN_NOTIFICATION_DELAY_SECONDS
        }
        None => true,
    }
}

fn main() {
    let matches = App::new("What You're Doing")
        .version(crate_version!())
        .settings(&[AppSettings::InferSubcommands])
        .subcommand(
            SubCommand::with_name("push")
                .about("Adds a new task to the top of the stack.")
                .arg(
                    Arg::with_name("timebox")
                        .help("Time until task sends reminder notifications. (e.g. 1h 30m)")
                        .long("timebox")
                        .short("t")
                        .takes_value(true),
                )
                .arg(
                    Arg::with_name("word").multiple(true).help(
                        "Name of the new task. Supports bare words like `wyd push Send emails`",
                    ),
                ),
        )
        .subcommand(
            SubCommand::with_name("done").about("Marks the top task of the stack as complete"),
        )
        .subcommand({
            let about = "Output reminders for expired timers".to_owned();
            let about_extra = r#"

If you are using the notifier (created with `wyd spawn-notifier`), you
shouldn't need this subcommand very much - the notifer effectively runs
it every second. You may still occassionally find the `--force` flag useful,
since it re-triggers reminders that have already sent notifiactions recently.
"#;
            SubCommand::with_name("remind")
                .about(about.clone().as_str())
                .long_about((about + about_extra).as_str())
                .arg(
                    Arg::with_name("force")
                        .long("force")
                        .short("f")
                        .takes_value(false)
                        .help("Re-send all active reminders, even recently sent ones."),
                )
        })
        .subcommand(
            SubCommand::with_name("ls")
                .about("Prints a list of all tasks, including suspended ones."),
        )
        .subcommand(
            SubCommand::with_name("notifier")
                .about("Starts the notifier process, which sends wyd's reminder notifications.")
                .arg(
                    Arg::with_name("kill")
                        .long("kill")
                        .short("k")
                        .takes_value(false)
                        .help("Kills any active notifier processes."),
                )
                .arg(
                    // Causes the active process to become the notifer.
                    Arg::with_name("become")
                        .long("become")
                        .takes_value(true)
                        .set(ArgSettings::Hidden),
                ),
        )
        .subcommand(
            SubCommand::with_name("resume")
                .arg(Arg::with_name("word").multiple(true))
                .about("Resumes a suspended task"),
        )
        .subcommand(
            SubCommand::with_name("suspend")
                .about("Moves a task from the active stack to the suspended queue.")
                .arg(
                    Arg::with_name("reason")
                        .long("reason")
                        .short("r")
                        .takes_value(true)
                        .help("An optional note about why you suspended the task."),
                )
                .arg(
                    Arg::with_name("timer")
                        .long("timer")
                        .short("t")
                        .takes_value(true)
                        .help("Sets a timer, after which the suspended task will send reminders."),
                )
                .arg(
                    Arg::with_name("new")
                        .long("new")
                        .short("n")
                        .takes_value(false)
                        .help(
                            "Creates a new suspended task instead of suspending an existing one.",
                        ),
                )
                .arg(
                    Arg::with_name("word")
                        .multiple(true)
                        .help("The name (or part of the name) of the task to be suspended."),
                ),
        )
        .get_matches();

    let app_dir = dirs::data_local_dir()
        .expect("Could not locate current user's app data folder.")
        .join(".wyd");

    fs::create_dir_all(&app_dir).expect("Could not create application directory");
    let mut app = WydApplication::load(app_dir);

    match matches.subcommand() {
        ("push", Some(m)) => {
            let label = word_args_to_string(m);
            if label.is_empty() {
                eprintln!("Can't create a job without a label.");
                return;
            }
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
            let words = word_args_to_string(m);
            if words.is_empty() {
                eprintln!("Can't perform suspend without a label.");
                return;
            }
            let reason = m.value_of("reason").unwrap_or("None").to_owned();
            let timer = if let Some(timer_str) = m.value_of("timer") {
                let std_duration = humantime::parse_duration(timer_str).expect("Invalid duration");
                let utc_date = Utc::now()
                    + Duration::from_std(std_duration)
                        .expect("Unable to convert std duration to chrono duration.");
                Some(utc_date)
            } else {
                None
            };

            if m.is_present("new") {
                let job = Job {
                    label: words,
                    begin_date: Utc::now(),
                    timebox: None,
                    last_notifiaction: None,
                };
                app.add_suspended_job(job, reason, timer);
            } else {
                let matcher = substring_matcher(&words);
                if app
                    .job_board
                    .suspend_matching(matcher, reason, timer)
                    .is_ok()
                {
                    println!("Job suspended.");
                } else {
                    println!("No matching job to suspend.")
                }
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
            let pattern = word_args_to_string(m);
            let outcome = if pattern.is_empty() {
                app.job_board.resume_at_index(0)
            } else {
                app.job_board.resume_matching(substring_matcher(&pattern))
            };

            if let Some(new_top) = outcome.ok().and(app.job_board.active_stack.last()) {
                println!("Job resumed: {}", new_top);
            } else {
                eprintln!("No matching job to resume.");
            }
            app.save();
        }
        ("notifier", Some(m)) => {
            let lock_path = app.app_dir.join(".notifier");
            if m.is_present("kill") {
                File::create(lock_path)
                    .expect("unable to create .notifier file.")
                    .write("kill".as_bytes())
                    .expect("Unable to write to .notifier file.");
            } else if let Some(id_str) = m.value_of("become") {
                let mut app_dir = app.app_dir;
                let mut id_buf = Vec::<u8>::with_capacity(4);
                id_buf.extend(ron::from_str::<Uuid>(id_str).unwrap().as_bytes());
                loop {
                    if lock_path.exists() {
                        let mut lock_file = OpenOptions::new().read(true).open(&lock_path).unwrap();
                        let mut file_bytes = Vec::<u8>::with_capacity(4);
                        lock_file.read_to_end(&mut file_bytes).unwrap();
                        if file_bytes.as_slice() != &id_buf {
                            break;
                        }
                    }
                    app = WydApplication::load(app_dir);
                    app.send_reminders(false);
                    app.save();
                    app_dir = app.app_dir;
                    std::thread::sleep(StdDuration::from_secs(1));
                }
            } else {
                // Default usage - spawn the notifier process
                if lock_path.exists() {
                    fs::remove_file(&lock_path).expect("Unable to delete .notifier file.");
                }
                let id = Uuid::new_v4();
                OpenOptions::new()
                    .create(true)
                    .write(true)
                    .open(&lock_path)
                    .expect("Unable to open .notifier file.")
                    .write(id.as_bytes())
                    .expect("Unable to write .notifier file.");
                let exe_path =
                    std::env::current_exe().expect("Unable to locate current executable.");
                Command::new(exe_path)
                    .arg("notifier")
                    .arg("--become")
                    .arg(ron::to_string(&id).unwrap())
                    .spawn()
                    .expect("Unable to spawn notifier process.");
            }
        }
        ("remind", Some(m)) => {
            let force = m.is_present("force");
            app.send_reminders(force);
            app.save();
        }

        ("ls", Some(_)) => {
            app.job_board.sort_suspended_stacks();
            let main_summary = app.job_board.get_summary();
            let suspended_summary = app.job_board.suspended_stack_summary();
            print!(
                "Suspended jobs:\n\n{}\n\nMain jobs:\n\n{}\n",
                suspended_summary, main_summary
            )
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
