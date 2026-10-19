//! Bounded wildcard PDH reads shared by thermal and GPU counters.
use std::{mem::size_of, ptr::null_mut};
use windows_sys::Win32::System::Performance::*;

// pdh.h flags omitted by Windows metadata: native units and uncapped percentages.
const PDH_FMT_NOSCALE: PDH_FMT = 0x0000_1000;
const PDH_FMT_NOCAP100: PDH_FMT = 0x0000_8000;
const MAX_ARRAY_BYTES: usize = 16 * 1024 * 1024;
const RETAINED_ARRAY_BYTES: usize = 256 * 1024;

#[derive(Default)]
pub(super) struct Array {
    items: Vec<PDH_FMT_COUNTERVALUE_ITEM_W>,
    name: String,
}

impl Array {
    /// Names borrow a single UTF-8 scratch buffer and live only for the callback.
    pub fn read(
        &mut self,
        counter: PDH_HCOUNTER,
        mut visit: impl FnMut(&str, f64),
    ) -> Result<(), u32> {
        let result = self.collect(counter, &mut visit);
        // Keep normal buffers warm without retaining a transient large process list.
        if self.items.capacity() * size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>() > RETAINED_ARRAY_BYTES {
            self.items = Vec::new();
        }
        if self.name.capacity() > 4096 {
            self.name = String::new();
        }
        result
    }

    fn collect(
        &mut self,
        counter: PDH_HCOUNTER,
        visit: &mut impl FnMut(&str, f64),
    ) -> Result<(), u32> {
        // SAFETY: PDH writes into aligned storage owned by this worker. All name
        // pointers are checked against the returned buffer and consumed before reuse.
        unsafe {
            for _ in 0..3 {
                let mut bytes = 0;
                let mut count = 0;
                let format = PDH_FMT_DOUBLE | PDH_FMT_NOSCALE | PDH_FMT_NOCAP100;
                let status = PdhGetFormattedCounterArrayW(
                    counter,
                    format,
                    &mut bytes,
                    &mut count,
                    null_mut(),
                );
                if status != PDH_MORE_DATA {
                    return if status == 0 { Ok(()) } else { Err(status) };
                }
                if bytes == 0 || bytes as usize > MAX_ARRAY_BYTES {
                    return Err(PDH_INVALID_BUFFER);
                }
                let slots = (bytes as usize).div_ceil(size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>());
                if self.items.len() < slots {
                    self.items
                        .resize(slots, PDH_FMT_COUNTERVALUE_ITEM_W::default());
                }
                let supplied = bytes;
                let status = PdhGetFormattedCounterArrayW(
                    counter,
                    format,
                    &mut bytes,
                    &mut count,
                    self.items.as_mut_ptr(),
                );
                if status == PDH_MORE_DATA {
                    continue;
                }
                if status != 0 {
                    return Err(status);
                }
                if bytes > supplied
                    || count as usize > bytes as usize / size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>()
                {
                    return Err(PDH_INVALID_BUFFER);
                }
                self.visit(count as usize, bytes as usize, visit);
                return Ok(());
            }
            Err(PDH_MORE_DATA)
        }
    }

    fn visit(&mut self, count: usize, bytes: usize, visit: &mut impl FnMut(&str, f64)) {
        let base = self.items.as_ptr() as usize;
        for item in &self.items[..count] {
            if !matches!(
                item.FmtValue.CStatus,
                PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA
            ) {
                continue;
            }
            let Some(offset) = (item.szName as usize).checked_sub(base) else {
                continue;
            };
            if offset >= bytes || offset % 2 != 0 {
                continue;
            }
            // SAFETY: the aligned name lies inside the native buffer, bounded by
            // its returned byte count. No pointer or borrowed name escapes visit.
            let wide = unsafe { std::slice::from_raw_parts(item.szName, (bytes - offset) / 2) };
            let Some(end) = wide.iter().position(|&unit| unit == 0) else {
                continue;
            };
            self.name.clear();
            let mut valid = true;
            for character in char::decode_utf16(wide[..end].iter().copied()) {
                match character {
                    Ok(character) => self.name.push(character),
                    Err(_) => {
                        valid = false;
                        break;
                    }
                }
            }
            if valid {
                // SAFETY: the array was requested with PDH_FMT_DOUBLE.
                visit(&self.name, unsafe { item.FmtValue.Anonymous.doubleValue });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streamed_names_are_bounded_and_invalid_entries_are_skipped() {
        let item_size = size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        let names: Vec<_> = "GPU_温度\0bad\0"
            .encode_utf16()
            .chain([0xd800, 0])
            .collect();
        let mut array = Array::default();
        let bytes = 4 * item_size + names.len() * 2;
        array.items.resize(
            bytes.div_ceil(item_size),
            PDH_FMT_COUNTERVALUE_ITEM_W::default(),
        );
        let base = array.items.as_mut_ptr();
        // SAFETY: the name area follows the four items and fits the allocation.
        unsafe {
            let wide = base.add(4).cast::<u16>();
            std::ptr::copy_nonoverlapping(names.as_ptr(), wide, names.len());
            for index in 0..4 {
                (*base.add(index)).FmtValue.CStatus = PDH_CSTATUS_VALID_DATA;
                (*base.add(index)).FmtValue.Anonymous.doubleValue = index as f64;
            }
            (*base).szName = wide;
            (*base.add(1)).szName = wide.add(7);
            (*base.add(1)).FmtValue.CStatus = PDH_CSTATUS_INVALID_DATA;
            (*base.add(2)).szName = wide.add(11);
            (*base.add(3)).szName = base.cast::<u8>().add(bytes).cast();
        }
        let mut seen = Vec::new();
        array.visit(4, bytes, &mut |name, value| {
            seen.push((name.to_owned(), value))
        });
        assert_eq!(seen, [("GPU_温度".into(), 0.0)]);
    }
}
