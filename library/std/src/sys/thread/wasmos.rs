use crate::sys::wasmos;
use crate::time::Duration;

pub fn yield_now() {
    wasmos::sleep_ms(0);
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
