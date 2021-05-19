use chrono::{serde::ts_seconds, DateTime, Local, Utc};

use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    path::Path,
};

extern crate clap;

use std::default::Default;

use crate::{default, Job, StringMatch};

type JobStack = Vec<Job>;

// todo - whole struct private
#[derive(Serialize, Deserialize, Clone)]
pub struct SuspendedStack {
    pub data: JobStack,
    pub reason: String,
    #[serde(with = "ts_seconds")]
    pub date_suspended: DateTime<Utc>,
    pub timer: Option<DateTime<Utc>>,
    pub last_notifiaction: Option<DateTime<Utc>>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct JobBoard {
    // todo - private
    pub active_stack: JobStack,
    // todo - private
    pub suspended_stacks: Vec<SuspendedStack>,
}

impl JobBoard {
    #[allow(dead_code)]
    fn empty() -> Self {
        JobBoard {
            active_stack: default(),
            suspended_stacks: default(),
        }
    }

    pub fn load(app_dir: &Path) -> Self {
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

    fn suspend_at(
        &mut self,
        index: usize,
        reason: String,
        timer: Option<DateTime<Utc>>,
    ) -> Result<(), ()> {
        if index >= self.active_stack.len() {
            return Err(());
        }
        let jobs_to_suspend = self.active_stack.split_off(index);
        let suspended_stack = SuspendedStack {
            data: jobs_to_suspend,
            reason,
            date_suspended: Utc::now(),
            timer,
            last_notifiaction: None,
        };
        self.add_suspended_stack(suspended_stack);
        Ok(())
    }

    pub fn suspend_matching(
        &mut self,
        pattern: impl StringMatch,
        reason: String,
        timer: Option<DateTime<Utc>>,
    ) -> Result<(), ()> {
        if let Some((i, _job)) = self.find_job(pattern) {
            self.suspend_at(i, reason, timer)
        } else {
            Err(())
        }
    }

    // todo - private
    pub fn sort_suspended_stacks(&mut self) {
        let now = Utc::now();
        self.suspended_stacks.sort_by(|stack1, stack2| {
            let timer1 = stack1.timer.unwrap_or(now);
            let timer2 = stack2.timer.unwrap_or(now);
            timer1.cmp(&timer2)
        })
    }

    // todo - private
    pub fn add_suspended_stack(&mut self, stack: SuspendedStack) {
        self.suspended_stacks.push(stack);
        self.sort_suspended_stacks();
    }

    pub fn resume_matching(&mut self, mut pattern: impl StringMatch) -> Result<(), ()> {
        let mut found_index = self.suspended_stacks.len();
        for (i, stack) in self.suspended_stacks.iter().enumerate() {
            if pattern(&stack.data[0].label) {
                found_index = i;
                break;
            }
        }
        self.resume_at_index(found_index)
    }

    pub fn resume_at_index(&mut self, index: usize) -> Result<(), ()> {
        if index >= self.suspended_stacks.len() {
            Err(())
        } else {
            let mut suspended_stack = self.suspended_stacks.remove(index);
            for mut job in &mut suspended_stack.data {
                job.begin_date = Utc::now();
            }
            self.active_stack.extend(suspended_stack.data);
            Ok(())
        }
    }

    pub fn push(&mut self, job: Job) {
        self.active_stack.push(job);
    }

    pub fn pop(&mut self) -> Option<Job> {
        self.active_stack.pop()
    }

    fn num_active_jobs(&self) -> usize {
        self.active_stack.len()
    }

    // todo - private
    pub fn get_summary(&self) -> String {
        if self.num_active_jobs() == 0 {
            format!("{}", self.empty_stack_message())
        } else {
            self.active_stack
                .iter()
                .map(|job| format!("{}\n", job))
                .collect()
        }
    }

    // todo - private
    pub fn suspended_stack_summary(&self) -> String {
        let mut output = String::new();
        for stack in &self.suspended_stacks {
            for (i, job) in stack.data.iter().enumerate() {
                if i == 0 {
                    if let Some(timer) = stack.timer {
                        let local_time = DateTime::<Local>::from(timer);
                        output.push_str(&format!("{}", local_time.format("%a %F %r")));
                        output.push_str(":  ");
                        output.push_str(&job.label);
                    } else {
                        output.push_str(&job.label);
                        output.push_str(" (suspended at ");
                        output.push_str(&format!(
                            "{}",
                            DateTime::<Local>::from(stack.date_suspended).format("%a %F %r")
                        ));
                        output.push_str(")");
                    }
                } else {
                    output.push_str("    ");
                    output.push_str(&job.label);
                }
                output.push('\n');
            }
        }
        output
    }

    fn suspended_tasks_ready(&self) -> bool {
        let now = Utc::now();
        if let Some(task) = self.suspended_stacks.last() {
            if let Some(timer) = task.timer {
                if timer < now {
                    true
                } else {
                    false
                }
            } else {
                true
            }
        } else {
            false
        }
    }

    pub fn empty_stack_message(&self) -> String {
        let mut output = String::new();
        if self.suspended_tasks_ready() {
            output.push_str("You finished your jobs in progress. Yay! Use `wyd resume` to resume the topmost suspended task:\n");
            output.push_str(&self.suspended_stack_summary())
        } else {
            output.push_str("No jobs in progress, and no suspended tasks! Use `wyd push [some arbitrary label]` to start a new task.")
        }
        output
    }
}
