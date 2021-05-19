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

use crate::{job::Job, should_notify};
use crate::job_board::{JobBoard, SuspendedStack};


#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct WydApplication {
    //todo - private
    pub job_board: JobBoard,
    // todo - private
    pub app_dir: PathBuf,
    icon_url: Url,
}

impl WydApplication {
    pub fn save(&self) {
        let new_file_text = ser::to_string_pretty(&self.job_board, PrettyConfig::new())
            .expect("Attempt to reserialize updated job list failed.");
        fs::write(self.app_dir.join("jobs.ron"), new_file_text)
            .expect("Failed to write updated job list.");
    }

    pub fn load(app_dir: PathBuf) -> WydApplication {
        let job_board = JobBoard::load(&app_dir);
        let icon_url =
            Url::from_file_path(app_dir.join("wyd-icon.png")).expect("Unable to load icon.");
        WydApplication {
            app_dir,
            job_board,
            icon_url,
        }
    }

    // todo - private
    pub fn print(&self, message: &str) {
        self.append_to_log(&(message.to_owned() + "\n"));
        println!("{}", message.trim());
    }

    // todo - private
    pub fn get_indent(&self) -> String {
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

    pub fn add_suspended_job(&mut self, job: Job, reason: String, timer: Option<DateTime<Utc>>) {
        let new_stack = SuspendedStack {
            data: vec![job],
            reason,
            date_suspended: Utc::now(),
            timer,
            last_notifiaction: None,
        };
        self.job_board.add_suspended_stack(new_stack)
    }

    pub fn add_job(&mut self, job: Job) {
        let mut log_line = String::new();
        log_line.push_str(&self.get_indent());
        log_line.push_str(&format!("{}", job));
        self.job_board.push(job);
        self.print(&log_line);
    }

    pub fn send_reminders(&mut self, force: bool) {
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
