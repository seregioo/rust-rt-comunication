#[cfg(all(feature = "preempt_rt", feature = "xenomai"))]
compile_error!("`preempt_rt` and `xenomai` features cannot be enabled simultaneously.");

#[cfg(feature = "preempt_rt")]
mod preempt_rt {
    use libc::{
        CLOCK_MONOTONIC, SCHED_FIFO, SYS_gettid, TIMER_ABSTIME, clock_gettime, clock_nanosleep,
        sched_param, sched_setscheduler, syscall, timespec,
    };
    use std::ptr;
    use std::time::Duration;

    pub struct PreemptRt;

    impl PreemptRt {
        pub fn prepare() -> Result<(), String> {
            unsafe {
                let tid = syscall(SYS_gettid) as i32;
                let param = sched_param { sched_priority: 99 };
                let ret = sched_setscheduler(tid, SCHED_FIFO, &param);
                if ret != 0 {
                    let err = std::io::Error::last_os_error();
                    return Err(format!("Realtime scheduling unavailable: {err}"));
                }
            }
            Ok(())
        }

        pub fn init_sleep(period: Duration) -> timespec {
            let mut now = timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            unsafe {
                clock_gettime(CLOCK_MONOTONIC, &mut now);
            }
            add_duration(&mut now, period);
            now
        }

        pub fn sleep(period: Duration, next: &mut timespec) {
            if period.is_zero() {
                return;
            }
            unsafe {
                clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, next, ptr::null_mut());
            }
            add_duration(next, period);
        }
    }

    fn add_duration(target: &mut timespec, duration: Duration) {
        let secs = duration.as_secs() as i64;
        let nanos = duration.subsec_nanos() as i64;
        target.tv_sec += secs;
        target.tv_nsec += nanos;
        if target.tv_nsec >= 1_000_000_000 {
            target.tv_sec += target.tv_nsec / 1_000_000_000;
            target.tv_nsec %= 1_000_000_000;
        }
    }
}

#[cfg(all(not(feature = "preempt_rt"), not(feature = "xenomai")))]
mod normal_thread {
    use libc::{CLOCK_MONOTONIC, TIMER_ABSTIME, clock_gettime, clock_nanosleep, timespec};
    use std::ptr;
    use std::time::Duration;

    pub struct NormalThread;

    impl NormalThread {
        pub fn prepare() -> Result<(), String> {
            Ok(())
        }

        pub fn init_sleep(period: Duration) -> timespec {
            let mut now = timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            unsafe {
                clock_gettime(CLOCK_MONOTONIC, &mut now);
            }
            add_duration(&mut now, period);
            now
        }

        pub fn sleep(period: Duration, next: &mut timespec) {
            if period.is_zero() {
                return;
            }
            unsafe {
                clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, next, ptr::null_mut());
            }
            add_duration(next, period);
        }
    }

    fn add_duration(target: &mut timespec, duration: Duration) {
        let secs = duration.as_secs() as i64;
        let nanos = duration.subsec_nanos() as i64;
        target.tv_sec += secs;
        target.tv_nsec += nanos;
        if target.tv_nsec >= 1_000_000_000 {
            target.tv_sec += target.tv_nsec / 1_000_000_000;
            target.tv_nsec %= 1_000_000_000;
        }
    }
}

#[cfg(all(not(feature = "preempt_rt"), feature = "xenomai"))]
mod xenomai {
    use libc::timespec;
    use std::time::Duration;

    pub struct XenomaiRt;

    impl XenomaiRt {
        pub fn prepare() -> Result<(), String> {
            Err("Xenomai backend is not implemented yet.".to_string())
        }

        pub fn init_sleep(_period: Duration) -> timespec {
            timespec {
                tv_sec: 0,
                tv_nsec: 0,
            }
        }

        pub fn sleep(_period: Duration, _next: &mut timespec) {
            // Placeholder; default to no-op
        }
    }
}

#[cfg(all(not(feature = "preempt_rt"), not(feature = "xenomai")))]
pub use normal_thread::NormalThread as ActiveRtBackend;
#[cfg(feature = "preempt_rt")]
pub use preempt_rt::PreemptRt as ActiveRtBackend;
#[cfg(all(not(feature = "preempt_rt"), feature = "xenomai"))]
pub use xenomai::XenomaiRt as ActiveRtBackend;

use std::{sync::mpsc, thread};

pub(crate) struct RuntimeThread;

impl RuntimeThread {
    pub(crate) fn spawn<F>(f: F) -> Result<thread::JoinHandle<()>, String>
    where
        F: FnOnce() + Send + 'static,
    {
        let (status_tx, status_rx) = mpsc::sync_channel(1);
        let handle = thread::spawn(move || {
            let status = ActiveRtBackend::prepare();
            let _ = status_tx.send(status.clone());
            if status.is_err() {
                return;
            }
            f();
        });

        match status_rx.recv() {
            Ok(Ok(())) => Ok(handle),
            Ok(Err(err)) => {
                let _ = handle.join();
                Err(err)
            }
            Err(_) => {
                let _ = handle.join();
                Err("Realtime thread failed to report status".to_string())
            }
        }
    }
}
