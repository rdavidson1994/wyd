use chrono::{DateTime, Duration, Local, Utc};
use chrono_english::Dialect;

use std::{fmt::Display, fs::{self, OpenOptions}, io::Write, thread, time::Duration as StdDuration};

extern crate clap;
use clap::{crate_version, AppSettings, ArgSettings, Clap};

use std::default::Default;

mod job;
use job::Job;

mod job_board;

mod wyd_application;
use wyd_application::WydApplication;

use anyhow::Context;

use crate::job_board::WorkState;

fn default<D: Default>() -> D {
    Default::default()
}

impl Display for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.timebox_expired() {
            f.write_str("(!) ")?;
        }
        f.write_str(&self.label)?;
        f.write_str(" | started at ")?;
        let local_time = DateTime::<Local>::from(self.begin_date);
        let formatted_date = local_time.format("%r");
        formatted_date.fmt(f)?;
        let chrono_timebox = match self.timebox {
            Some(std_timebox) => match Duration::from_std(std_timebox) {
                Ok(chrono_timebox) => Some(chrono_timebox),
                Err(_out_of_range) => Some(Duration::seconds(0)),
            },
            None => None,
        };
        if let Some(chrono_timebox) = chrono_timebox {
            let time_elapsed = Local::now().signed_duration_since(self.begin_date);
            let time_remaining = chrono_timebox - time_elapsed;
            if let Ok(std_dur) = time_remaining.to_std() {
                f.write_str(" | timebox remaining : ")?;
                let rounded_dur = StdDuration::from_secs(std_dur.as_secs());
                let formatted_dur = humantime::format_duration(rounded_dur);
                formatted_dur.fmt(f)?;
            } else {
                f.write_str(" | timebox expired")?;
            }
        }
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

    /// Alias of `push -t 5m`
    FiveMinutes {
        /// Name of new task, similar to `push`
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
        timebox: Option<StdDuration>,

        /// Removes the current timebox instead of applying a new one.
        #[clap(long, short)]
        remove: bool,
    },

    /// Prints today's log file
    Log,

    /// Starts a countdown for mindfulness
    Meditate {
        #[clap(long, short)]
        #[clap(default_value = "20")]
        seconds: i32,

        #[clap(long, short)]
        intent: Option<String>,
    },

    /// Adds a message to today's log
    Jot {
        /// List of words forming the content of the message.
        words: Vec<String>,
    },

    /// Enters work mode (sends reminders every few minutes if no timebox is set.)
    Work {
        /// Exits work mode
        #[clap(long, short)]
        done: bool,
    }
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
    match perform_work() {
        Ok(()) => {
            // Done
        },
        Err(error) => {
            handle_error(error)
        }
    }
}

fn perform_work() -> anyhow::Result<()> {
    let args = Arguments::parse();

    let app_dir = dirs::data_local_dir()
        .context("Could not locate current user's app data folder.")?
        .join(".wyd");

    fs::create_dir_all(&app_dir).context("Could not create application directory")?;
    let mut app = WydApplication::load(app_dir).context("Failed to load application state from app directory.")?;

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
                return Ok(());
            }
            app.create_job(label, timebox, retro)?;
        }

        FiveMinutes { words } => {
            app.create_job(words.join(" "), Some(StdDuration::from_secs(5 * 60)), None)?;
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
            app.save().context("Unable to save after attempting to suspend job.")?;
        }

        Done { cancelled } => {
            app.complete_current_job(cancelled)?;
        }

        Resume { words } => {
            let pattern = words.join(" ");
            app.resume_job_named(&pattern)?;
        }

        Notifier { kill, become_id } => {
            if kill {
                app.kill_notifier();
            } else if let Some(id_str) = become_id {
                app.become_notifier(&id_str).context("Unable to start notifier process")?;
            } else {
                app.spawn_notifier();
            }
        }

        Remind { force } => {
            app.send_reminders(force)?;
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
                app.print_current_timebox();
            } else {
                app.apply_timebox(timebox)?;
            }
        }

        Log => {
            app.print_log();
        }

        Meditate { seconds, intent } => {
            for i in 0..seconds {
                println!("{}", seconds - i);
                thread::sleep(StdDuration::from_secs(1));
            }
            if let Some(intent) = intent {
                println!("{}", intent);
            }
        }

        Jot { words } => {
            let content = words.join(" ");
            app.add_log_note(content);
        }

        Work { done } => {
            let work_state = if done {
                WorkState::Off
            } else {
                WorkState::Working
            };
            app.set_work_state(work_state)?;
        }
    };

    Ok(())
}

fn handle_error(error: anyhow::Error) {
    let app_dir = dirs::data_local_dir()
        .context("Could not locate current user's app data folder.")
        .unwrap()
        .join(".wyd");

    fs::create_dir_all(&app_dir)
        .context("Could not create application directory")
        .unwrap();
    
    let mut error_log_file = OpenOptions::new()
        .write(true)
        .append(true)
        .open(app_dir.join("wyd-error.log"))
        .unwrap();

    writeln!(error_log_file, "{:#}", error)
        .context("Error attempting to write to error log")
        .unwrap();
}
