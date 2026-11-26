//! The transfer queue (Sftp.dc.html, "Transfers"): files from planned
//! transfers, run a few at a time (Settings, "Transfers at once"), with
//! pause, resume, retry and remove, progress, rate and time left.
//!
//! A copy that loses the connection is **interrupted**, not failed: the
//! engine stops starting copies and reports [`EngineEvent::ConnectionLost`];
//! once the tab reconnects and hands over a new [`Remote`], interrupted
//! copies queue again and resume where they stopped.
//!
//! [`Queue`] is the plain state (tested without I/O); [`Engine`] runs it on
//! Tokio. The interface reads [`Engine::snapshot`] whenever an event says
//! something changed.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;

use crate::RemotePath;
use crate::format;
use crate::plan::{Direction, FileJob};
use crate::remote::Remote;
use crate::transfer::{self, CANCEL, Control, CopyError, Outcome, PAUSE, RUN, Stamp};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Active,
    Paused,
    /// The connection went; resumes after reconnecting.
    Interrupted,
    Failed(String),
    Done,
}

#[derive(Clone, Debug)]
pub struct Job {
    pub id: JobId,
    pub direction: Direction,
    pub name: String,
    pub local: PathBuf,
    pub remote: RemotePath,
    pub size: u64,
    pub done: u64,
    pub state: JobState,
    /// Bytes per second, smoothed; 0 until known.
    pub rate: f64,
    pub finished_at: Option<Instant>,
    stamp: Option<Stamp>,
    sample: Option<(Instant, u64)>,
}

impl Job {
    pub fn percent(&self) -> u8 {
        if self.size == 0 {
            return if self.state == JobState::Done { 100 } else { 0 };
        }
        ((self.done.min(self.size) * 100) / self.size) as u8
    }

    /// "64% · 4.2 MB/s · 4 s left" for an active copy (Sftp.dc.html).
    pub fn progress_text(&self) -> String {
        let mut parts = vec![format!("{}%", self.percent())];
        if self.rate >= 1.0 {
            parts.push(format::rate(self.rate));
        }
        if let Some(left) = format::time_left(self.size.saturating_sub(self.done), self.rate) {
            parts.push(left);
        }
        parts.join(" · ")
    }
}

/// The jobs and their states, without I/O.
#[derive(Default)]
pub struct Queue {
    jobs: Vec<Job>,
    next_id: u64,
}

