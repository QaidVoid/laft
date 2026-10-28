use std::sync::mpsc;
use std::time::Duration;

use crate::Error;
use crate::test_support::Env;

#[test]
fn busy_lock_names_its_holder() {
    let env = Env::new();
    let held = env.root.lock("apply").expect("the lock is free");
    match env.root.try_lock("gc") {
        Err(Error::Busy(Some(holder))) => {
            assert_eq!(holder.pid, std::process::id());
            assert_eq!(holder.command, "apply");
        }
        other => panic!("expected a busy lock, got {other:?}"),
    }
    drop(held);
    env.root.try_lock("gc").expect("the lock is free again");
}

#[test]
fn lock_waits_until_the_holder_releases_it() {
    let env = Env::new();
    let held = env.root.lock("apply").expect("the lock is free");
    let (sender, receiver) = mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let lock = env.root.lock("gc").expect("the lock is taken eventually");
            sender.send(()).expect("the test waits");
            drop(lock);
        });
        assert!(receiver.recv_timeout(Duration::from_millis(200)).is_err());
        drop(held);
        receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("the waiter gets the lock");
    });
}
