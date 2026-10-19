//! Bounded wildcard PDH reads shared by thermal and GPU counters.
use std::{mem::size_of, ptr::null_mut};
use windows_sys::Win32::System::Performance::*;

// pdh.h flags omitted by Windows metadata: native units and uncapped percentages.
const PDH_FMT_NOSCALE: PDH_FMT = 0x0000_1000;
const PDH_FMT_NOCAP100: PDH_FMT = 0x0000_8000;

pub(super) fn values(counter: PDH_HCOUNTER) -> Result<Vec<(String, f64)>, u32> {
    // SAFETY: both calls use a live counter and the same DOUBLE format. The
    // allocation is aligned for ITEM_W, with additional storage for instance names.
    // PDH returns name pointers into that buffer, copied before it is dropped.
    unsafe {
        for _ in 0..3 {
            let mut bytes = 0;
            let mut count = 0;
            let format = PDH_FMT_DOUBLE | PDH_FMT_NOSCALE | PDH_FMT_NOCAP100;
            let status =
                PdhGetFormattedCounterArrayW(counter, format, &mut bytes, &mut count, null_mut());
            if status != PDH_MORE_DATA {
                return if status == 0 {
                    Ok(Vec::new())
                } else {
                    Err(status)
                };
            }
            if bytes == 0 || bytes > 16 * 1024 * 1024 {
                return Err(PDH_INVALID_BUFFER);
            }
            let slots = (bytes as usize).div_ceil(size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>());
            let mut buffer = vec![PDH_FMT_COUNTERVALUE_ITEM_W::default(); slots];
            let status = PdhGetFormattedCounterArrayW(
                counter,
                format,
                &mut bytes,
                &mut count,
                buffer.as_mut_ptr(),
            );
            if status == PDH_MORE_DATA {
                continue; // Instance count changed. Ask for the size again.
            }
            if status != 0 {
                return Err(status);
            }
            if count as usize > buffer.len() {
                return Err(PDH_INVALID_BUFFER);
            }
            let mut output = Vec::new();
            for item in &buffer[..count as usize] {
                if item.szName.is_null()
                    || !matches!(
                        item.FmtValue.CStatus,
                        PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA
                    )
                {
                    continue;
                }
                if let Ok(name) = windows::core::PCWSTR(item.szName).to_string() {
                    output.push((name, item.FmtValue.Anonymous.doubleValue));
                }
            }
            return Ok(output);
        }
        Err(PDH_MORE_DATA)
    }
}
