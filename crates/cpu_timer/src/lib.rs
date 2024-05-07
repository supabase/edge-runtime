pub mod timerid;

#[cfg(target_os = "linux")]
use std::sync::Arc;

use anyhow::Error;
use tokio::sync::mpsc;

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::atomic::AtomicUsize;

    pub use crate::timerid::TimerId;
    pub use anyhow::bail;
    pub use ctor::ctor;
    pub use tokio::sync::Mutex;

    use once_cell::sync::Lazy;
    use tokio::sync::mpsc;

    use crate::CPUTimer;

    pub enum SignalMsg {
        Alarm(usize),
        Add((usize, CPUTimer)),
        Remove(usize),
    }

    type SignalMessageChannel = (
        mpsc::UnboundedSender<SignalMsg>,
        std::sync::Mutex<Option<mpsc::UnboundedReceiver<SignalMsg>>>,
    );

    pub static TIMER_COUNTER: AtomicUsize = AtomicUsize::new(0);
    pub static SIG_MSG_CHAN: Lazy<SignalMessageChannel> = Lazy::new(|| {
        let (sig_msg_tx, sig_msg_rx) = mpsc::unbounded_channel::<SignalMsg>();
        (sig_msg_tx, std::sync::Mutex::new(Some(sig_msg_rx)))
    });
}

#[repr(C)]
#[derive(Clone)]
pub struct CPUAlarmVal {
    pub cpu_alarms_tx: mpsc::UnboundedSender<()>,
}

#[cfg(target_os = "linux")]
struct CPUTimerVal {
    tid: linux::TimerId,
    initial_expiry: u64,
    interval: u64,
}

#[cfg(target_os = "linux")]
unsafe impl Send for CPUTimerVal {}

#[cfg(target_os = "linux")]
#[derive(Clone)]
pub struct CPUTimer {
    id: usize,
    timer: Arc<linux::Mutex<CPUTimerVal>>,
    cpu_alarm_val: Arc<CPUAlarmVal>,
}

#[cfg(target_os = "linux")]
impl Drop for CPUTimer {
    fn drop(&mut self) {
        if Arc::strong_count(&self.timer) == 2 {
            linux::SIG_MSG_CHAN
                .0
                .clone()
                .send(linux::SignalMsg::Remove(self.id))
                .unwrap();
        }
    }
}

#[cfg(not(target_os = "linux"))]
#[derive(Clone)]
pub struct CPUTimer {}

impl CPUTimer {
    #[cfg(target_os = "linux")]
    pub fn start(
        initial_expiry: u64,
        interval: u64,
        cpu_alarm_val: CPUAlarmVal,
    ) -> Result<Self, Error> {
        use std::sync::atomic::Ordering;

        use linux::*;

        let id = TIMER_COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut timerid = TimerId(std::ptr::null_mut());
        let mut sigev: libc::sigevent = unsafe { std::mem::zeroed() };
        let cpu_alarm_val = Arc::new(cpu_alarm_val);

        sigev.sigev_notify = libc::SIGEV_SIGNAL;
        sigev.sigev_signo = libc::SIGALRM;
        sigev.sigev_value = libc::sigval {
            sival_ptr: id as *mut libc::c_void,
        };

        if unsafe {
            // creates a new per-thread timer
            libc::timer_create(
                libc::CLOCK_THREAD_CPUTIME_ID,
                &mut sigev as *mut libc::sigevent,
                &mut timerid.0 as *mut *mut libc::c_void,
            )
        } < 0
        {
            bail!(std::io::Error::last_os_error())
        }

        let this = Self {
            id,
            timer: Arc::new(Mutex::new(CPUTimerVal {
                tid: timerid,
                initial_expiry,
                interval,
            })),
            cpu_alarm_val,
        };

        Ok({
            this.reset()?;

            linux::SIG_MSG_CHAN
                .0
                .clone()
                .send(SignalMsg::Add((id, this.clone())))
                .unwrap();

            this
        })
    }

