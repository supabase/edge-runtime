pub struct TimerId(pub *mut libc::c_void);

#[cfg(target_os = "linux")]
impl Drop for TimerId {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { libc::timer_delete(self.0) };
        }
    }
}
