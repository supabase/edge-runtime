pub mod timerid;

use std::sync::Arc;

#[cfg(target_os = "linux")]
use crate::timerid::TimerId;

#[cfg(target_os = "linux")]
use anyhow::bail;
use anyhow::Error;
use log::debug;
use nix::sys::signal;
use tokio::sync::{mpsc, Mutex};

#[repr(C)]
#[derive(Clone)]
pub struct CPUAlarmVal {
    pub cpu_alarms_tx: mpsc::UnboundedSender<()>,
}

#[cfg(target_os = "linux")]
struct CPUTimerVal {
    id: TimerId,
    initial_expiry: u64,
    interval: u64,
}

#[cfg(target_os = "linux")]
unsafe impl Send for CPUTimerVal {}

#[cfg(target_os = "linux")]
#[derive(Clone)]
pub struct CPUTimer {
    _timer: Arc<Mutex<CPUTimerVal>>,
    _cpu_alarm_val: Arc<CPUAlarmVal>,
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
        let mut timerid = TimerId(std::ptr::null_mut());
        let mut sigev: libc::sigevent = unsafe { std::mem::zeroed() };
        let cpu_alarm_val = Arc::new(cpu_alarm_val);

        sigev.sigev_notify = libc::SIGEV_SIGNAL;
        sigev.sigev_signo = libc::SIGALRM;
        sigev.sigev_value = libc::sigval {
            sival_ptr: Arc::as_ptr(&cpu_alarm_val) as *mut _,
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
            _timer: Arc::new(Mutex::new(CPUTimerVal {
                id: timerid,
                initial_expiry,
                interval,
            })),
            _cpu_alarm_val: cpu_alarm_val,
        };

        Ok({
            this.reset()?;
            this
        })
    }

    #[cfg(target_os = "linux")]
    pub fn reset(&self) -> Result<(), Error> {
        use anyhow::Context;

        let timer = self._timer.try_lock().context("failed to get the lock")?;

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
            libc::timer_settime(timer.id.0, 0, &tmspec, std::ptr::null_mut())
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
    pub fn reset() -> Result<(), Error> {
        Ok(())
    }
}

extern "C" fn sigalrm_handler(_: libc::c_int, info: *mut libc::siginfo_t, _: *mut libc::c_void) {
    let cpu_alarms_tx = unsafe {
        let sival = (*info).si_value();
        let val = Arc::from_raw(sival.sival_ptr as *const CPUAlarmVal);
        let sig = val.cpu_alarms_tx.clone();

        std::mem::forget(val);
        sig
    };

    if cpu_alarms_tx.send(()).is_err() {
        debug!("failed to send cpu alarm to the provided channel");
    }
}

pub fn register_alarm() -> Result<(), Error> {
    let sig_handler = signal::SigHandler::SigAction(sigalrm_handler);
    let sig_action = signal::SigAction::new(
        sig_handler,
        signal::SaFlags::empty(),
        signal::SigSet::empty(),
    );
    unsafe {
        signal::sigaction(signal::SIGALRM, &sig_action)?;
    }
    Ok(())
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
