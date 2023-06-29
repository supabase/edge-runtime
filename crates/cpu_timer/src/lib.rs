pub mod timerid;

#[cfg(target_os = "linux")]
use crate::timerid::TimerId;

#[cfg(target_os = "linux")]
use anyhow::bail;
use anyhow::Error;
use log::debug;
use nix::sys::signal;
use tokio::sync::mpsc;

#[repr(C)]
pub struct CPUAlarmVal {
    pub cpu_alarms_tx: mpsc::UnboundedSender<()>,
}

#[cfg(target_os = "linux")]
pub struct CPUTimer {
    _timerid: TimerId,
    val_ptr: *mut CPUAlarmVal,
}
#[cfg(not(target_os = "linux"))]
pub struct CPUTimer {}

impl CPUTimer {
    #[cfg(target_os = "linux")]
    pub fn start(interval: u64, cpu_alarm_val: CPUAlarmVal) -> Result<Self, Error> {
        let mut timerid = TimerId(std::ptr::null_mut());
        let val_ptr = Box::into_raw(Box::new(cpu_alarm_val));
        let sival_ptr: *mut libc::c_void = val_ptr as *mut libc::c_void;

        let mut sigev: libc::sigevent = unsafe { std::mem::zeroed() };
        sigev.sigev_notify = libc::SIGEV_SIGNAL;
        sigev.sigev_signo = libc::SIGALRM;
        sigev.sigev_value = libc::sigval { sival_ptr };

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

        let mut tmspec: libc::itimerspec = unsafe { std::mem::zeroed() };
        tmspec.it_interval.tv_sec = 0;
        tmspec.it_interval.tv_nsec = (interval as i64) * 1_000_000;
        tmspec.it_value.tv_sec = 0;
        tmspec.it_value.tv_nsec = (interval as i64) * 1_000_000;

        if unsafe {
            // start the timer with an expiry
            libc::timer_settime(timerid.0, 0, &tmspec, std::ptr::null_mut())
        } < 0
        {
            bail!(std::io::Error::last_os_error())
        }

        Ok(Self {
            _timerid: timerid,
            val_ptr,
        })
    }

    #[cfg(not(target_os = "linux"))]
    pub fn start(_: i64, _: CPUAlarmVal) -> Result<Self, Error> {
        println!("CPU timer: not enabled (need Linux)");
        Ok(Self {})
    }
}

#[cfg(target_os = "linux")]
impl Drop for CPUTimer {
    fn drop(&mut self) {
        // reconstruct the CPUAlarmVal so its memory freed up correctly
        unsafe {
            let _ = Box::from_raw(self.val_ptr);
        }
    }
}

extern "C" fn sigalrm_handler(_: libc::c_int, info: *mut libc::siginfo_t, _: *mut libc::c_void) {
    let cpu_alarms_tx: mpsc::UnboundedSender<()>;
    unsafe {
        let sival = (*info).si_value();
        let boxed_state = Box::from_raw(sival.sival_ptr as *mut CPUAlarmVal);
        cpu_alarms_tx = boxed_state.cpu_alarms_tx.clone();
        std::mem::forget(boxed_state);
    }

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

    Ok(time.tv_nsec)
}
