use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Local, Utc};
use uuid::Uuid;

use std::{
    fmt::Display,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
    process::Command,
    time::Duration as StdDuration,
};

extern crate clap;

// use notify_rust::Notification;
use ron::ser::{self, PrettyConfig};

use url::Url;

use std::io::BufReader;
use rodio::{Decoder, OutputStream, source::Source};

use crate::{job::Job, job_board::WorkState};
use crate::{
    job_board::{JobBoard, SuspendedStack},
    substring_matcher,
};

pub struct TimerState {
    needs_save: bool,
    send_alarm: bool
}

fn should_notify(last_notified: &Option<DateTime<Utc>>) -> bool {
    // We only send one notification to avoid spam.
    // Later, we can think about sequence of contingency notifications,
    // But for now this is the simplest way.
    let last_notified = match last_notified {
        Some(date) => date,
        None => return true,
    };
    if Utc::now().signed_duration_since(*last_notified) > Duration::seconds(30) {
        true
    } else {
        false
    }
}

// fn play_alarm() -> Result<()> {
//     let (_stream, stream_handle) = OutputStream::try_default().unwrap();
//     let file = BufReader::new(File::open(r"C:\Windows\Media\Alarm01.wav").unwrap());
//     let source = Decoder::new(file).unwrap();
//     stream_handle.play_raw(source.convert_samples())?;
//     std::thread::sleep(std::time::Duration::from_secs(5));
//     Ok(())
// }

fn play_alarm() -> Result<()> {
    let (_stream, stream_handle) = OutputStream::try_default().unwrap();
    let audio_bytes : &[u8] = include_bytes!("audio/bell.wav");
    //let file = BufReader::new((&include_bytes!("audio/bell.wav").read_u8()));//BufReader::new(File::open(r"C:\Windows\Media\Alarm01.wav").unwrap());
    let cursor = std::io::Cursor::new(audio_bytes);
    let reader = BufReader::new(cursor);
    let source = Decoder::new(reader).unwrap();
    stream_handle.play_raw(source.convert_samples())?;
    std::thread::sleep(std::time::Duration::from_secs(5));
    Ok(())
}


#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct WydApplication {
    job_board: JobBoard,
    app_dir: PathBuf,
    icon_url: Url,
}


impl WydApplication {
    pub fn save(&self) -> anyhow::Result<()> {
        // Create a backup copy of the jobs file before we overwrite it
        let copy_result = fs::copy(self.app_dir.join("jobs.ron"), self.current_backup_path());

        // Add any resulting errors from this copy to the log
        if let Err(io_error) = copy_result {
            self.append_to_log(&io_error.to_string())
        }

        // Serialize the current job board, and write the result into jobs.ron
        let new_file_text = ser::to_string_pretty(&self.job_board, PrettyConfig::new())
            .context("Attempt to reserialize updated job list failed.")?;
        fs::write(self.app_dir.join("jobs.ron"), new_file_text)
            .context("Failed to write updated job list.")?;

        Ok(())
    }