    #[cfg(target_os = "linux")]
    pub fn reset(&self) -> Result<(), Error> {
        use anyhow::Context;
        use linux::*;

        let timer = self.timer.try_lock().context("failed to get the lock")?;

        let initial_expiry_secs = timer.initial_expiry / 1000;
        let initial_expiry_msecs = timer.initial_expiry % 1000;
        let interval_secs = timer.interval / 1000;
        let interval_msecs = timer.interval % 1000;
        let mut tmspec: libc::itimerspec = unsafe { std::mem::zeroed() };

        tmspec.it_value.tv_sec = initial_expiry_secs as i64;
        tmspec.it_value.tv_nsec = (initial_expiry_msecs as i64) * 1_000_000;
        tmspec.it_interval.tv_sec = interval_secs as i64;
        tmspec.it_interval.tv_nsec = (interval_msecs as i64) * 1_000_000;

        if unsafe {
            // start the timer with an expiry
            libc::timer_settime(timer.tid.0, 0, &tmspec, std::ptr::null_mut())
        } < 0
        {
            bail!(std::io::Error::last_os_error())
        }

        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn start(_: u64, _: u64, _: CPUAlarmVal) -> Result<Self, Error> {
        log::error!("CPU timer: not enabled (need Linux)");
        Ok(Self {})
    }

    #[cfg(not(target_os = "linux"))]
    pub fn reset(&self) -> Result<(), Error> {
        Ok(())
    }
}

pub fn get_thread_time() -> Result<i64, Error> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };

    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) } == -1 {
        return Err(std::io::Error::last_os_error().into());
    }

    // convert seconds to nanoseconds and add to nsec value
    Ok(time.tv_sec * 1_000_000_000 + time.tv_nsec)
}

#[cfg_attr(target_os = "linux", linux::ctor)]
#[cfg(target_os = "linux")]
fn register_sigalrm() {
    use std::collections::HashMap;

    use futures::StreamExt;
    use linux::SignalMsg;
    use log::{debug, error};
    use signal_hook::{consts::signal, iterator::exfiltrator::raw};
    use signal_hook_tokio::SignalsInfo;

    let (sig_timer_id_tx, mut sig_timer_id_rx) = mpsc::unbounded_channel::<usize>();

    let mut registry = HashMap::<usize, CPUTimer>::new();

    let sig_msg_tx = linux::SIG_MSG_CHAN.0.clone();
    let mut sig_msg_rx = linux::SIG_MSG_CHAN.1.lock().unwrap().take().unwrap();

    std::thread::Builder::new()
        .name("sb-cpu-timer".into())
        .spawn(|| {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let sig_receiver_handle = rt.spawn(async move {
                let mut signals = SignalsInfo::with_exfiltrator([signal::SIGALRM], raw::WithRawSiginfo).unwrap();

                while let Some(siginfo) = signals.next().await {
                    let _ = sig_timer_id_tx.send(unsafe { siginfo.si_value().sival_ptr as usize });
                }
            });

            let msg_handle = rt.spawn(async move {
                loop {
                    tokio::select! {
                        Some(msg) = sig_msg_rx.recv() => {
                            match msg {
                                SignalMsg::Alarm(ref timer_id) => {
                                    if let Some(cpu_timer) = registry.get(timer_id) {
                                        let tx = cpu_timer.cpu_alarm_val.cpu_alarms_tx.clone();

                                        if tx.send(()).is_err() {
                                            debug!("failed to send cpu alarm to the provided channel");
                                        }
                                    } else {
                                        // NOTE: Unix signals are being
                                        // delivered asynchronously, and there
                                        // are no guarantees to cancel the
                                        // signal after a timer has been
                                        // deleted, and after a signal is
                                        // received, there may no longer be a
                                        // target to accept it.
                                        error!("can't find the cpu alarm signal matched with the received timer id: {}", *timer_id);
                                    }
                                }

                                SignalMsg::Add((timer_id, cpu_timer)) => {
                                    let _ = registry.insert(timer_id, cpu_timer);
                                }

                                SignalMsg::Remove(ref timer_id) => {
                                    let _ = registry.remove(timer_id);
                                }
                            }
                        }

                        Some(id) = sig_timer_id_rx.recv() => {
                            let _ = sig_msg_tx.send(SignalMsg::Alarm(id));
                        }
                    }
                }
            });

            rt.block_on(async move {
                let _ = tokio::join!(sig_receiver_handle, msg_handle);
            });
        })
        .unwrap();
}
