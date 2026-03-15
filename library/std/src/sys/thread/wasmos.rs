use core::ffi::c_void;
#[cfg(target_arch = "wasm32")]
use core::arch::global_asm;

use crate::ffi::CStr;
use crate::num::NonZero;
use crate::sync::Arc;
use crate::sync::atomic::{AtomicI32, Ordering};
use crate::sys::wasmos;
use crate::thread::ThreadInit;
use crate::time::Duration;
use crate::vec;
use crate::vec::Vec;

#[cfg(target_arch = "wasm32")]
global_asm!(
    r#"
    .globaltype __stack_pointer, i32
    .globaltype __tls_base, i32

    .globl __wasmos_set_thread_state
    .type __wasmos_set_thread_state,@function
__wasmos_set_thread_state:
    .functype __wasmos_set_thread_state (i32, i32) -> ()
    local.get 0
    global.set __stack_pointer
    local.get 1
    global.set __tls_base
    end_function

    .globl __wasmos_get_tls_base
    .type __wasmos_get_tls_base,@function
__wasmos_get_tls_base:
    .functype __wasmos_get_tls_base () -> (i32)
    global.get __tls_base
    end_function
"#
);

#[cfg(target_arch = "wasm32")]
unsafe extern "C" {
    fn __wasmos_set_thread_state(stack: *mut u8, tls: *mut u8);
    fn __wasmos_get_tls_base() -> u32;
    fn __wasmos_clone_trampoline_asm(func: usize, arg: *mut c_void, stack: *mut u8, tls: *mut u8);
}

#[cfg(target_arch = "wasm32")]
global_asm!(
    r#"
    .functype __wasmos_thread_entry_call (i32, i32) -> ()

    .globl __wasmos_clone_trampoline_asm
    .type __wasmos_clone_trampoline_asm,@function
__wasmos_clone_trampoline_asm:
    .functype __wasmos_clone_trampoline_asm (i32, i32, i32, i32) -> ()
    local.get 2
    local.get 3
    call __wasmos_set_thread_state
    local.get 0
    local.get 1
    call __wasmos_thread_entry_call
    end_function
"#
);

const CLONE_VM: u32 = 0x0000_0100;
const CLONE_THREAD: u32 = 0x0001_0000;
const CLONE_PARENT_SETTID: u32 = 0x0010_0000;
const CLONE_CHILD_CLEARTID: u32 = 0x0020_0000;
const CLONE_CHILD_SETTID: u32 = 0x0100_0000;

pub const DEFAULT_MIN_STACK_SIZE: usize = 128 * 1024;
const THREAD_TLS_ALIGN: usize = 64;
const THREAD_TLS_BYTES: usize = 64 * 1024;

#[repr(C)]
struct ThreadState {
    tid: AtomicI32,
    stack: Vec<u8>,
    tls: Vec<u8>,
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
            tls: vec![0u8; THREAD_TLS_BYTES + THREAD_TLS_ALIGN - 1],
        });
        let stack_base = state.stack.as_ptr() as usize;
        let stack_end = (stack_base + state.stack.len()) & !0xf;
        let stack_ptr = stack_end - 8;
        let tls_ptr = aligned_tls_ptr(&state.tls, THREAD_TLS_ALIGN);
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
        let child_tid = match wasmos::clone_thread(
            flags,
            stack_ptr as u32,
            tid_addr,
            tls_ptr as u32,
            tid_addr,
        )
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

    pub fn id(&self) -> u64 {
        self.state.tid.load(Ordering::Acquire).max(1) as u64
    }

    pub fn into_id(self) -> u64 {
        self.id()
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

#[unsafe(no_mangle)]
extern "C" fn __wasmos_thread_entry_call(func: usize, arg: *mut c_void) {
    let func: extern "C" fn(*mut c_void) -> i32 = unsafe { core::mem::transmute(func) };
    let _ = func(arg);
}

#[unsafe(export_name = "__wasmos_clone_trampoline")]
pub extern "C" fn wasmos_clone_trampoline(
    func: extern "C" fn(*mut c_void) -> i32,
    arg: *mut c_void,
    stack: *mut u8,
    tls: *mut u8,
) {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        __wasmos_clone_trampoline_asm(func as usize, arg, stack, tls);
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = func(arg);
    }
}

#[unsafe(export_name = "__wasmos_tls_base")]
pub extern "C" fn wasmos_tls_base() -> u32 {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        __wasmos_get_tls_base()
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        0
    }
}

fn aligned_tls_ptr(tls: &[u8], align: usize) -> *mut u8 {
    if tls.is_empty() {
        return core::ptr::null_mut();
    }
    let base = tls.as_ptr() as usize;
    let aligned = (base + (align - 1)) & !(align - 1);
    aligned as *mut u8
}
