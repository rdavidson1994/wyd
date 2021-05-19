use chrono::{DateTime, Duration, Local, Utc};
use fs::File;

use std::{
    fmt::Display,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{PathBuf},
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

mod wyd_application;
use wyd_application::WydApplication;

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

pub trait StringMatch: FnMut(&str) -> bool {}

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

// todo - private to wyd_application module
pub fn should_notify(last_notified: &Option<DateTime<Utc>>) -> bool {
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