impl Queue {
    pub fn add(&mut self, files: Vec<FileJob>) -> Vec<JobId> {
        files
            .into_iter()
            .map(|f| {
                self.next_id += 1;
                let id = JobId(self.next_id);
                self.jobs.push(Job {
                    id,
                    direction: f.direction,
                    name: f.name,
                    local: f.local,
                    remote: f.remote,
                    size: f.size,
                    done: 0,
                    state: JobState::Queued,
                    rate: 0.0,
                    finished_at: None,
                    stamp: None,
                    sample: None,
                });
                id
            })
            .collect()
    }

    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    pub fn get(&self, id: JobId) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }

    fn get_mut(&mut self, id: JobId) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    fn count(&self, f: impl Fn(&JobState) -> bool) -> usize {
        self.jobs.iter().filter(|j| f(&j.state)).count()
    }

    /// The next queued jobs to start, in order, to fill `at_once`.
    pub fn to_start(&self, at_once: usize) -> Vec<JobId> {
        let free = at_once.saturating_sub(self.count(|s| *s == JobState::Active));
        self.jobs.iter().filter(|j| j.state == JobState::Queued).take(free).map(|j| j.id).collect()
    }

    /// "2 active · 1 queued" (Sftp.dc.html), or "Idle".
    pub fn summary(&self) -> String {
        let parts: Vec<String> = [
            (self.count(|s| *s == JobState::Active), "active"),
            (self.count(|s| *s == JobState::Paused), "paused"),
            (self.count(|s| *s == JobState::Queued), "queued"),
            (self.count(|s| *s == JobState::Interrupted), "waiting to reconnect"),
            (self.count(|s| matches!(s, JobState::Failed(_))), "failed"),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, what)| format!("{n} {what}"))
        .collect();
        if parts.is_empty() { "Idle".into() } else { parts.join(" · ") }
    }

    pub fn finished(&self) -> usize {
        self.count(|s| *s == JobState::Done)
    }

    /// Bytes done so far; the rate is smoothed over samples a quarter of a
    /// second or more apart.
    fn record(&mut self, id: JobId, done: u64, now: Instant) {
        let Some(job) = self.get_mut(id) else { return };
        job.done = done;
        match job.sample {
            None => job.sample = Some((now, done)),
            Some((at, before)) => {
                let dt = now.duration_since(at).as_secs_f64();
                if dt >= 0.25 {
                    let instant = done.saturating_sub(before) as f64 / dt;
                    job.rate = if job.rate == 0.0 { instant } else { job.rate * 0.7 + instant * 0.3 };
                    job.sample = Some((now, done));
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineEvent {
    /// Something in the queue changed: read [`Engine::snapshot`].
    Changed,
    /// A copy lost the connection. Copies wait for [`Engine::set_remote`].
    ConnectionLost,
}

struct Inner {
    queue: Queue,
    remote: Option<Arc<Remote>>,
    at_once: usize,
    controls: HashMap<JobId, Arc<Control>>,
    last_notice: HashMap<JobId, Instant>,
}

/// Runs the queue. Cheap to clone; clones share the queue.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Mutex<Inner>>,
    runtime: tokio::runtime::Handle,
    events: UnboundedSender<EngineEvent>,
}

/// How often an active copy's progress is reported.
const NOTICE_EVERY: Duration = Duration::from_millis(100);

impl Engine {
    pub fn new(runtime: tokio::runtime::Handle, at_once: usize, events: UnboundedSender<EngineEvent>) -> Self {
        let inner = Inner {
            queue: Queue::default(),
            remote: None,
            at_once: at_once.max(1),
            controls: HashMap::new(),
            last_notice: HashMap::new(),
        };
        Self { inner: Arc::new(Mutex::new(inner)), runtime, events }
    }

    fn changed(&self) {
        let _ = self.events.send(EngineEvent::Changed);
    }

    /// The connection to copy over; `None` while it is down. A new one
    /// queues interrupted copies again.
    pub fn set_remote(&self, remote: Option<Arc<Remote>>) {
        {
            let mut g = self.inner.lock().unwrap();
            let up = remote.is_some();
            g.remote = remote;
            if up {
                for job in g.queue.jobs.iter_mut().filter(|j| j.state == JobState::Interrupted) {
                    job.state = JobState::Queued;
                }
            }
        }
        self.changed();
        self.pump();
    }

    pub fn set_at_once(&self, n: usize) {
        self.inner.lock().unwrap().at_once = n.max(1);
        self.pump();
    }

    /// Queue a planned transfer's files (after [`crate::plan::Plan::create_folders`]).
    pub fn add(&self, files: Vec<FileJob>) -> Vec<JobId> {
        let ids = self.inner.lock().unwrap().queue.add(files);
        self.changed();
        self.pump();
        ids
    }

    pub fn snapshot(&self) -> Vec<Job> {
        self.inner.lock().unwrap().queue.jobs.clone()
    }

    pub fn summary(&self) -> String {
        self.inner.lock().unwrap().queue.summary()
    }

    pub fn pause(&self, id: JobId) {
        let mut g = self.inner.lock().unwrap();
        let control = g.controls.get(&id).cloned();
        if let Some(job) = g.queue.get_mut(id) {
            match job.state {
                JobState::Active => {
                    if let Some(c) = control {
                        c.store(PAUSE, Ordering::Relaxed);
                    }
                }
                JobState::Queued => job.state = JobState::Paused,
                _ => {}
            }
        }
        drop(g);
        self.changed();
    }

    pub fn resume(&self, id: JobId) {
        {
            let mut g = self.inner.lock().unwrap();
            if let Some(job) = g.queue.get_mut(id)
                && matches!(job.state, JobState::Paused | JobState::Interrupted)
            {
                job.state = JobState::Queued;
            }
        }
        self.changed();
        self.pump();
    }

    /// Start a failed copy again from nothing.
    pub fn retry(&self, id: JobId) {
        {
            let mut g = self.inner.lock().unwrap();
            if let Some(job) = g.queue.get_mut(id)
                && matches!(job.state, JobState::Failed(_))
            {
                job.state = JobState::Queued;
                job.stamp = None;
                job.done = 0;
                job.rate = 0.0;
                job.sample = None;
                if job.direction == Direction::Download {
                    let _ = std::fs::remove_file(transfer::part_path(&job.local));
                }
            }
        }
        self.changed();
        self.pump();
    }

    /// Remove a job; an active one is cancelled first. A download's part
    /// file goes with it.
    pub fn remove(&self, id: JobId) {
        let mut g = self.inner.lock().unwrap();
        if let Some(c) = g.controls.get(&id) {
            c.store(CANCEL, Ordering::Relaxed);
            return;
        }
        if let Some(i) = g.queue.jobs.iter().position(|j| j.id == id) {
            let job = g.queue.jobs.remove(i);
            if job.direction == Direction::Download && job.state != JobState::Done {
                let _ = std::fs::remove_file(transfer::part_path(&job.local));
            }
        }
        drop(g);
        self.changed();
    }

    /// "Hide finished": drop completed jobs.
    pub fn clear_finished(&self) {
        self.inner.lock().unwrap().queue.jobs.retain(|j| j.state != JobState::Done);
        self.changed();
    }

    fn on_progress(&self, id: JobId, done: u64) {
        let now = Instant::now();
        let notify = {
            let mut g = self.inner.lock().unwrap();
            g.queue.record(id, done, now);
            let due = g.last_notice.get(&id).is_none_or(|t| now.duration_since(*t) >= NOTICE_EVERY);
            if due {
                g.last_notice.insert(id, now);
            }
            due
        };
        if notify {
            self.changed();
        }
    }

    /// Start queued jobs while there is room and a connection.
    fn pump(&self) {
        let mut g = self.inner.lock().unwrap();
        let Some(remote) = g.remote.clone() else { return };
        for id in g.queue.to_start(g.at_once) {
            let control = Arc::new(Control::new(RUN));
            g.controls.insert(id, control.clone());
            let Some(job) = g.queue.get_mut(id) else { continue };
            job.state = JobState::Active;
            job.sample = None;
            let (direction, local, path, mut stamp) = (job.direction, job.local.clone(), job.remote.clone(), job.stamp);
            let (engine, remote) = (self.clone(), remote.clone());
            self.runtime.spawn(async move {
                let progress = {
                    let engine = engine.clone();
                    move |done| engine.on_progress(id, done)
                };
                let result = match direction {
                    Direction::Upload => transfer::upload(&remote, &local, &path, &mut stamp, &control, progress).await,
                    Direction::Download => {
                        transfer::download(&remote, &path, &local, &mut stamp, &control, progress).await
                    }
                };
                let result = match result {
                    Err(e) => Err(transfer::classify(&remote, e).await),
                    ok => ok,
                };
                engine.finish(id, stamp, result, control.load(Ordering::Relaxed));
            });
        }
        drop(g);
        self.changed();
    }

    fn finish(&self, id: JobId, stamp: Option<Stamp>, result: Result<Outcome, CopyError>, control: u8) {
        let mut lost = false;
        {
            let mut g = self.inner.lock().unwrap();
            g.controls.remove(&id);
            g.last_notice.remove(&id);
            let cancelled = control == CANCEL;
            if cancelled {
                if let Some(i) = g.queue.jobs.iter().position(|j| j.id == id) {
                    let job = g.queue.jobs.remove(i);
                    if job.direction == Direction::Download {
                        let _ = std::fs::remove_file(transfer::part_path(&job.local));
                    }
                }
            } else if let Some(job) = g.queue.get_mut(id) {
                job.stamp = stamp;
                job.rate = 0.0;
                match result {
                    Ok(Outcome::Done) => {
                        job.state = JobState::Done;
                        job.done = job.size.max(job.done);
                        if let Some(s) = stamp {
                            job.size = s.size;
                            job.done = s.size;
                        }
                        job.finished_at = Some(Instant::now());
                    }
                    Ok(Outcome::Stopped) => job.state = JobState::Paused,
                    Err(e) if e.is_lost() => {
                        job.state = JobState::Interrupted;
                        lost = true;
                    }
                    Err(e) => job.state = JobState::Failed(e.to_string()),
                }
            }
            if lost && g.remote.is_some() {
                g.remote = None;
            } else {
                lost = false;
            }
        }
        if lost {
            let _ = self.events.send(EngineEvent::ConnectionLost);
        }
        self.changed();
        self.pump();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(n: usize) -> Vec<FileJob> {
        (0..n)
            .map(|i| FileJob {
                direction: Direction::Upload,
                local: PathBuf::from(format!("C:\\f{i}")),
                remote: RemotePath::new(format!("/f{i}").into_bytes()),
                name: format!("f{i}"),
                size: 1000,
            })
            .collect()
    }

    #[test]
    fn starts_in_order_up_to_the_limit() {
        let mut q = Queue::default();
        let ids = q.add(files(5));
        assert_eq!(q.to_start(2), ids[..2]);
        for id in &ids[..2] {
            q.get_mut(*id).unwrap().state = JobState::Active;
        }
        assert!(q.to_start(2).is_empty());
        assert_eq!(q.to_start(3), [ids[2]]);
        q.get_mut(ids[2]).unwrap().state = JobState::Paused;
        assert_eq!(q.to_start(3), [ids[3]]);
        assert_eq!(q.summary(), "2 active · 1 paused · 2 queued");
    }

    #[test]
    fn summary_reads_idle_and_counts_each_state() {
        let mut q = Queue::default();
        assert_eq!(q.summary(), "Idle");
        let ids = q.add(files(3));
        q.get_mut(ids[0]).unwrap().state = JobState::Done;
        q.get_mut(ids[1]).unwrap().state = JobState::Failed("Permission denied.".into());
        q.get_mut(ids[2]).unwrap().state = JobState::Interrupted;
        assert_eq!(q.summary(), "1 waiting to reconnect · 1 failed");
        assert_eq!(q.finished(), 1);
    }

    #[test]
    fn rate_is_smoothed_and_progress_reads_as_drawn() {
        let mut q = Queue::default();
        let id = q.add(files(1))[0];
        q.get_mut(id).unwrap().size = 10 * 1_048_576;
        let t0 = Instant::now();
        q.record(id, 0, t0);
        q.record(id, 1_048_576, t0 + Duration::from_millis(100)); // too soon to sample
        assert_eq!(q.get(id).unwrap().rate, 0.0);
        q.record(id, 2 * 1_048_576, t0 + Duration::from_secs(1));
        let job = q.get(id).unwrap();
        assert!((job.rate - 2.0 * 1_048_576.0).abs() < 1.0);
        assert_eq!(job.percent(), 20);
        assert_eq!(job.progress_text(), "20% · 2.0 MB/s · 4 s left");
    }
}
