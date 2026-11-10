//! Loopers, messengers, timers, tasks and answers.

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use olo_app::{Handler, Latest, Looper, ask, pool};

struct Counter {
    seen: Rc<RefCell<Vec<u32>>>,
}

enum Count {
    Add(u32),
    Quit,
}

impl Handler for Counter {
    type Message = Count;

    fn message_received(&mut self, message: Count, looper: &mut Looper) {
        match message {
            Count::Add(n) => self.seen.borrow_mut().push(n),
            Count::Quit => looper.quit(),
        }
    }
}

/// Messages from another thread arrive in the order sent, on the
/// looper's thread, and the looper ends when told.
#[test]
fn messages_arrive_in_order() {
    let mut looper = Looper::new("test");
    let seen = Rc::new(RefCell::new(Vec::new()));
    let id = looper.add_handler(Counter { seen: seen.clone() });
    let messenger = looper.messenger::<Counter>(id);
    let sender = std::thread::spawn(move || {
        for n in 0..1000 {
            assert!(messenger.send(Count::Add(n)));
        }
        messenger.send(Count::Quit);
    });
    looper.run();
    sender.join().unwrap();
    assert_eq!(*seen.borrow(), (0..1000).collect::<Vec<_>>());
}

/// A messenger to a looper that has ended says so, and nothing hangs.
#[test]
fn a_looper_gone_takes_nothing() {
    let handle = Looper::spawn("short", |looper| looper.quit());
    let started = Instant::now();
    while !handle.is_gone() {
        assert!(started.elapsed() < Duration::from_secs(5), "the looper never ended");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!handle.post(|_| panic!("ran on a looper that had ended")));
}

/// What a looper defers runs before what other threads queued, in the
/// order deferred.
#[test]
fn deferred_goes_first() {
    let mut looper = Looper::new("test");
    let order = Rc::new(RefCell::new(Vec::new()));
    let handle = looper.handle();
    let (o1, o2, o3) = (order.clone(), order.clone(), order.clone());
    // (Posted through the handle, as another thread would: Send, so it
    // tells the test by a channel.)
    let (tx, rx) = mpsc::channel();
    handle.post(move |_| tx.send("posted").unwrap());
    looper.defer(move |looper| {
        o1.borrow_mut().push("first");
        looper.defer(move |_| o2.borrow_mut().push("third"));
    });
    looper.defer(move |_| o3.borrow_mut().push("second"));
    looper.run_until_dry();
    assert_eq!(*order.borrow(), ["first", "second", "third"]);
    assert_eq!(rx.try_recv(), Ok("posted"));
}

/// Timers run when due, in order; a repeating one until cancelled; and
/// the looper sleeps between them rather than spinning.
#[test]
fn timers_run_when_due() {
    let mut looper = Looper::new("test");
    let log = Rc::new(RefCell::new(Vec::new()));
    let started = Instant::now();
    let (a, b, c) = (log.clone(), log.clone(), log.clone());
    looper.after(Duration::from_millis(30), move |_| a.borrow_mut().push(("late", started.elapsed())));
    looper.after(Duration::from_millis(10), move |_| b.borrow_mut().push(("early", started.elapsed())));
    let ticks = Rc::new(RefCell::new(0));
    let t = ticks.clone();
    let every = Rc::new(RefCell::new(None));
    let e = every.clone();
    *every.borrow_mut() = Some(looper.every(Duration::from_millis(5), move |looper| {
        *t.borrow_mut() += 1;
        if *t.borrow() == 3 {
            looper.cancel(e.borrow().unwrap());
        }
    }));
    looper.after(Duration::from_millis(60), move |looper| {
        c.borrow_mut().push(("end", started.elapsed()));
        looper.quit();
    });
    looper.run();
    let log = log.borrow();
    assert_eq!(log.iter().map(|(n, _)| *n).collect::<Vec<_>>(), ["early", "late", "end"]);
    assert!(log[0].1 >= Duration::from_millis(10) && log[1].1 >= Duration::from_millis(30));
    assert_eq!(*ticks.borrow(), 3, "a cancelled timer ran again");
}

