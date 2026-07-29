use std::cell::Cell;
use std::fmt::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

static ENABLED: AtomicBool = AtomicBool::new(false);
static NEXT_SEQUENCE: AtomicUsize = AtomicUsize::new(0);
static EVENTS: Mutex<Vec<ProfileEvent>> = Mutex::new(Vec::new());

thread_local! {
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}

struct ProfileEvent {
    sequence: usize,
    depth: usize,
    thread_id: std::thread::ThreadId,
    name: &'static str,
    elapsed: Duration,
}

pub(crate) struct ProfileGuard {
    event: Option<ActiveEvent>,
}

struct ActiveEvent {
    sequence: usize,
    depth: usize,
    thread_id: std::thread::ThreadId,
    name: &'static str,
    started: Instant,
}

pub(crate) fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
    NEXT_SEQUENCE.store(0, Ordering::Relaxed);
    DEPTH.with(|depth| depth.set(0));
    events().clear();
}

pub(crate) fn scope(name: &'static str) -> ProfileGuard {
    if !ENABLED.load(Ordering::Relaxed) {
        return ProfileGuard { event: None };
    }
    let sequence = NEXT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let depth = DEPTH.with(|depth| {
        let current = depth.get();
        depth.set(current + 1);
        current
    });
    ProfileGuard {
        event: Some(ActiveEvent {
            sequence,
            depth,
            thread_id: std::thread::current().id(),
            name,
            started: Instant::now(),
        }),
    }
}

impl Drop for ProfileGuard {
    fn drop(&mut self) {
        let Some(event) = self.event.take() else {
            return;
        };
        let elapsed = event.started.elapsed();
        DEPTH.with(|depth| depth.set(event.depth));
        events().push(ProfileEvent {
            sequence: event.sequence,
            depth: event.depth,
            thread_id: event.thread_id,
            name: event.name,
            elapsed,
        });
    }
}

pub(crate) fn print_report() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let mut events = std::mem::take(&mut *events());
    events.sort_by_key(|event| event.sequence);
    let mut report = String::from("profile (inclusive elapsed):\n");
    let mut threads: Vec<(std::thread::ThreadId, Vec<ProfileEvent>)> = Vec::new();
    for event in events {
        if let Some((_, thread_events)) = threads
            .iter_mut()
            .find(|(thread_id, _)| *thread_id == event.thread_id)
        {
            thread_events.push(event);
        } else {
            threads.push((event.thread_id, vec![event]));
        }
    }
    let multiple_threads = threads.len() > 1;
    for (thread_id, thread_events) in threads {
        if multiple_threads {
            let _ = writeln!(report, "  thread {thread_id:?}:");
        }
        for event in thread_events {
            write_event(&mut report, &event, usize::from(multiple_threads));
        }
    }
    eprint!("{report}");
}

fn write_event(report: &mut String, event: &ProfileEvent, extra_depth: usize) {
    let _ = writeln!(
        report,
        "  {:indent$}{:<42} {:>10}",
        "",
        event.name,
        format_duration(event.elapsed),
        indent = (event.depth + extra_depth) * 2,
    );
}

fn events() -> std::sync::MutexGuard<'static, Vec<ProfileEvent>> {
    EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn format_duration(duration: Duration) -> String {
    if duration.as_secs() != 0 {
        format!("{:.3} s", duration.as_secs_f64())
    } else if duration.as_millis() != 0 {
        format!("{:.3} ms", duration.as_secs_f64() * 1_000.0)
    } else {
        format!("{:.3} us", duration.as_secs_f64() * 1_000_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_thread_events_are_collected() {
        set_enabled(true);
        std::thread::spawn(|| {
            let _profile = scope("worker");
        })
        .join()
        .unwrap();

        assert!(events().iter().any(|event| event.name == "worker"));
        set_enabled(false);
    }
}
