use chrono::{DateTime, Local, Utc};
use uuid::Uuid;

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
    process::Command,
};

const MIN_NOTIFICATION_DELAY_SECONDS: i64 = 60 * 3;

extern crate clap;

use notify_rust::Notification;
use ron::ser::{self, PrettyConfig};

use url::Url;

use crate::job::Job;
use crate::{
    job_board::{JobBoard, SuspendedStack},
    substring_matcher,
};

fn should_notify(last_notified: &Option<DateTime<Utc>>) -> bool {
    match last_notified {
        Some(date) => {
            Utc::now().signed_duration_since(*date).num_seconds() > MIN_NOTIFICATION_DELAY_SECONDS
        }
        None => true,
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct WydApplication {
    job_board: JobBoard,
    app_dir: PathBuf,
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

    pub fn create_suspended_job(
        &mut self,
        label: String,
        reason: String,
        timer: Option<DateTime<Utc>>,
    ) {
        let job = Job {
            label,
            begin_date: Utc::now(),
            timebox: None,
            last_notifiaction: None,
        };
        let new_stack = SuspendedStack {
            data: vec![job],
            reason,
            date_suspended: Utc::now(),
            timer,
            last_notifiaction: None,
        };
        self.job_board.add_suspended_stack(new_stack);
    }

    pub fn create_job(&mut self, label: String, timebox: Option<std::time::Duration>) {
        let job = Job {
            label,
            begin_date: Utc::now(),
            timebox,
            last_notifiaction: None,
        };
        let mut log_line = String::new();
        log_line.push_str(&self.get_indent());
        log_line.push_str(&format!("{}", job));
        self.job_board.push(job);
        self.print(&log_line);
        self.save();
    }

    fn lock_path(&self) -> PathBuf {
        self.app_dir.join(".notifier")
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
        self.save();
    }

    // CLI methods:

    pub fn kill_notifier(&self) {
        File::create(self.lock_path())
            .expect("unable to create .notifier file.")
            .write("kill".as_bytes())
            .expect("Unable to write to .notifier file.");
    }

    pub fn become_notifier(mut self, id_str: &str) {
        let lock_path = self.lock_path();
        let mut app_dir = self.app_dir;
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
            self = WydApplication::load(app_dir);
            self.send_reminders(false);
            app_dir = self.app_dir;
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    pub fn spawn_notifier(&self) {
        let lock_path = self.lock_path();
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
        let exe_path = std::env::current_exe().expect("Unable to locate current executable.");
        Command::new(exe_path)
            .arg("notifier")
            .arg("--become")
            .arg(ron::to_string(&id).unwrap())
            .spawn()
            .expect("Unable to spawn notifier process.");
    }

    pub fn ls_job_board(&mut self) {
        self.job_board.sort_suspended_stacks();
        let main_summary = self.job_board.get_summary();
        let suspended_summary = self.job_board.suspended_stack_summary();
        print!(
            "Suspended jobs:\n\n{}\n\nMain jobs:\n\n{}\n",
            suspended_summary, main_summary
        )
    }

    pub fn suspend_current_job(
        &mut self,
        reason: String,
        timer: Option<DateTime<Utc>>
    ) {
        if self.job_board.suspend_current(reason, timer).is_ok() {
            println!("Job suspended.");
        } else {
            println!("No job to suspend.")
        }
    }

    pub fn suspend_job_named(
        &mut self,
        pattern: &str,
        reason: String,
        timer: Option<DateTime<Utc>>,
    ) {
        let matcher = substring_matcher(&pattern);
        if self
            .job_board
            .suspend_matching(matcher, reason, timer)
            .is_ok()
        {
            println!("Job suspended.");
        } else {
            println!("No matching job to suspend.")
        }
    }

    pub fn resume_job_named(&mut self, pattern: &str) {
        let outcome = if pattern.is_empty() {
            self.job_board.resume_at_index(0)
        } else {
            self.job_board.resume_matching(substring_matcher(&pattern))
        };

        if let Some(new_top) = outcome.ok().and(self.job_board.active_stack.last()) {
            println!("Job resumed: {}", new_top);
        } else {
            eprintln!("No matching job to resume.");
        }
        self.save();
    }

    pub fn complete_current_job(&mut self) {
        match self.job_board.pop() {
            Some(job) => {
                let duration = Local::now().signed_duration_since(job.begin_date);
                let non_negative_dur = chrono::Duration::seconds(duration.num_seconds())
                    .to_std()
                    .unwrap_or(std::time::Duration::new(0, 0));
                let duration_str = humantime::format_duration(non_negative_dur);

                let log_line = format!(
                    "{}Completed job \"{}\" (time elapsed: {})",
                    self.get_indent(),
                    job.label,
                    duration_str
                );
                self.print(&log_line);
                if let Some(new_job) = self.job_board.active_stack.last() {
                    println!("{}", new_job)
                } else {
                    print!("{}", self.job_board.get_summary())
                }
                self.save();
            }
            None => {
                print!("{}", self.job_board.empty_stack_message())
            }
        }
    }

    pub fn get_summary(&self) -> String {
        self.job_board.get_summary()
    }
}