/// A looper's task awaits work on the pool and an answer from another
/// looper without blocking either.
#[test]
fn tasks_and_answers() {
    struct Doubler;
    struct Double(u32, olo_app::Reply<u32>);
    impl Handler for Doubler {
        type Message = Double;
        fn message_received(&mut self, Double(n, reply): Double, _: &mut Looper) {
            reply.send(n * 2);
        }
    }
    let (tx, rx) = mpsc::channel();
    let other = Looper::spawn("doubler", move |looper| {
        let id = looper.add_handler(Doubler);
        tx.send(looper.messenger::<Doubler>(id)).unwrap();
    });
    let doubler = rx.recv().unwrap();

    let mut looper = Looper::new("test");
    let handle = looper.handle();
    let result = Rc::new(RefCell::new(None));
    let r = result.clone();
    looper
        .spawn_local(async move {
            let from_pool = pool::run(|| 20 + 1).await;
            let (reply, answer) = ask();
            doubler.send(Double(from_pool, reply));
            *r.borrow_mut() = answer.await;
            handle.quit();
        })
        .detach();
    looper.run();
    assert_eq!(*result.borrow(), Some(42));
    other.quit();

    // A question dropped unanswered ends, with nothing.
    let (reply, answer) = ask::<u32>();
    drop(reply);
    assert_eq!(answer.wait(Duration::from_secs(1)), None);
    // A worker thread may block on one.
    let (reply, answer) = ask::<u32>();
    std::thread::spawn(move || reply.send(7));
    assert_eq!(answer.wait(Duration::from_secs(5)), Some(7));
}

/// A flood into a `Latest` is one message with the last value.
#[test]
fn latest_gathers() {
    let mut looper = Looper::new("test");
    let handle = looper.handle();
    let latest = Latest::new();
    let taken = Arc::new(Mutex::new(Vec::new()));
    for n in 0..100 {
        let taken = taken.clone();
        latest.put(&handle, n, move |_, n| taken.lock().unwrap().push(n));
    }
    assert_eq!(handle.waiting(), 1);
    looper.run_until_dry();
    assert_eq!(*taken.lock().unwrap(), [99]);
    // And again afterwards: the slot is free for the next burst.
    let again = taken.clone();
    latest.put_with(&handle, 1, |a, b| a + b, move |_, n| again.lock().unwrap().push(n));
    let again = taken.clone();
    latest.put_with(&handle, 2, |a, b| a + b, move |_, n| again.lock().unwrap().push(n));
    looper.run_until_dry();
    assert_eq!(*taken.lock().unwrap(), [99, 3]);
}

/// The idle hook runs once the queue is dry, not once a message, and a
/// handler removed while taking a message goes when it's done.
#[test]
fn idle_and_removal() {
    struct Leaver {
        id: Rc<RefCell<Option<olo_app::HandlerId>>>,
        took: Rc<RefCell<u32>>,
    }
    impl Handler for Leaver {
        type Message = ();
        fn message_received(&mut self, _: (), looper: &mut Looper) {
            *self.took.borrow_mut() += 1;
            looper.remove_handler(self.id.borrow().unwrap());
        }
    }
    let mut looper = Looper::new("test");
    let idles = Rc::new(RefCell::new(0));
    let i = idles.clone();
    looper.on_idle(move |_| *i.borrow_mut() += 1);
    let id = Rc::new(RefCell::new(None));
    let took = Rc::new(RefCell::new(0));
    let handler = looper.add_handler(Leaver { id: id.clone(), took: took.clone() });
    *id.borrow_mut() = Some(handler);
    for _ in 0..10 {
        let h = handler;
        looper.defer(move |looper| {
            looper.deliver::<Leaver>(h, ());
        });
    }
    looper.run_until_dry();
    assert_eq!(*took.borrow(), 1, "a removed handler took another message");
    assert_eq!(*idles.borrow(), 1, "the idle hook ran more than once for one burst");
    assert!(looper.handler::<Leaver>(handler).is_none());
}

/// Pool threads come with work and go without.
#[test]
fn the_pool_starts_threads_for_work() {
    let (tx, rx) = mpsc::channel();
    let tasks: Vec<_> = (0..4)
        .map(|n| {
            let tx = tx.clone();
            pool::run(move || {
                std::thread::sleep(Duration::from_millis(20));
                tx.send(n).unwrap();
            })
        })
        .collect();
    let mut got: Vec<i32> = (0..4).map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap()).collect();
    got.sort();
    assert_eq!(got, [0, 1, 2, 3]);
    assert!(pool::threads() >= 1);
    for t in tasks {
        t.detach();
    }
}
