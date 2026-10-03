//! Everything OS-specific lives here (CLAUDE.md non-negotiable #6).
//!
//! P1: process memory statistics for `vt-bench`.
//! P2+: keyboard hook, text insertion, clipboard guard, foreground app.

pub mod process {
    /// Peak resident memory of the current process in bytes (Windows: peak
    /// working set; Linux: `VmHWM`). `None` where unsupported.
    pub fn peak_rss_bytes() -> Option<u64> {
        imp::peak_rss_bytes()
    }

    #[cfg(windows)]
    #[allow(unsafe_code)]
    mod imp {
        use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        pub fn peak_rss_bytes() -> Option<u64> {
            let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
            let cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
            counters.cb = cb;
            // SAFETY: `counters` is a valid, writable PROCESS_MEMORY_COUNTERS of size `cb`;
            // GetCurrentProcess returns a pseudo-handle that needs no closing.
            let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, cb) };
            (ok != 0).then_some(counters.PeakWorkingSetSize as u64)
        }
    }

    #[cfg(target_os = "linux")]
    mod imp {
        pub fn peak_rss_bytes() -> Option<u64> {
            let status = std::fs::read_to_string("/proc/self/status").ok()?;
            parse_vm_hwm(&status)
        }

        pub(super) fn parse_vm_hwm(status: &str) -> Option<u64> {
            let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
            let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
            Some(kb * 1024)
        }
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    mod imp {
        pub fn peak_rss_bytes() -> Option<u64> {
            None
        }
    }

    #[cfg(all(test, target_os = "linux"))]
    mod tests {
        #[test]
        fn parses_vm_hwm() {
            let s = "Name:\tx\nVmPeak:\t  999 kB\nVmHWM:\t  2048 kB\nVmRSS:\t 1000 kB\n";
            assert_eq!(super::imp::parse_vm_hwm(s), Some(2048 * 1024));
            assert_eq!(super::imp::parse_vm_hwm("nothing"), None);
        }

        #[test]
        fn reports_nonzero_for_self() {
            assert!(super::peak_rss_bytes().unwrap_or(0) > 0);
        }
    }
}
