use chrono::{DateTime, Duration, Local, Utc};

use std::{fmt::Display, fs};

extern crate clap;
use clap::{crate_version, App, AppSettings, Arg, ArgMatches, ArgSettings, SubCommand};

use std::default::Default;

mod job;
use job::Job;

mod job_board;

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
                app.create_suspended_job(words, reason, timer);
            } else {
                app.suspend_job_named(&words, reason, timer)
            }
            app.save();
        }

        ("done", Some(_)) => {
            app.complete_current_job();
        },

        ("resume", Some(m)) => {
            let pattern = word_args_to_string(m);
            app.resume_job_named(&pattern);
        }

        ("notifier", Some(m)) => {
            if m.is_present("kill") {
                app.kill_notifier();
            } else if let Some(id_str) = m.value_of("become") {
                app.become_notifier(id_str);
            } else {
                app.spawn_notifier();
            }
        }

        ("remind", Some(m)) => {
            let force = m.is_present("force");
            app.send_reminders(force);
        }

        ("ls", Some(_)) => {
            app.ls_job_board();
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
