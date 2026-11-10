#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn clock() -> std::ffi::c_ulong;
}

pub fn process_ns() -> u64 {
    #[cfg(target_os = "macos")]
    {
        // Darwin clock_t is unsigned long; CLOCKS_PER_SEC is 1,000,000.
        let ticks = unsafe { clock() };
        assert_ne!(ticks, std::ffi::c_ulong::MAX, "process CPU clock failed");
        ticks * 1000
    }
    #[cfg(not(target_os = "macos"))]
    compile_error!("this probe's process CPU clock and time -l parser target macOS");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_clock_advances_under_load() {
        let before = process_ns();
        for i in 0..100_000_u64 {
            std::hint::black_box(i.wrapping_mul(i));
        }
        assert!(process_ns() > before);
    }
}