    pub fn load(app_dir: PathBuf) -> anyhow::Result<WydApplication> {
        let job_board = JobBoard::load(&app_dir);
        let icon_url = match Url::from_file_path(app_dir.join("wyd-icon.png")) {
            Ok(url) => url,
            Err(()) => bail!("Failed to create file url for icon."),
        };
        Ok(WydApplication {
            app_dir,
            job_board,
            icon_url,
        })
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

    fn current_log_path(&self) -> PathBuf {
        let date = Local::now();
        let log_file_name = format!("{}", date.format("wyd-%F.log"));
        self.app_dir.join(log_file_name)
    }

    fn current_backup_path(&self) -> PathBuf {
        let date = Local::now();
        let log_file_name = format!("{}", date.format("jobs-archive-%F.ron"));
        self.app_dir.join(log_file_name)
    }

    fn append_to_log(&self, text: &str) {
        let log_path = self.current_log_path();

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
            last_notification: None,
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

    pub fn create_job(
        &mut self,
        label: String,
        timebox: Option<StdDuration>,
        retro: Option<StdDuration>,
    ) -> anyhow::Result<()> {
        let begin_date = if let Some(retro) = retro {
            let dur =
                Duration::from_std(retro).expect("Unable to convert duration to chrono format.");
            Utc::now()
                .checked_sub_signed(dur)
                .expect("Unable to subtract duration from current date.")
        } else {
            Utc::now()
        };

        if let Some(Job {
            timebox: Some(_), ..
        }) = self.job_board.active_stack.last()
        {
            // Timeboxed tasks cannot have subtasks
            eprintln!(
                "Current job has a timebox. \
                Finish the task or remove the timebox before \
                Creating a sub task."
            );
            return Ok(());
        }

        let job = Job {
            label,
            begin_date,
            timebox,
            last_notification: None,
        };

        let mut log_line = String::new();
        log_line.push_str(&self.get_indent());
        log_line.push_str(&format!("{}", job));
        self.print(&log_line);
        self.job_board.push(job);
        self.save().context("Unable to save after job creation.")?;
        Ok(())
    }

    fn indent(&self, text: impl Display) -> String {
        let mut log_line = String::new();
        log_line.push_str(&self.get_indent());
        log_line.push_str(&format!("{}", text));
        log_line
    }

    fn timestamp(&self, text: impl Display) -> String {
        let timestamp = Local::now().format("%r");
        format!("{}: {}", timestamp, text)
    }

    fn lock_path(&self) -> PathBuf {
        self.app_dir.join(".notifier")
    }

    pub fn update_timers(&mut self) -> anyhow::Result<TimerState> {
        for job in &mut self.job_board.active_stack {
            if job.timebox_expired() {
                if !should_notify(&job.last_notification) {
                    continue;
                }
                
                job.last_notification = Some(Utc::now());
                return Ok(TimerState{ send_alarm: true, needs_save: true});
            }
        }

        for stack in &mut self.job_board.suspended_stacks {
            let timer_exhausted = match stack.timer {
                Some(timer) => timer < Utc::now(),
                None => false,
            };
            if !timer_exhausted {
                continue;
            }
            if !should_notify(&stack.last_notifiaction) {
                continue;
            }
            stack.last_notifiaction = Some(Utc::now());
            return Ok(TimerState{ send_alarm: true, needs_save: true});
        }

        let slack_date = match self.job_board.work_state {
            WorkState::Off => None,
            WorkState::Working => Some(Utc::now()),
            WorkState::SlackingSince(date) => Some(date),
        };

        if let Some(slack_date) = slack_date {
            let mut timer_state = TimerState{ send_alarm: false, needs_save: false};
            let is_slacking = self.job_board.active_stack.iter().all(|job| {
                job.timebox.is_none()
            });
            let new_work_state = if is_slacking {
                let now = Utc::now();
                if now.signed_duration_since(slack_date).num_seconds() > 5*60 {
                    timer_state.send_alarm = true;
                    WorkState::SlackingSince(now)
                }
                else {
                    WorkState::SlackingSince(slack_date)
                }
            } else {
                WorkState::Working
            };

            if new_work_state != self.job_board.work_state {
                self.job_board.work_state = new_work_state;
                timer_state.needs_save = true;
            }

            return Ok(timer_state);
        }

        return Ok(TimerState{ send_alarm: false, needs_save: false});    
    }

    // CLI methods:

    pub fn kill_notifier(&self) {
        File::create(self.lock_path())
            .expect("unable to create .notifier file.")
            .write("kill".as_bytes())
            .expect("Unable to write to .notifier file.");
    }

    pub fn become_notifier(mut self, id_str: &str) -> anyhow::Result<()> {
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
            self = WydApplication::load(app_dir).context("Failed to deserialize application state")?;
            let timer_state = self.update_timers()?;
            if timer_state.needs_save {
                self.save().context("Unable to save from reminder thread.")?;
            }
            if timer_state.send_alarm {
                play_alarm().context("Unable to play alarm sound")?;
            }
            app_dir = self.app_dir;
            std::thread::sleep(std::time::Duration::from_secs(1));
        };
        Ok(())
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

    pub fn suspend_current_job(&mut self, reason: String, timer: Option<DateTime<Utc>>) {
        if self.job_board.suspend_current(reason, timer).is_ok() {
            println!("Job suspended.");
        } else {
            println!("No job to suspend.")
        }
    }

    pub fn apply_timebox(&mut self, timebox: Option<StdDuration>) -> anyhow::Result<()> {
        if let Some(job) = self.job_board.active_stack.last_mut() {
            job.timebox = timebox;
            match timebox {
                Some(timebox) => {
                    let formatted_duration = humantime::format_duration(timebox);
                    println!(
                        "Applied timebox \"{t}\" to job \"{j}\"",
                        t = formatted_duration,
                        j = job.label
                    );
                }
                None => {
                    println!("Removed timebox from job \"{j}\"", j = job.label);
                }
            }

            // Refresh the job's begin date, so that the timebox
            // just applied is measured from now
            job.begin_date = Utc::now();

            self.save().context("Unable to save after applying timebox.")?;
        } else {
            println!("No active job to apply timebox to.");
        }
        Ok(())
    }

    pub fn print_current_timebox(&self) {
        if let Some(job) = self.job_board.active_stack.last() {
            if let Some(timebox) = job.timebox {
                let timebox = match chrono::Duration::from_std(timebox) {
                    Ok(timebox) => timebox,
                    Err(_) => todo!(),
                };
                let expiry_utc = match job.begin_date.checked_add_signed(timebox) {
                    Some(expiry) => expiry,
                    None => todo!(),
                };
                let expiry = chrono::DateTime::<Local>::from(expiry_utc);
                println!("Current timebox: {}", expiry.format("%a %F %r"))
            }
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

    pub fn resume_job_named(&mut self, pattern: &str) -> anyhow::Result<()> {
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
        self.save().context("Unable to save after resuming job")?;
        Ok(())
    }

    pub fn complete_current_job(&mut self, cancelled: bool) -> anyhow::Result<()> {
        match self.job_board.pop() {
            Some(job) => {
                let duration = Local::now().signed_duration_since(job.begin_date);
                let non_negative_dur = chrono::Duration::seconds(duration.num_seconds())
                    .to_std()
                    .unwrap_or(std::time::Duration::new(0, 0));
                let duration_str = humantime::format_duration(non_negative_dur);

                let log_line = format!(
                    "{indent}{verb} job \"{j}\" (time elapsed: {t})",
                    indent = self.get_indent(),
                    verb = if cancelled { "Cancelled" } else { "Finished" },
                    j = job.label,
                    t = duration_str
                );
                self.print(&log_line);
                if let Some(new_job) = self.job_board.active_stack.last() {
                    println!("{}", new_job)
                } else {
                    print!("{}", self.job_board.get_summary())
                }
                self.save().context("Unable to save after completing job")?;
                Ok(())
            }
            None => {
                print!("{}", self.job_board.empty_stack_message());
                Ok(())
            }
        }
    }

    pub fn get_summary(&self) -> String {
        self.job_board.get_summary()
    }

    #[allow(dead_code)]
    pub fn write_html(&mut self) {
        let output = self.job_board.generate_html();
        match fs::write(self.app_dir.join("wyd-homepage.html"), output) {
            Ok(()) => (),
            Err(x) => self.append_to_log(&format!(
                "Could not write to html summary due to this error: {}",
                x
            )),
        }
    }

    pub fn print_log(&self) {
        let log_path = self.current_log_path();
        let log_content =
            fs::read_to_string(log_path).unwrap_or("[Today's log is empty]".to_owned());
        println!("{}", log_content);
    }

    pub fn add_log_note(&self, content: String) -> () {
        let formatted_content = self.indent(self.timestamp(content));
        self.append_to_log(&(formatted_content + "\n"))
    }

    pub fn set_work_state(&mut self, work_state: WorkState) -> anyhow::Result<()> {
        self.job_board.work_state = work_state;
        self.save().context("Unable to save after setting work state.")?;
        Ok(())
    }
}
