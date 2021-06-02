use chrono::{DateTime, Duration, Local, Utc};
use chrono_english::Dialect;

use std::{fmt::Display, fs, time::Duration as StdDuration};

extern crate clap;
use clap::{crate_version, AppSettings, ArgSettings, Clap};

use std::default::Default;

mod job;
use job::Job;

mod job_board;

mod wyd_application;
use wyd_application::WydApplication;

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

fn substring_matcher(pattern: &str) -> impl Fn(&str) -> bool + '_ {
    move |s: &str| -> bool { s.contains(pattern) }
}

fn parse_date_or_dur(input: &str) -> anyhow::Result<StdDuration> {
    let now = Local::now();
    let future = chrono_english::parse_date_string(input, now, Dialect::Us)?;
    let dur = future.signed_duration_since(now);
    Ok(dur.to_std()?)
}

#[derive(Clap, Debug)]
//     let matches = App::new("What You're Doing")
//         .version(crate_version!())
//         .settings(&[AppSettings::InferSubcommands])
enum Command {
    /// Add a new task to the top of the stack.
    Push {
        /// Time until task sends reminder notifications. (e.g. 1h 30m)
        #[clap(long, short)]
        #[clap(parse(try_from_str = humantime::parse_duration))]
        timebox: Option<StdDuration>,

        /// "Start" a job some time in the past
        #[clap(long, short)]
        #[clap(parse(try_from_str = humantime::parse_duration))]
        retro: Option<StdDuration>,

        /// Name of the new task. Supports bare words like `wyd push Send emails`
        words: Vec<String>,
    },

    /// Moves a task from the active stack to the suspended queue.
    Suspend {
        /// Sets a timer, after which the suspended task will send reminders.
        #[clap(long, short)]
        #[clap(parse(try_from_str = parse_date_or_dur))]
        timebox: Option<StdDuration>,

        /// Creates a new suspended task instead of suspending an existing one.
        #[clap(long, short)]
        new: bool,

        /// An optional note about why you suspended the task.
        #[clap(long, short, default_value = "None")]
        reason: String,

        /// The name (or part of the name) of the task to be suspended.
        words: Vec<String>,
    },

    /// Marks the top task of the stack as complete
    Done {
        /// Marks the task as cancelled instead of complete
        #[clap(long, short)]
        cancelled: bool,
    },

    /// Output reminders for expired timers
    Remind {
        /// Re-send all active reminders, even recently sent ones.
        #[clap(long, short)]
        force: bool,
    },

    /// Resumes a suspended task.
    Resume { words: Vec<String> },

    /// Prints the active task stack.
    Info,

    /// Prints a list of all tasks, including suspended ones.
    Ls,

    /// Starts the notifier process, which sends wyd's reminder notifications.
    Notifier {
        // Kill active notifier processes without creating a new one.
        #[clap(long, short)]
        kill: bool,
        #[clap(long = "become", short)]
        #[clap(setting = ArgSettings::Hidden)]
        become_id: Option<String>,
    },

    /// Applies a new timebox to the current active task
    Timebox {
        /// The new timebox (e.g. 1h5m30s)
        #[clap(parse(try_from_str = humantime::parse_duration))]
        #[clap(group("my_group"))]
        timebox: Option<StdDuration>,

        /// Removes the current timebox instead of applying a new one.
        #[clap(long, short)]
        remove: bool,
    },
}

#[derive(Clap, Debug)]
#[clap(name = "What You're Doing")]
#[clap(version = crate_version!())]
#[clap(setting = AppSettings::InferSubcommands)]
struct Arguments {
    #[clap(subcommand)]
    subcommand: Option<Command>,
}

fn main() {
    let args = Arguments::parse();

    let app_dir = dirs::data_local_dir()
        .expect("Could not locate current user's app data folder.")
        .join(".wyd");

    fs::create_dir_all(&app_dir).expect("Could not create application directory");
    let mut app = WydApplication::load(app_dir);

    let subcommand = args.subcommand.unwrap_or(Command::Info);
    use Command::*;
    match subcommand {
        Push {
            timebox,
            retro,
            words,
        } => {
            let label = words.join(" ");
            if label.is_empty() {
                eprintln!("Can't create a job without a label.");
                return;
            }
            app.create_job(label, timebox, retro);
        }

        Suspend {
            words,
            reason,
            timebox,
            new,
        } => {
            let words = words.join(" ");
            let timer = if let Some(std_duration) = timebox {
                let utc_date = Utc::now()
                    + Duration::from_std(std_duration)
                        .expect("Unable to convert std duration to chrono duration.");
                Some(utc_date)
            } else {
                None
            };

            if new {
                app.create_suspended_job(words, reason, timer);
            } else if words.is_empty() {
                app.suspend_current_job(reason, timer);
            } else {
                app.suspend_job_named(&words, reason, timer);
            }
            app.save();
        }

        Done { cancelled } => {
            app.complete_current_job(cancelled);
        }

        Resume { words } => {
            let pattern = words.join(" ");
            app.resume_job_named(&pattern);
        }

        Notifier { kill, become_id } => {
            if kill {
                app.kill_notifier();
            } else if let Some(id_str) = become_id {
                app.become_notifier(&id_str);
            } else {
                app.spawn_notifier();
            }
        }

        Remind { force } => {
            app.send_reminders(force);
        }

        Ls => {
            app.ls_job_board();
        }

        Info => {
            print!("{}", app.get_summary());
        }

        Timebox { timebox, remove } => {
            if timebox.is_some() && remove {
                eprintln!("Cannot specify a new timebox while using the --remove flag.");
            } else if timebox.is_none() && !remove {
                eprintln!("Must specify a new timebox unless using the --remove flag.");
            } else {
                app.apply_timebox(timebox);
            }
        }
    };
}
