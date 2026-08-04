use super::*;
use std::time::Duration;

/// Polls `receiver` without blocking the runtime thread, so the signal task can still be driven
/// while the test waits on it.
#[cfg(unix)]
async fn recv(
    receiver: &mut mpsc::Receiver<Event>,
    timeout: Duration,
) -> Result<Event, mpsc::RecvTimeoutError> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match receiver.try_recv() {
            Ok(event) => return Ok(event),
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(mpsc::RecvTimeoutError::Disconnected);
            }
            Err(mpsc::TryRecvError::Empty) if std::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(mpsc::TryRecvError::Empty) => return Err(mpsc::RecvTimeoutError::Timeout),
        }
    }
}

/// SAFETY: `raise` on a signal the watcher has a handler installed for, or (SIGCONT) one whose
/// default action for a process that isn't stopped is nothing at all.
#[cfg(unix)]
fn raise(signal: libc::c_int) {
    unsafe { libc::raise(signal) };
}

/// Every case in one test, and one test only: a raised signal goes to the whole process, so two
/// of these running concurrently would each answer the other's.
#[cfg(unix)]
#[tokio::test]
async fn terminal_signals_are_answered_only_when_the_app_holds_the_terminal() {
    let suspended = Arc::new(AtomicBool::new(false));
    let (sender, mut receiver) = mpsc::channel();
    watch_terminal_signals(sender, suspended.clone());

    // Wait until the handlers are actually installed, which a SIGCONT the watcher answers proves:
    // the task registers all three together before it starts receiving, and a SIGINT raised
    // before that point would find the default disposition and kill the test binary.
    let mut ready = false;
    for _ in 0..50 {
        raise(libc::SIGCONT);
        if let Ok(Event::Resumed) = recv(&mut receiver, Duration::from_millis(100)).await {
            ready = true;
            break;
        }
    }
    assert!(ready, "the signal watcher never came up");

    // ctrl-z then `fg` leaves raw mode and the alt screen behind, so the app is asked to take the
    // terminal back.
    raise(libc::SIGCONT);
    let event = recv(&mut receiver, Duration::from_secs(5)).await;
    assert!(
        matches!(event, Ok(Event::Resumed)),
        "expected a Resumed event, got {event:?}"
    );

    // An interrupt with no child in the way is an outside `kill -INT`: quit, don't die.
    raise(libc::SIGINT);
    let event = recv(&mut receiver, Duration::from_secs(5)).await;
    assert!(
        matches!(event, Ok(Event::App(AppEvent::Quit))),
        "expected a Quit event, got {event:?}"
    );

    // While an editor is up, all of it belongs to the editor: ctrl-c and ctrl-\ at its prompt are
    // delivered to this process too, and `fg` continues the whole group.
    suspended.store(true, Ordering::SeqCst);
    raise(libc::SIGINT);
    raise(libc::SIGQUIT);
    raise(libc::SIGCONT);
    let event = recv(&mut receiver, Duration::from_millis(500)).await;
    assert!(event.is_err(), "expected no event, got {event:?}");
}
