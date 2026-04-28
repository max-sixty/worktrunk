use lazy_static::lazy_static;
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, SigmaskHow, Signal};
use nix::sys::signal::{pthread_sigmask, sigaction};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

lazy_static! {
    static ref NOTIFIER_COUNTER: AtomicUsize = AtomicUsize::new(1);
    static ref NOTIFIER: Mutex<HashMap<usize, Sender<()>>> = Mutex::new(HashMap::new());
}

static ONCE: Once = Once::new();

pub fn initialize_signals() {
    ONCE.call_once(listen_sigwinch);
}

pub fn notify_on_sigwinch() -> (usize, Receiver<()>) {
    let (tx, rx) = channel();
    let new_id = NOTIFIER_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut notifiers = NOTIFIER.lock().unwrap();
    notifiers.entry(new_id).or_insert(tx);
    (new_id, rx)
}

pub fn unregister_sigwinch(id: usize) -> Option<Sender<()>> {
    let mut notifiers = NOTIFIER.lock().unwrap();
    notifiers.remove(&id)
}

extern "C" fn handle_sigwiwnch(_: i32) {}

fn listen_sigwinch() {
    let (tx_sig, rx_sig) = channel();

    // register terminal resize event, `pthread_sigmask` should be run before any thread.
    let mut sigset = SigSet::empty();
    sigset.add(Signal::SIGWINCH);
    let _ = pthread_sigmask(SigmaskHow::SIG_BLOCK, Some(&sigset), None);

    // SIGWINCH is ignored by mac by default, thus we need to register an empty handler
    let action = SigAction::new(SigHandler::Handler(handle_sigwiwnch), SaFlags::empty(), SigSet::empty());

    unsafe {
        let _ = sigaction(Signal::SIGWINCH, &action);
    }

    thread::spawn(move || {
        // listen to the resize event;
        loop {
            let _errno = sigset.wait();
            let _ = tx_sig.send(());
        }
    });

    thread::spawn(move || {
        while rx_sig.recv().is_ok() {
            let notifiers = NOTIFIER.lock().unwrap();
            for (_, sender) in notifiers.iter() {
                let _ = sender.send(());
            }
        }
    });
}
