use core::ffi::c_void;
#[cfg(target_arch = "wasm32")]
use core::arch::asm;

use crate::ffi::CStr;
use crate::num::NonZero;
use crate::sync::Arc;
use crate::sync::atomic::{AtomicI32, Ordering};
use crate::sys::wasmos;
use crate::thread::ThreadInit;
use crate::time::Duration;
use crate::vec;
use crate::vec::Vec;

const CLONE_VM: u32 = 0x0000_0100;
const CLONE_THREAD: u32 = 0x0001_0000;
const CLONE_PARENT_SETTID: u32 = 0x0010_0000;
const CLONE_CHILD_CLEARTID: u32 = 0x0020_0000;
const CLONE_CHILD_SETTID: u32 = 0x0100_0000;

pub const DEFAULT_MIN_STACK_SIZE: usize = 128 * 1024;

#[repr(C)]
struct ThreadState {
    tid: AtomicI32,
    stack: Vec<u8>,
}

#[repr(C)]
struct ThreadStartData {
    init: Box<ThreadInit>,
    state: Arc<ThreadState>,
}

pub struct Thread {
    state: Arc<ThreadState>,
}

impl Thread {
    pub unsafe fn new(stack: usize, init: Box<ThreadInit>) -> crate::io::Result<Thread> {
        let stack_size = stack.max(DEFAULT_MIN_STACK_SIZE);
        let state = Arc::new(ThreadState {
            tid: AtomicI32::new(0),
            stack: vec![0u8; stack_size + 16],
        });
        let stack_base = state.stack.as_ptr() as usize;
        let stack_end = (stack_base + state.stack.len()) & !0xf;
        let stack_ptr = stack_end - 8;
        let data = Box::into_raw(Box::new(ThreadStartData {
            init,
            state: state.clone(),
        }));

        unsafe {
            let meta = stack_ptr as *mut u32;
            meta.write(data as u32);
            meta.add(1).write(thread_start as usize as u32);
        }

        let tid_addr = (&state.tid as *const AtomicI32).cast_mut().cast::<i32>();
        let flags = CLONE_VM
            | CLONE_THREAD
            | CLONE_PARENT_SETTID
            | CLONE_CHILD_CLEARTID
            | CLONE_CHILD_SETTID;
        let child_tid = match wasmos::clone_thread(flags, stack_ptr as u32, tid_addr, 0, tid_addr)
        {
            Ok(tid) => tid,
            Err(errno) => {
                drop(unsafe { Box::from_raw(data) });
                return Err(wasmos::io_error(errno));
            }
        };

        state.tid.store(child_tid as i32, Ordering::Release);
        Ok(Thread { state })
    }

    pub fn join(self) {
        let tid_addr = (&self.state.tid as *const AtomicI32).cast_mut().cast::<i32>();
        loop {
            let tid = self.state.tid.load(Ordering::Acquire);
            if tid == 0 {
                return;
            }
            match wasmos::futex_wait(tid_addr, tid as u32, wasmos::FUTEX_WAIT_FOREVER) {
                Ok(()) => {}
                Err(errno)
                    if errno == wasmos::EAGAIN
                        || errno == wasmos::EINTR
                        || errno == wasmos::ETIMEDOUT => {}
                Err(errno) => panic!("failed to join thread: {}", wasmos::io_error(errno)),
            }
        }
    }
}

pub fn yield_now() {
    wasmos::yield_now();
}

pub fn sleep(dur: Duration) {
    let millis = dur.as_millis();
    if millis == 0 {
        if dur.is_zero() {
            return;
        }
        wasmos::sleep_ms(1);
        return;
    }

    let mut remaining = millis;
    while remaining != 0 {
        let chunk = remaining.min(u32::MAX as u128) as u32;
        wasmos::sleep_ms(chunk);
        remaining -= chunk as u128;
    }
}

pub fn available_parallelism() -> crate::io::Result<NonZero<usize>> {
    Ok(NonZero::new(1).unwrap())
}

pub fn current_os_id() -> Option<u64> {
    wasmos::gettid().ok().map(u64::from)
}

pub fn set_name(_name: &CStr) {}

extern "C" fn thread_start(data: *mut c_void) -> i32 {
    let data = unsafe { Box::from_raw(data.cast::<ThreadStartData>()) };
    let rust_start = data.init.init();
    rust_start();
    0
}

#[unsafe(export_name = "__wasmos_clone_trampoline")]
pub extern "C" fn wasmos_clone_trampoline(
    func: extern "C" fn(*mut c_void) -> i32,
    arg: *mut c_void,
    stack: *mut u8,
    _tls: *mut u8,
) {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        asm!(
            "local.get {stack}",
            "global.set __stack_pointer",
            stack = in(local) stack as i32,
        );
    }
    let _ = func(arg);
}
