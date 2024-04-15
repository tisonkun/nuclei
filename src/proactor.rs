use std::task::{Context, Poll};
use std::time::Duration;
use std::{future::Future, io};

use crate::config::NucleiConfig;
use once_cell::sync::OnceCell;
use waker_fn::waker_fn;

use super::syscore::*;
use crate::spawn_blocking;
use crate::sys::IoBackend;

pub use super::handle::*;

///
/// Concrete proactor instance
pub struct Proactor(pub(crate) SysProactor);
unsafe impl Send for Proactor {}
unsafe impl Sync for Proactor {}

static mut PROACTOR: OnceCell<Proactor> = OnceCell::new();

impl Proactor {
    /// Returns a reference to the proactor.
    pub fn get() -> &'static Proactor {
        unsafe {
            PROACTOR.get_or_init(|| {
                Proactor(
                    SysProactor::new(NucleiConfig::default())
                        .expect("cannot initialize IO backend"),
                )
            })
        }
    }

    /// Builds a proactor instance with given config and returns a reference to it.
    pub fn with_config(config: NucleiConfig) -> &'static Proactor {
        unsafe {
            let proactor =
                Proactor(SysProactor::new(config.clone()).expect("cannot initialize IO backend"));
            PROACTOR
                .set(proactor)
                .map_err(|e| "Proactor instance not being able to set.")
                .unwrap();

            PROACTOR.wait()
        }
    }

    /// Wakes the thread waiting on proactor.
    pub fn wake(&self) {
        self.0.wake().expect("failed to wake thread");
    }

    /// Wait for completion of IO object
    pub fn wait(&self, max_event_size: usize, duration: Option<Duration>) -> io::Result<usize> {
        self.0.wait(max_event_size, duration)
    }

    /// Get the IO backend that is used with Nuclei's proactor.
    pub fn backend() -> IoBackend {
        BACKEND
    }

    /// Get underlying proactor instance.
    pub(crate) fn inner(&self) -> &SysProactor {
        &self.0
    }

    #[cfg(all(feature = "iouring", target_os = "linux"))]
    /// Get IO_URING backend probes
    pub fn ring_params(&self) -> &rustix_uring::Parameters {
        unsafe { IO_URING.as_ref().unwrap().params() }
    }
}

///
/// IO driver that drives underlying event systems
pub fn drive<T>(future: impl Future<Output = T>) -> T {
    let p = Proactor::get();
    let waker = waker_fn(move || {
        p.wake();
    });

    let cx = &mut Context::from_waker(&waker);
    futures::pin_mut!(future);

    let driver = spawn_blocking(move || loop {
        let _ = p.wait(1, None);
    });

    futures::pin_mut!(driver);

    loop {
        if let Poll::Ready(val) = future.as_mut().poll(cx) {
            return val;
        }

        cx.waker().wake_by_ref();

        // TODO: (vcq): we don't need this.
        // let _duration = Duration::from_millis(1);
        let _ = driver.as_mut().poll(cx);
    }
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod proactor_tests {
    use crate::config::{IoUringConfiguration, NucleiConfig};
    use crate::Proactor;

    #[test]
    #[ignore]
    fn proactor_with_defaults() {
        let old = Proactor::get();

        let osq = old.0.sq.lock();
        let olen = osq.capacity();
        drop(osq);
        dbg!(olen);

        assert_eq!(olen, 2048);
    }

    #[test]
    fn proactor_with_config_rollup() {
        let config = NucleiConfig {
            iouring: IoUringConfiguration {
                queue_len: 10,
                sqpoll_wake_interval: Some(11),
                per_numa_bounded_worker_count: Some(12),
                per_numa_unbounded_worker_count: Some(13),
                ..IoUringConfiguration::default()
            },
        };
        let new = Proactor::with_config(config);
        let old = Proactor::get();

        let nsq = new.0.sq.lock();
        let nlen = nsq.capacity();
        drop(nsq);

        let osq = old.0.sq.lock();
        let olen = osq.capacity();
        drop(osq);

        dbg!(nlen);
        dbg!(olen);

        assert_eq!(nlen, olen);
        assert_eq!(nlen, 16);
        assert_eq!(olen, 16);
    }
}
